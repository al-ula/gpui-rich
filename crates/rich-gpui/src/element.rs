//! Custom GPUI [`Element`] implementing virtualized non-paginated 6-layer rich text painting.

use crate::shaper::{GpuiGlyphBackend, GpuiShaper, rgba_to_hsla};
use crate::theme::RichTheme;
use crate::view::{ImePreedit, RichDocumentView};
#[cfg(not(feature = "gpui-fast"))]
use gpui_kit::IntoElement;
use gpui_kit::{
    App, BorderStyle, Bounds, BoxShadow, ContentMask, Corners, CursorStyle, Edges, Element,
    ElementId, ElementInputHandler, Entity, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior,
    Hsla, InspectorElementId, LayoutId, Length, Pixels, Point, SharedString, StrikethroughStyle,
    Style, TextRun, UnderlineStyle, Window, fill, outline, point, px, quad, size,
};
use rich_layout::{
    BlockLayout, BlockLayoutKind, HorizontalMetrics, InlineBoxFragment, LineFragment,
    ParagraphLayout, Rect, TextFragment, VisibleBlock, pt_to_px, resolve_horizontal_metrics,
};
use rich_model::{Affinity, DocumentSelection, InlineObject, UnderlineKind};

const OVERSCAN_PX: f32 = 300.0;

/// Custom GPUI element painting the continuous non-paginated document canvas.
pub struct RichDocumentElement {
    view: Entity<RichDocumentView>,
}

impl RichDocumentElement {
    /// Creates a document canvas element bound to `view`.
    pub fn new(view: Entity<RichDocumentView>) -> Self {
        Self { view }
    }
}

// `gpui-fast` supplies a blanket `impl<T: Element> IntoElement for T`, while upstream `gpui-pre` requires an explicit impl.
#[cfg(not(feature = "gpui-fast"))]
impl IntoElement for RichDocumentElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Prepaint payload for painting an ephemeral IME preedit composition overlay on Layer 5.
struct PrepaintPreedit {
    preedit: ImePreedit,
    anchor_rect: Rect,
    font_size_px: f32,
}

/// Prepaint payload passed from `Element::prepaint` into `Element::paint`.
pub struct RichPrepaintState {
    visible_blocks: Vec<VisibleBlock<GpuiGlyphBackend>>,
    selection_rects: Vec<Rect>,
    caret_rect: Option<Rect>,
    preedit_overlay: Option<PrepaintPreedit>,
    scroll_x: f32,
    scroll_y: f32,
    total_height: f32,
    horizontal: HorizontalMetrics,
    zoom: f32,
    debug_overlay: bool,
    show_heading_chevrons: bool,
    editable: bool,
    focused: bool,
    focus_handle: FocusHandle,
    theme: RichTheme,
    cursor_style: CursorStyle,
    hitbox: Hitbox,
}

impl Element for RichDocumentElement {
    type RequestLayoutState = ();
    type PrepaintState = RichPrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("rich-document-canvas".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = Length::Definite(gpui_kit::relative(1.0).into());
        style.size.height = Length::Definite(gpui_kit::relative(1.0).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport_width: f32 = bounds.size.width.into();
        let viewport_height: f32 = bounds.size.height.into();

        let (
            visible_blocks,
            selection_rects,
            caret_rect,
            preedit_overlay,
            scroll_x,
            scroll_y,
            total_height,
            horizontal,
            zoom,
            debug_overlay,
            show_heading_chevrons,
            editable,
            focused,
            focus_handle,
            theme,
            cursor_style,
        ) = self.view.update(cx, |view, cx| {
            view.last_viewport_bounds = bounds;
            let focused = view.focus_handle.is_focused(window);
            view.sync_focus_state(focused, cx);

            let horizontal =
                resolve_horizontal_metrics(viewport_width, &view.document.config, view.zoom);
            let max_scroll_x = (horizontal.canvas_width - viewport_width).max(0.0);
            view.scroll_x = view.scroll_x.clamp(0.0, max_scroll_x);

            let anchor = view.flow.capture_anchor(view.scroll_y);
            let mut shaper = GpuiShaper::new(window);
            let visible = view.flow.layout_visible_blocks(
                &view.document,
                view.scroll_y,
                viewport_width,
                viewport_height,
                OVERSCAN_PX,
                view.zoom,
                &mut shaper,
            );

            if let Some(captured) = anchor {
                view.scroll_y = view.flow.restore_anchor(captured, viewport_height);
            } else {
                let max_scroll_y = (view.flow.total_height() - viewport_height).max(0.0);
                view.scroll_y = view.scroll_y.clamp(0.0, max_scroll_y);
            }

            let selection_rects = view
                .selection
                .as_ref()
                .map(|selection| view.flow.selection_rects_for_visible(selection, &visible))
                .unwrap_or_default();

            let preedit_overlay = view.ime_preedit.as_ref().and_then(|preedit| {
                let anchor_pos = view
                    .document
                    .doc_pos_for_block(preedit.block_id, preedit.replace_range.start)?;
                let anchor_rect =
                    view.flow
                        .caret_rect_in_visible(&anchor_pos, Affinity::Downstream, &visible)?;
                let block_font_pt = view
                    .document
                    .find_block(preedit.block_id)
                    .map(|block| {
                        block
                            .style
                            .resolved_base_text(
                                &view.document.stylesheet,
                                &view.document.config.default_style,
                            )
                            .font_size_pt
                    })
                    .unwrap_or(view.document.config.default_style.font_size_pt);
                Some(PrepaintPreedit {
                    preedit: preedit.clone(),
                    anchor_rect,
                    font_size_px: pt_to_px(block_font_pt, view.zoom).max(10.0),
                })
            });

            let caret_rect = if focused
                && view.policy.should_show_caret()
                && view.caret_visible
                && preedit_overlay.is_none()
            {
                match &view.selection {
                    Some(DocumentSelection::Text {
                        anchor,
                        focus,
                        affinity,
                        ..
                    }) if anchor == focus => {
                        view.flow.caret_rect_in_visible(focus, *affinity, &visible)
                    }
                    _ => None,
                }
            } else {
                None
            };

            let cursor_style = if view.hovered_pointer {
                CursorStyle::PointingHand
            } else if view.policy.selectable || view.policy.editable {
                CursorStyle::IBeam
            } else {
                CursorStyle::Arrow
            };

            let theme = view.active_theme(cx);
            let focus_handle = view.focus_handle.clone();

            (
                visible,
                selection_rects,
                caret_rect,
                preedit_overlay,
                view.scroll_x,
                view.scroll_y,
                view.flow.total_height(),
                horizontal,
                view.zoom,
                view.debug_overlay,
                view.policy.collapsible_headings,
                view.policy.editable,
                focused,
                focus_handle,
                theme,
                cursor_style,
            )
        });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        RichPrepaintState {
            visible_blocks,
            selection_rects,
            caret_rect,
            preedit_overlay,
            scroll_x,
            scroll_y,
            total_height,
            horizontal,
            zoom,
            debug_overlay,
            show_heading_chevrons,
            editable,
            focused,
            focus_handle,
            theme,
            cursor_style,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_cursor_style(prepaint.cursor_style, &prepaint.hitbox);
        if prepaint.editable {
            window.handle_input(
                &prepaint.focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
            if prepaint.focused {
                let anchor_key = prepaint.caret_rect.map(|rect| {
                    (
                        rect.x.round() as i32,
                        rect.y.round() as i32,
                        prepaint.scroll_x.round() as i32,
                        prepaint.scroll_y.round() as i32,
                    )
                });
                let moved = self.view.update(cx, |view, _| {
                    view.last_ime_anchor.replace(anchor_key) != Some(anchor_key)
                });
                if moved {
                    window.invalidate_character_coordinates();
                }
            }
        }

        let origin_x: f32 = bounds.origin.x.into();
        let origin_y: f32 = bounds.origin.y.into();
        let viewport_width: f32 = bounds.size.width.into();
        let viewport_height: f32 = bounds.size.height.into();
        let canvas_to_window = |canvas_x: f32, canvas_y: f32| -> Point<Pixels> {
            point(
                px(origin_x + canvas_x - prepaint.scroll_x),
                px(origin_y + canvas_y - prepaint.scroll_y),
            )
        };

        let theme = &prepaint.theme;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Layer 0: Fluid canvas backdrop or neutral desk + infinite-length continuous paper sheet.
            if let Some(sheet) = prepaint.horizontal.paper_sheet {
                window.paint_quad(fill(bounds, theme.desk_background));

                let sheet_height = (prepaint.total_height - sheet.top_y - sheet.desk_margin * 0.5)
                    .max(viewport_height - sheet.top_y + prepaint.scroll_y);
                let sheet_origin = canvas_to_window(sheet.x, sheet.top_y);
                let sheet_bounds =
                    Bounds::new(sheet_origin, size(px(sheet.width), px(sheet_height)));
                let sheet_radius = Corners::all(px(3.0));
                window.paint_drop_shadows(
                    sheet_bounds,
                    sheet_radius,
                    &[BoxShadow {
                        color: theme.paper_shadow,
                        offset: point(px(0.0), px(2.0)),
                        blur_radius: px(10.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }],
                );
                window.paint_quad(quad(
                    sheet_bounds,
                    sheet_radius,
                    theme.surface_background,
                    Edges::all(px(1.0)),
                    theme.paper_border,
                    BorderStyle::Solid,
                ));
            } else {
                window.paint_quad(fill(bounds, theme.surface_background));
            }

            // Layer 1: Block chrome (tables, blockquotes, code frames, rules, list markers, chevrons).
            for visible in &prepaint.visible_blocks {
                paint_block_chrome(
                    &visible.layout,
                    visible.x,
                    visible.y,
                    prepaint.zoom,
                    prepaint.show_heading_chevrons,
                    theme,
                    &canvas_to_window,
                    window,
                    cx,
                );
            }

            // Layer 2: Selection highlights.
            for rect in &prepaint.selection_rects {
                let top_left = canvas_to_window(rect.x, rect.y);
                window.paint_quad(fill(
                    Bounds::new(top_left, size(px(rect.width), px(rect.height))),
                    theme.selection_background,
                ));
            }

            // Layer 3: Inline backgrounds (highlight colors and inline code pills).
            for visible in &prepaint.visible_blocks {
                paint_block_inline_backgrounds(
                    &visible.layout,
                    visible.x,
                    visible.y,
                    prepaint.zoom,
                    theme,
                    &canvas_to_window,
                    window,
                );
            }

            // Layer 4: Foreground glyphs, underlines, strikethroughs, and inline objects.
            for visible in &prepaint.visible_blocks {
                paint_block_foreground(
                    &visible.layout,
                    visible.x,
                    visible.y,
                    prepaint.zoom,
                    prepaint.debug_overlay,
                    theme,
                    &canvas_to_window,
                    window,
                    cx,
                );
            }

            // Layer 5: Caret, ephemeral IME preedit overlay, and scrollbar thumbs.
            if let Some(preedit_overlay) = &prepaint.preedit_overlay {
                paint_ime_preedit(
                    preedit_overlay,
                    theme,
                    &canvas_to_window,
                    window,
                    cx,
                );
            } else if let Some(caret) = prepaint.caret_rect {
                let top_left = canvas_to_window(caret.x.round(), caret.y.round());
                window.paint_quad(fill(
                    Bounds::new(top_left, size(px(2.0), px(caret.height.max(12.0)))),
                    theme.caret,
                ));
            }

            if prepaint.total_height > viewport_height + 1.0 {
                let track_padding = 4.0;
                let thumb_width = 7.0;
                let track_height = (viewport_height - 2.0 * track_padding).max(20.0);
                let visible_ratio = (viewport_height / prepaint.total_height).clamp(0.05, 1.0);
                let thumb_height = (track_height * visible_ratio).max(28.0);
                let max_scroll = (prepaint.total_height - viewport_height).max(1.0);
                let progress = (prepaint.scroll_y / max_scroll).clamp(0.0, 1.0);
                let thumb_y = origin_y + track_padding + progress * (track_height - thumb_height);
                let thumb_bounds = Bounds::new(
                    point(px(origin_x + viewport_width - thumb_width - 4.0), px(thumb_y)),
                    size(px(thumb_width), px(thumb_height)),
                );
                window.paint_quad(quad(
                    thumb_bounds,
                    Corners::all(px(3.5)),
                    theme.scrollbar_thumb,
                    Edges::all(px(0.0)),
                    gpui_kit::transparent_black(),
                    BorderStyle::Solid,
                ));
            }

            if prepaint.horizontal.canvas_width > viewport_width + 1.0 {
                let track_padding = 4.0;
                let thumb_height = 7.0;
                let track_width = (viewport_width - 2.0 * track_padding - 12.0).max(20.0);
                let visible_ratio =
                    (viewport_width / prepaint.horizontal.canvas_width).clamp(0.05, 1.0);
                let thumb_width = (track_width * visible_ratio).max(28.0);
                let max_scroll_x = (prepaint.horizontal.canvas_width - viewport_width).max(1.0);
                let progress = (prepaint.scroll_x / max_scroll_x).clamp(0.0, 1.0);
                let thumb_x = origin_x + track_padding + progress * (track_width - thumb_width);
                let thumb_bounds = Bounds::new(
                    point(px(thumb_x), px(origin_y + viewport_height - thumb_height - 4.0)),
                    size(px(thumb_width), px(thumb_height)),
                );
                window.paint_quad(quad(
                    thumb_bounds,
                    Corners::all(px(3.5)),
                    theme.scrollbar_thumb,
                    Edges::all(px(0.0)),
                    gpui_kit::transparent_black(),
                    BorderStyle::Solid,
                ));
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_block_chrome(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    show_heading_chevrons: bool,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    match &layout.kind {
        BlockLayoutKind::Paragraph(_) => {}
        BlockLayoutKind::Heading {
            collapsed,
            chevron_rect,
            ..
        } => {
            if !show_heading_chevrons {
                return;
            }
            let symbol = if *collapsed { "▸" } else { "▾" };
            let glyph_origin = to_window(
                block_x + chevron_rect.x + pt_to_px(2.0, zoom),
                block_y + chevron_rect.y + pt_to_px(1.0, zoom),
            );
            paint_small_label(
                symbol,
                glyph_origin,
                pt_to_px(11.5, zoom),
                theme.muted_text,
                window,
                cx,
            );
        }
        BlockLayoutKind::ListItem {
            marker,
            checked,
            marker_origin,
            paragraph,
        } => {
            let first_line = paragraph.lines.first();
            if let Some(is_checked) = checked {
                // Draw vector checkbox quads so checklists do not depend on Unicode box glyph availability.
                let box_size = pt_to_px(10.5, zoom);
                let center_y = first_line
                    .map(|line| line.baseline_y - line.ascent * 0.38)
                    .unwrap_or(marker_origin.y - box_size * 0.5);
                let box_origin = to_window(
                    block_x + marker_origin.x,
                    block_y + center_y - box_size * 0.5,
                );
                let box_bounds = Bounds::new(box_origin, size(px(box_size), px(box_size)));
                let (background, border) = if *is_checked {
                    (theme.accent, theme.accent)
                } else {
                    (theme.surface_background, theme.muted_text)
                };
                window.paint_quad(quad(
                    box_bounds,
                    Corners::all(px(pt_to_px(2.0, zoom))),
                    background,
                    Edges::all(px(1.25)),
                    border,
                    BorderStyle::Solid,
                ));
                if *is_checked {
                    let inset = box_size * 0.28;
                    let inner_origin = to_window(
                        block_x + marker_origin.x + inset,
                        block_y + center_y - box_size * 0.5 + inset,
                    );
                    let inner_size = (box_size - 2.0 * inset).max(2.0);
                    window.paint_quad(quad(
                        Bounds::new(inner_origin, size(px(inner_size), px(inner_size))),
                        Corners::all(px(1.0)),
                        theme.accent_foreground,
                        Edges::all(px(0.0)),
                        gpui_kit::transparent_black(),
                        BorderStyle::Solid,
                    ));
                }
            } else {
                let line_top = first_line.map(|line| line.rect.y).unwrap_or(0.0);
                let origin = to_window(block_x + marker_origin.x, block_y + line_top + 1.0);
                paint_small_label(
                    marker,
                    origin,
                    pt_to_px(11.0, zoom),
                    theme.muted_text,
                    window,
                    cx,
                );
            }
        }
        BlockLayoutKind::Blockquote { bar_rect, children } => {
            let bar_origin = to_window(block_x + bar_rect.x, block_y + bar_rect.y);
            window.paint_quad(quad(
                Bounds::new(bar_origin, size(px(bar_rect.width), px(bar_rect.height))),
                Corners::all(px(1.5)),
                theme.blockquote_rail,
                Edges::all(px(0.0)),
                gpui_kit::transparent_black(),
                BorderStyle::Solid,
            ));
            for child in children {
                paint_block_chrome(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    zoom,
                    show_heading_chevrons,
                    theme,
                    to_window,
                    window,
                    cx,
                );
            }
        }
        BlockLayoutKind::CodeBlock {
            language,
            frame_rect,
            ..
        } => {
            let frame_origin = to_window(block_x + frame_rect.x, block_y + frame_rect.y);
            window.paint_quad(quad(
                Bounds::new(frame_origin, size(px(frame_rect.width), px(frame_rect.height))),
                Corners::all(px(pt_to_px(5.0, zoom))),
                theme.code_background,
                Edges::all(px(1.0)),
                theme.code_border,
                BorderStyle::Solid,
            ));
            if let Some(label) = language {
                let label_origin = to_window(
                    block_x + pt_to_px(10.0, zoom),
                    block_y + pt_to_px(5.0, zoom),
                );
                paint_small_label(
                    label,
                    label_origin,
                    pt_to_px(8.5, zoom),
                    theme.muted_text,
                    window,
                    cx,
                );
            }
        }
        BlockLayoutKind::Table(table) => {
            let border_color = theme.resolve_table_border(table.border_style.color);
            let border_width = pt_to_px(table.border_style.width_pt, zoom).max(1.0);
            for cell in &table.cells {
                let cell_origin = to_window(block_x + cell.rect.x, block_y + cell.rect.y);
                let cell_bounds =
                    Bounds::new(cell_origin, size(px(cell.rect.width), px(cell.rect.height)));
                let fill_color = cell
                    .background
                    .map(rgba_to_hsla)
                    .unwrap_or_else(|| {
                        if cell.is_header {
                            theme.table_header_background
                        } else {
                            theme.table_cell_background
                        }
                    });
                window.paint_quad(quad(
                    cell_bounds,
                    Corners::all(px(0.0)),
                    fill_color,
                    Edges::all(px(border_width)),
                    border_color,
                    BorderStyle::Solid,
                ));
                for child in &cell.children {
                    paint_block_chrome(
                        &child.layout,
                        block_x + cell.rect.x + child.x,
                        block_y + cell.rect.y + child.y,
                        zoom,
                        show_heading_chevrons,
                        theme,
                        to_window,
                        window,
                        cx,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { rule_y } => {
            let line_origin = to_window(block_x, block_y + *rule_y);
            window.paint_quad(fill(
                Bounds::new(line_origin, size(px(layout.width), px(1.0))),
                theme.rule_color,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_block_inline_backgrounds(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
) {
    match &layout.kind {
        BlockLayoutKind::Paragraph(paragraph)
        | BlockLayoutKind::Heading { paragraph, .. }
        | BlockLayoutKind::ListItem { paragraph, .. } => {
            paint_paragraph_backgrounds(paragraph, block_x, block_y, zoom, theme, to_window, window);
        }
        BlockLayoutKind::CodeBlock {
            content_origin,
            paragraph,
            ..
        } => {
            paint_paragraph_backgrounds(
                paragraph,
                block_x + content_origin.x,
                block_y + content_origin.y,
                zoom,
                theme,
                to_window,
                window,
            );
        }
        BlockLayoutKind::Blockquote { children, .. } => {
            for child in children {
                paint_block_inline_backgrounds(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    zoom,
                    theme,
                    to_window,
                    window,
                );
            }
        }
        BlockLayoutKind::Table(table) => {
            for cell in &table.cells {
                for child in &cell.children {
                    paint_block_inline_backgrounds(
                        &child.layout,
                        block_x + cell.rect.x + child.x,
                        block_y + cell.rect.y + child.y,
                        zoom,
                        theme,
                        to_window,
                        window,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { .. } => {}
    }
}

fn paint_paragraph_backgrounds(
    paragraph: &ParagraphLayout<GpuiGlyphBackend>,
    origin_x: f32,
    origin_y: f32,
    zoom: f32,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
) {
    for line in &paragraph.lines {
        for fragment in &line.fragments {
            if let LineFragment::Text(text_fragment) = fragment {
                let fragment_baseline = line.baseline_y - text_fragment.baseline_shift_px;
                let top = fragment_baseline - text_fragment.ascent - 1.0;
                let height = text_fragment.ascent + text_fragment.descent + 2.0;

                for decoration in &text_fragment.decorations {
                    if let Some(background) = decoration.background {
                        let start_x = text_fragment.x
                            + text_fragment.local_x_for_byte(decoration.doc_range.start);
                        let end_x = text_fragment.x
                            + text_fragment.local_x_for_byte(decoration.doc_range.end);
                        let width = (end_x - start_x).max(0.0);
                        if width > 0.0 {
                            let radius = if decoration.inline_code {
                                pt_to_px(3.0, zoom)
                            } else {
                                0.0
                            };
                            let pad_x = if decoration.inline_code { 2.0 } else { 0.0 };
                            let top_left =
                                to_window(origin_x + start_x - pad_x, origin_y + top);
                            window.paint_quad(quad(
                                Bounds::new(top_left, size(px(width + 2.0 * pad_x), px(height))),
                                Corners::all(px(radius)),
                                theme.resolve_inline_background(background, decoration.inline_code),
                                Edges::all(px(0.0)),
                                gpui_kit::transparent_black(),
                                BorderStyle::Solid,
                            ));
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_block_foreground(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    debug_overlay: bool,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    match &layout.kind {
        BlockLayoutKind::Paragraph(paragraph)
        | BlockLayoutKind::Heading { paragraph, .. }
        | BlockLayoutKind::ListItem { paragraph, .. } => {
            paint_paragraph_foreground(
                paragraph,
                block_x,
                block_y,
                zoom,
                debug_overlay,
                theme,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::CodeBlock {
            content_origin,
            paragraph,
            ..
        } => {
            paint_paragraph_foreground(
                paragraph,
                block_x + content_origin.x,
                block_y + content_origin.y,
                zoom,
                debug_overlay,
                theme,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::Blockquote { children, .. } => {
            for child in children {
                paint_block_foreground(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    zoom,
                    debug_overlay,
                    theme,
                    to_window,
                    window,
                    cx,
                );
            }
        }
        BlockLayoutKind::Table(table) => {
            for cell in &table.cells {
                for child in &cell.children {
                    paint_block_foreground(
                        &child.layout,
                        block_x + cell.rect.x + child.x,
                        block_y + cell.rect.y + child.y,
                        zoom,
                        debug_overlay,
                        theme,
                        to_window,
                        window,
                        cx,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_paragraph_foreground(
    paragraph: &ParagraphLayout<GpuiGlyphBackend>,
    origin_x: f32,
    origin_y: f32,
    zoom: f32,
    debug_overlay: bool,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    for line in &paragraph.lines {
        if debug_overlay {
            let line_origin = to_window(origin_x + line.rect.x, origin_y + line.rect.y);
            window.paint_quad(outline(
                Bounds::new(line_origin, size(px(line.rect.width), px(line.rect.height))),
                theme.debug_line_outline,
                BorderStyle::Solid,
            ));
            let base_origin = to_window(origin_x + line.rect.x, origin_y + line.baseline_y);
            window.paint_quad(fill(
                Bounds::new(base_origin, size(px(line.rect.width), px(1.0))),
                theme.debug_baseline,
            ));
        }

        for fragment in &line.fragments {
            match fragment {
                LineFragment::Text(text_fragment) => {
                    paint_text_fragment(
                        text_fragment,
                        origin_x,
                        origin_y + line.baseline_y,
                        theme,
                        to_window,
                        window,
                    );
                }
                LineFragment::InlineBox(inline_box) => {
                    let box_rect = line.inline_box_rect(inline_box);
                    paint_inline_box(
                        inline_box,
                        box_rect.translate(origin_x, origin_y),
                        zoom,
                        theme,
                        to_window,
                        window,
                        cx,
                    );
                }
                LineFragment::Tab(_) => {}
            }
        }
    }
}

fn paint_text_fragment(
    fragment: &TextFragment<GpuiGlyphBackend>,
    origin_x: f32,
    line_baseline_y: f32,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
) {
    let baseline_y = line_baseline_y - fragment.baseline_shift_px;
    let font_size = px(fragment.font_size_px);

    for run in &fragment.runs {
        let color = theme.resolve_text_color(run.color);
        for glyph in &run.glyphs {
            let glyph_origin = to_window(
                origin_x + fragment.x + glyph.x,
                baseline_y + glyph.y,
            );
            let paint_result = if glyph.is_emoji {
                window.paint_emoji(glyph_origin, run.font, glyph.glyph_id, font_size)
            } else {
                window.paint_glyph(glyph_origin, run.font, glyph.glyph_id, font_size, color)
            };
            if let Err(error) = paint_result {
                eprintln!("rich-gpui: failed to paint glyph: {error}");
            }
        }
    }

    let default_run_color = fragment
        .runs
        .first()
        .map(|run| theme.resolve_text_color(run.color))
        .unwrap_or(theme.text);

    for decoration in &fragment.decorations {
        let start_x = fragment.x + fragment.local_x_for_byte(decoration.doc_range.start);
        let end_x = fragment.x + fragment.local_x_for_byte(decoration.doc_range.end);
        let width = (end_x - start_x).max(0.0);
        if width <= 0.0 {
            continue;
        }
        let thickness = px((fragment.font_size_px * 0.065).clamp(1.0, 2.5));

        if let Some(underline) = decoration.underline {
            let (color, wavy) = match underline {
                UnderlineKind::Solid => (default_run_color, false),
                UnderlineKind::Wavy(wavy_color) => (theme.resolve_text_color(wavy_color), true),
            };
            let underline_y = baseline_y + (fragment.descent * 0.35).max(1.5);
            window.paint_underline(
                to_window(origin_x + start_x, underline_y),
                px(width),
                &UnderlineStyle {
                    thickness,
                    color: Some(color),
                    wavy,
                },
            );
        }

        if decoration.strikethrough {
            let strike_y = baseline_y - fragment.ascent * 0.34;
            window.paint_strikethrough(
                to_window(origin_x + start_x, strike_y),
                px(width),
                &StrikethroughStyle {
                    thickness,
                    color: Some(default_run_color),
                },
            );
        }
    }
}

fn paint_inline_box(
    inline_box: &InlineBoxFragment,
    canvas_rect: Rect,
    zoom: f32,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let top_left = to_window(canvas_rect.x, canvas_rect.y);
    let bounds = Bounds::new(top_left, size(px(canvas_rect.width), px(canvas_rect.height)));

    match &inline_box.object {
        InlineObject::SmartChip {
            kind,
            label,
            accent,
        } => {
            let chip_colors = theme.smart_chips.for_kind(*kind);
            let resolved_border = accent.map(rgba_to_hsla).unwrap_or(chip_colors.border);
            window.paint_quad(quad(
                bounds,
                Corners::all(px(canvas_rect.height * 0.5)),
                chip_colors.background,
                Edges::all(px(1.0)),
                resolved_border,
                BorderStyle::Solid,
            ));
            let text_origin = to_window(
                canvas_rect.x + pt_to_px(8.0, zoom),
                canvas_rect.y + pt_to_px(2.5, zoom),
            );
            paint_small_label(
                label,
                text_origin,
                pt_to_px(10.0, zoom),
                chip_colors.text,
                window,
                cx,
            );
        }
        InlineObject::Image { alt, .. } => {
            window.paint_quad(quad(
                bounds,
                Corners::all(px(pt_to_px(4.0, zoom))),
                theme.image_placeholder_background,
                Edges::all(px(1.0)),
                theme.image_placeholder_border,
                BorderStyle::Solid,
            ));
            let caption = if alt.is_empty() { "Image" } else { alt.as_str() };
            let text_origin = to_window(
                canvas_rect.x + pt_to_px(8.0, zoom),
                canvas_rect.y + (canvas_rect.height * 0.5 - pt_to_px(6.0, zoom)).max(4.0),
            );
            paint_small_label(
                caption,
                text_origin,
                pt_to_px(9.5, zoom),
                theme.muted_text,
                window,
                cx,
            );
        }
        InlineObject::FootnoteRef { index } => {
            let label = format!("[{index}]");
            paint_small_label(
                &label,
                top_left,
                pt_to_px(8.5, zoom),
                theme.link,
                window,
                cx,
            );
        }
    }
}

fn paint_ime_preedit(
    overlay: &PrepaintPreedit,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let text = overlay.preedit.text.as_str();
    if text.is_empty() {
        return;
    }
    let font_size_px = overlay.font_size_px;
    let run = TextRun {
        len: text.len(),
        font: gpui_kit::font(".SystemUIFont"),
        color: theme.text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(font_size_px),
        &[run],
        None,
    );
    let shaped_width: f32 = shaped.width.into();
    let height = overlay.anchor_rect.height.max(font_size_px * 1.25);
    let pad_x = 2.0;
    let origin = to_window(overlay.anchor_rect.x, overlay.anchor_rect.y);
    let pill_bounds = Bounds::new(
        point(origin.x - px(pad_x), origin.y),
        size(px(shaped_width + 2.0 * pad_x), px(height)),
    );
    window.paint_quad(quad(
        pill_bounds,
        Corners::all(px(2.0)),
        theme.surface_background,
        Edges::all(px(0.0)),
        gpui_kit::transparent_black(),
        BorderStyle::Solid,
    ));

    if let Err(error) = shaped.paint(
        origin,
        px(height),
        gpui_kit::TextAlign::Left,
        None,
        window,
        cx,
    ) {
        eprintln!("rich-gpui: failed to paint IME preedit: {error}");
    }

    let underline_y = overlay.anchor_rect.y + height - 2.0;
    window.paint_underline(
        to_window(overlay.anchor_rect.x, underline_y),
        px(shaped_width.max(4.0)),
        &UnderlineStyle {
            thickness: px(1.5),
            color: Some(theme.caret),
            wavy: false,
        },
    );

    let caret_byte = overlay.preedit.selected_range.end.min(text.len());
    let caret_offset_x: f32 = shaped.x_for_index(caret_byte).into();
    window.paint_quad(fill(
        Bounds::new(
            to_window(
                (overlay.anchor_rect.x + caret_offset_x).round(),
                overlay.anchor_rect.y.round(),
            ),
            size(px(2.0), px(height)),
        ),
        theme.caret,
    ));
}

fn paint_small_label(
    text: &str,
    origin: Point<Pixels>,
    font_size_px: f32,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    if text.is_empty() {
        return;
    }
    let run = TextRun {
        len: text.len(),
        font: gpui_kit::font(".SystemUIFont"),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(font_size_px),
        &[run],
        None,
    );
    if let Err(error) = shaped.paint(
        origin,
        px(font_size_px * 1.25),
        gpui_kit::TextAlign::Left,
        None,
        window,
        cx,
    ) {
        eprintln!("rich-gpui: failed to paint label: {error}");
    }
}
