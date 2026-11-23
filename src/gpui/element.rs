//! Custom GPUI [`Element`] implementing virtualized non-paginated 6-layer rich text painting.

use crate::gpui::asset::{ImageAssetError, LoadedAsset};
use crate::gpui::extension::{
    BlockCallbackResult, CustomRenderOutput, DirectPaintCallback, ErasedBlockRenderContext,
    ErasedBlockRenderer, ErasedInlineRenderContext, ErasedInlineRenderer, ExtensionDiagnostic,
    ExtensionNodeTarget, ExtensionRegistry, InlineCallbackResult,
};
use crate::gpui::shaper::{GpuiGlyphBackend, GpuiShaper, rgba_to_hsla};
use crate::gpui::theme::RichTheme;
use crate::gpui::view::{
    ActiveSmartChipPopover, ImageResizeCorner, ImePreedit, RichDocumentView,
    image_resize_handle_rects,
};
use crate::layout::{
    BlockLayout, BlockLayoutKind, HorizontalMetrics, InlineBoxFragment, Insets, LineFragment,
    ParagraphLayout, Rect, TextFragment, VisibleBlock, collect_visible_leaf_paragraphs_in_y_range,
    pt_to_px, resolve_horizontal_metrics,
};
use crate::model::{
    Affinity, BlockId, BlockShapeKind, ChipKind, CustomPayload, DecorationContext,
    DecorationWriter, DocumentSelection, ImageAssetId, InlineDecoration, InlineObject,
    InlineObjectId, OverlayDecorationKind, RgbaColor, SharedStr, UnderlineKind,
};
use gpui_kit::{
    AnyElement, App, AvailableSpace, BorderStyle, Bounds, BoxShadow, ClipboardItem, ContentMask,
    Corners, CursorStyle, Edges, Element, ElementId, ElementInputHandler, Entity, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, Length, MouseButton, ParentElement, Pixels, Point, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Style, Styled, TextRun, UnderlineStyle, Window,
    div, fill, outline, point, px, quad, size,
};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

mod paint;

use paint::{
    build_smart_chip_popover_element, collect_pending_custom_nodes, paint_block_chrome,
    paint_block_foreground, paint_block_inline_backgrounds, paint_decoration_overlay,
    paint_ime_preedit, paint_selected_image_handles,
};

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

/// Resolved Layer 5 overlay from a [`crate::model::DecorationProvider`].
enum PrepaintDecorationOverlay {
    GhostText {
        anchor_rect: Rect,
        font_size_px: f32,
        text: String,
        color: Option<RgbaColor>,
    },
    RemoteCaret {
        caret_rect: Rect,
        color: RgbaColor,
        label: Option<String>,
    },
}

struct PendingCustomBlock {
    block_id: BlockId,
    shape: BlockShapeKind,
    kind: SharedStr,
    data: CustomPayload,
    canvas_rect: Rect,
    chrome: Insets,
    is_chrome: bool,
    renderer: Rc<dyn ErasedBlockRenderer>,
}

struct PendingCustomInline {
    block_id: BlockId,
    object_id: InlineObjectId,
    kind: SharedStr,
    data: CustomPayload,
    canvas_rect: Rect,
    renderer: Rc<dyn ErasedInlineRenderer>,
}

enum PrepaintedCustomChrome {
    Element(Bounds<Pixels>, AnyElement),
    DirectPaint(Bounds<Pixels>, DirectPaintCallback),
}

struct SelectedImageOverlay {
    canvas_rect: Rect,
    preview_rect: Option<Rect>,
    preview_label: Option<String>,
}

struct ColumnDividerGuide {
    canvas_x: f32,
    table_y: f32,
    table_height: f32,
    dragging: bool,
}

struct BlockChromeControls {
    heading_chevrons: bool,
    code_copy: bool,
    copied_code_block: Option<BlockId>,
}

/// Prepaint payload passed from `Element::prepaint` into `Element::paint`.
pub struct RichPrepaintState {
    visible_blocks: Vec<VisibleBlock<GpuiGlyphBackend>>,
    selection_rects: Vec<Rect>,
    decoration_highlights: Vec<(Rect, RgbaColor)>,
    block_inline_decorations: HashMap<BlockId, Vec<InlineDecoration>>,
    decoration_overlays: Vec<PrepaintDecorationOverlay>,
    image_assets: HashMap<ImageAssetId, LoadedAsset>,
    selected_image_overlay: Option<SelectedImageOverlay>,
    column_divider_guide: Option<ColumnDividerGuide>,
    popover_element: Option<AnyElement>,
    handled_custom_blocks: HashSet<BlockId>,
    handled_custom_inlines: HashSet<(BlockId, InlineObjectId)>,
    chrome_custom_nodes: HashMap<BlockId, PrepaintedCustomChrome>,
    hosted_custom_elements: Vec<(Bounds<Pixels>, AnyElement)>,
    direct_custom_paints: Vec<(Bounds<Pixels>, DirectPaintCallback)>,
    caret_rect: Option<Rect>,
    preedit_overlay: Option<PrepaintPreedit>,
    scroll_x: f32,
    scroll_y: f32,
    total_height: f32,
    horizontal: HorizontalMetrics,
    zoom: f32,
    debug_overlay: bool,
    chrome_controls: BlockChromeControls,
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
        style.size.width = Length::Definite(gpui_kit::relative(1.0));
        style.size.height = Length::Definite(gpui_kit::relative(1.0));
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
            decoration_highlights,
            block_inline_decorations,
            decoration_overlays,
            image_assets,
            selected_image_overlay,
            column_divider_guide,
            pending_chip_popover,
            pending_custom_blocks,
            pending_custom_inlines,
            caret_rect,
            preedit_overlay,
            scroll_x,
            scroll_y,
            total_height,
            horizontal,
            zoom,
            debug_overlay,
            chrome_controls,
            editable,
            focused,
            focus_handle,
            theme,
            cursor_style,
        ) = self.view.update(cx, |view, cx| {
            let policy = view.policy();
            view.last_viewport_bounds = bounds;
            let focused = view.focus_handle.is_focused(window);
            view.sync_focus_state(focused, cx);

            let horizontal =
                resolve_horizontal_metrics(viewport_width, &view.document.config, view.zoom);
            let max_scroll_x = (horizontal.canvas_width - viewport_width).max(0.0);
            view.scroll_x = view.scroll_x.clamp(0.0, max_scroll_x);

            let pending_reveal = view.pending_reveal.take();
            let anchor = if pending_reveal.is_none() {
                view.flow.capture_anchor(view.scroll_y)
            } else {
                None
            };
            let mut shaper = GpuiShaper::with_extensions(window, &view.extensions)
                .with_components(&view.document.components)
                .with_optional_font_resolver(view.font_resolver.as_deref());
            if let Some(pos) = pending_reveal {
                if viewport_width <= 1.0 || viewport_height <= 1.0 {
                    view.pending_reveal = Some(pos);
                } else if let Some(rect) = view.flow.measure_caret_rect(
                    &view.document,
                    &pos,
                    Affinity::Downstream,
                    viewport_width,
                    view.zoom,
                    &mut shaper,
                ) {
                    let max_y = (view.flow.total_height() - viewport_height).max(0.0);
                    if rect.x < view.scroll_x {
                        view.scroll_x = rect.x.clamp(0.0, max_scroll_x);
                    } else if rect.right() > view.scroll_x + viewport_width {
                        view.scroll_x = (rect.right() - viewport_width).clamp(0.0, max_scroll_x);
                    }
                    if rect.y < view.scroll_y {
                        view.scroll_y = rect.y.clamp(0.0, max_y);
                    } else if rect.bottom() > view.scroll_y + viewport_height {
                        view.scroll_y = (rect.bottom() - viewport_height).clamp(0.0, max_y);
                    }
                }
            }
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

            let mut decoration_highlights = Vec::new();
            let mut block_inline_decorations: HashMap<BlockId, Vec<InlineDecoration>> =
                HashMap::new();
            let mut decoration_overlays = Vec::new();

            if !view.decoration_providers.is_empty() || !view.navigation.search.matches.is_empty() {
                let min_y = view.scroll_y - OVERSCAN_PX;
                let max_y = view.scroll_y + viewport_height + OVERSCAN_PX;
                let leaf_paragraphs =
                    collect_visible_leaf_paragraphs_in_y_range(&visible, min_y, max_y);
                for leaf in leaf_paragraphs {
                    let Some(block) = view.document.find_block(leaf.block_id) else {
                        continue;
                    };
                    let mut writer = DecorationWriter::new(block);
                    view.navigation
                        .write_decorations(leaf.block_id, &mut writer);
                    let context = DecorationContext {
                        document: &view.document,
                        block,
                        selection: view.selection.as_ref(),
                    };
                    for (_, provider) in &view.decoration_providers {
                        provider.collect_decorations(&context, &mut writer);
                    }
                    let mut aggregated = writer.finish();
                    if aggregated.is_empty() {
                        continue;
                    }

                    aggregated
                        .inline
                        .sort_by_key(|decoration| decoration.priority);

                    for inline_decoration in &aggregated.inline {
                        if let Some(background) = inline_decoration.background {
                            for rect in leaf
                                .paragraph
                                .selection_rects(inline_decoration.range(), false)
                            {
                                decoration_highlights.push((
                                    rect.translate(leaf.origin_x, leaf.origin_y),
                                    background,
                                ));
                            }
                        }
                    }

                    let foreground_or_line: Vec<InlineDecoration> = aggregated
                        .inline
                        .into_iter()
                        .filter(|decoration| {
                            decoration.foreground.is_some()
                                || decoration.underline.is_some()
                                || decoration.strikethrough
                        })
                        .collect();
                    if !foreground_or_line.is_empty() {
                        block_inline_decorations.insert(leaf.block_id, foreground_or_line);
                    }

                    let block_font_pt = block
                        .style
                        .resolved_base_text(
                            &view.document.stylesheet,
                            &view.document.config.default_style,
                        )
                        .font_size_pt;
                    let font_size_px = pt_to_px(block_font_pt, view.zoom).max(10.0);

                    for overlay in aggregated.overlays {
                        let offset = overlay.offset().get();
                        let affinity = overlay.affinity();
                        match overlay.kind() {
                            OverlayDecorationKind::GhostText { text, color } => {
                                let anchor_rect = leaf
                                    .paragraph
                                    .caret_rect(offset, affinity)
                                    .translate(leaf.origin_x, leaf.origin_y);
                                decoration_overlays.push(PrepaintDecorationOverlay::GhostText {
                                    anchor_rect,
                                    font_size_px,
                                    text: text.clone(),
                                    color: *color,
                                });
                            }
                            OverlayDecorationKind::RemoteCaret { color, label } => {
                                let caret_rect = leaf
                                    .paragraph
                                    .caret_rect(offset, affinity)
                                    .translate(leaf.origin_x, leaf.origin_y);
                                decoration_overlays.push(PrepaintDecorationOverlay::RemoteCaret {
                                    caret_rect,
                                    color: *color,
                                    label: label.clone(),
                                });
                            }
                        }
                    }
                }
            }

            let mut pending_custom_blocks = Vec::new();
            let mut pending_custom_inlines = Vec::new();
            let mut visible_image_ids: HashSet<ImageAssetId> = HashSet::new();
            for entry in &visible {
                collect_pending_custom_nodes(
                    &entry.layout,
                    entry.x,
                    entry.y,
                    &view.extensions,
                    &mut pending_custom_blocks,
                    &mut pending_custom_inlines,
                    &mut visible_image_ids,
                );
            }

            let mut image_assets = HashMap::new();
            for asset_id in visible_image_ids {
                if let Some(loaded) = view.resolve_or_request_image(&asset_id, cx) {
                    image_assets.insert(asset_id, loaded);
                }
            }

            let selected_image_overlay = if policy.can_resize_images() {
                view.selected_image.and_then(|selected| {
                    let canvas_rect = view.flow.inline_object_rect_in_visible(
                        selected.block_id,
                        selected.object_id,
                        &visible,
                    )?;
                    let (preview_rect, preview_label) = match view.image_resize_drag.as_ref() {
                        Some(drag)
                            if drag.block_id == selected.block_id
                                && drag.object_id == selected.object_id =>
                        {
                            let new_w = pt_to_px(drag.current_width_pt, view.zoom);
                            let new_h = pt_to_px(drag.current_height_pt, view.zoom);
                            let rect = match drag.corner {
                                ImageResizeCorner::TopLeft => Rect::new(
                                    canvas_rect.right() - new_w,
                                    canvas_rect.bottom() - new_h,
                                    new_w,
                                    new_h,
                                ),
                                ImageResizeCorner::TopRight => Rect::new(
                                    canvas_rect.x,
                                    canvas_rect.bottom() - new_h,
                                    new_w,
                                    new_h,
                                ),
                                ImageResizeCorner::BottomLeft => Rect::new(
                                    canvas_rect.right() - new_w,
                                    canvas_rect.y,
                                    new_w,
                                    new_h,
                                ),
                                ImageResizeCorner::BottomRight => {
                                    Rect::new(canvas_rect.x, canvas_rect.y, new_w, new_h)
                                }
                            };
                            let label = format!(
                                "{} × {} pt",
                                drag.current_width_pt.round() as i32,
                                drag.current_height_pt.round() as i32
                            );
                            (Some(rect), Some(label))
                        }
                        _ => (None, None),
                    };
                    Some(SelectedImageOverlay {
                        canvas_rect,
                        preview_rect,
                        preview_label,
                    })
                })
            } else {
                None
            };

            let column_divider_guide = if let Some(drag) = view.table_column_drag.as_ref() {
                Some(ColumnDividerGuide {
                    canvas_x: drag.current_divider_x,
                    table_y: drag.table_y,
                    table_height: drag.table_height,
                    dragging: true,
                })
            } else {
                view.hovered_column_divider
                    .as_ref()
                    .map(|hit| ColumnDividerGuide {
                        canvas_x: hit.divider_x,
                        table_y: hit.table_y,
                        table_height: hit.table_height,
                        dragging: false,
                    })
            };

            let pending_chip_popover = view.active_chip_popover.as_ref().and_then(|popover| {
                let anchor_rect = view.flow.inline_object_rect_in_visible(
                    popover.block_id,
                    popover.object_id,
                    &visible,
                )?;
                Some((popover.clone(), anchor_rect))
            });

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
                && policy.should_show_caret()
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

            let active_handle = view
                .image_resize_drag
                .as_ref()
                .map(|drag| drag.corner)
                .or(view.hovered_image_handle);
            let cursor_style = if let Some(corner) = active_handle {
                match corner {
                    ImageResizeCorner::TopLeft | ImageResizeCorner::BottomRight => {
                        CursorStyle::ResizeUpRightDownLeft
                    }
                    ImageResizeCorner::TopRight | ImageResizeCorner::BottomLeft => {
                        CursorStyle::ResizeUpLeftDownRight
                    }
                }
            } else if view.table_column_drag.is_some() || view.hovered_column_divider.is_some() {
                CursorStyle::ResizeColumn
            } else if view.hovered_pointer {
                CursorStyle::PointingHand
            } else if policy.can_select() || policy.editing.enabled {
                CursorStyle::IBeam
            } else {
                CursorStyle::Arrow
            };

            let theme = view.active_theme(cx);
            let focus_handle = view.focus_handle.clone();

            (
                visible,
                selection_rects,
                decoration_highlights,
                block_inline_decorations,
                decoration_overlays,
                image_assets,
                selected_image_overlay,
                column_divider_guide,
                pending_chip_popover,
                pending_custom_blocks,
                pending_custom_inlines,
                caret_rect,
                preedit_overlay,
                view.scroll_x,
                view.scroll_y,
                view.flow.total_height(),
                horizontal,
                view.zoom,
                view.debug_overlay,
                BlockChromeControls {
                    heading_chevrons: policy.can_collapse_headings(),
                    code_copy: policy.can_copy_code(),
                    copied_code_block: view.copied_code_block,
                },
                policy.editing.enabled,
                focused,
                focus_handle,
                theme,
                cursor_style,
            )
        });

        let origin_x: f32 = bounds.origin.x.into();
        let origin_y: f32 = bounds.origin.y.into();
        let rect_to_bounds = |canvas_rect: Rect| -> Bounds<Pixels> {
            Bounds::new(
                point(
                    px(origin_x + canvas_rect.x - scroll_x),
                    px(origin_y + canvas_rect.y - scroll_y),
                ),
                size(px(canvas_rect.width), px(canvas_rect.height)),
            )
        };

        let mut handled_custom_blocks = HashSet::new();
        let mut handled_custom_inlines = HashSet::new();
        let mut chrome_custom_nodes = HashMap::new();
        let mut hosted_custom_elements = Vec::new();
        let mut direct_custom_paints = Vec::new();
        let mut foreground_custom_blocks = Vec::new();
        let mut frame_diagnostics = Vec::new();

        // Prepaint Layer 1 custom chrome elements before the document hitbox so clicks inside
        // custom text, container, and grid blocks hit the document canvas for text selection and editing.
        for custom_block in pending_custom_blocks {
            if !custom_block.is_chrome {
                foreground_custom_blocks.push(custom_block);
                continue;
            }
            let node_bounds = rect_to_bounds(custom_block.canvas_rect);
            match custom_block.renderer.render(
                ErasedBlockRenderContext {
                    shape: custom_block.shape,
                    block_id: custom_block.block_id,
                    kind: &custom_block.kind,
                    data: &custom_block.data,
                    bounds: node_bounds,
                    chrome: custom_block.chrome,
                    zoom,
                    theme: &theme,
                },
                window,
                cx,
            ) {
                BlockCallbackResult::Ok(Some(output)) => {
                    handled_custom_blocks.insert(custom_block.block_id);
                    let prepainted = match output {
                        CustomRenderOutput::Element(mut element) => {
                            let available = size(
                                AvailableSpace::Definite(node_bounds.size.width),
                                AvailableSpace::Definite(node_bounds.size.height),
                            );
                            element.prepaint_as_root(node_bounds.origin, available, window, cx);
                            PrepaintedCustomChrome::Element(node_bounds, element)
                        }
                        CustomRenderOutput::DirectPaint(callback) => {
                            PrepaintedCustomChrome::DirectPaint(node_bounds, callback)
                        }
                    };
                    chrome_custom_nodes.insert(custom_block.block_id, prepainted);
                }
                BlockCallbackResult::Ok(None) => {}
                BlockCallbackResult::Invalid(detail) => {
                    frame_diagnostics.push(ExtensionDiagnostic {
                        target: ExtensionNodeTarget::Block {
                            block_id: custom_block.block_id,
                        },
                        kind: custom_block.kind,
                        detail,
                    });
                }
            }
        }

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        // Prepaint hosted GPUI `AnyElement` leaf and inline children after the document hitbox so
        // interactive widgets receive pointer events above the document canvas.
        for custom_block in foreground_custom_blocks {
            let node_bounds = rect_to_bounds(custom_block.canvas_rect);
            match custom_block.renderer.render(
                ErasedBlockRenderContext {
                    shape: custom_block.shape,
                    block_id: custom_block.block_id,
                    kind: &custom_block.kind,
                    data: &custom_block.data,
                    bounds: node_bounds,
                    chrome: custom_block.chrome,
                    zoom,
                    theme: &theme,
                },
                window,
                cx,
            ) {
                BlockCallbackResult::Ok(Some(output)) => {
                    handled_custom_blocks.insert(custom_block.block_id);
                    match output {
                        CustomRenderOutput::Element(mut element) => {
                            let available = size(
                                AvailableSpace::Definite(node_bounds.size.width),
                                AvailableSpace::Definite(node_bounds.size.height),
                            );
                            element.prepaint_as_root(node_bounds.origin, available, window, cx);
                            hosted_custom_elements.push((node_bounds, element));
                        }
                        CustomRenderOutput::DirectPaint(callback) => {
                            direct_custom_paints.push((node_bounds, callback));
                        }
                    }
                }
                BlockCallbackResult::Ok(None) => {}
                BlockCallbackResult::Invalid(detail) => {
                    frame_diagnostics.push(ExtensionDiagnostic {
                        target: ExtensionNodeTarget::Block {
                            block_id: custom_block.block_id,
                        },
                        kind: custom_block.kind,
                        detail,
                    });
                }
            }
        }

        for custom_inline in pending_custom_inlines {
            let node_bounds = rect_to_bounds(custom_inline.canvas_rect);
            match custom_inline.renderer.render(
                ErasedInlineRenderContext {
                    block_id: custom_inline.block_id,
                    object_id: custom_inline.object_id,
                    kind: &custom_inline.kind,
                    data: &custom_inline.data,
                    bounds: node_bounds,
                    zoom,
                    theme: &theme,
                },
                window,
                cx,
            ) {
                InlineCallbackResult::Ok(Some(output)) => {
                    handled_custom_inlines
                        .insert((custom_inline.block_id, custom_inline.object_id));
                    match output {
                        CustomRenderOutput::Element(mut element) => {
                            let available = size(
                                AvailableSpace::Definite(node_bounds.size.width),
                                AvailableSpace::Definite(node_bounds.size.height),
                            );
                            element.prepaint_as_root(node_bounds.origin, available, window, cx);
                            hosted_custom_elements.push((node_bounds, element));
                        }
                        CustomRenderOutput::DirectPaint(callback) => {
                            direct_custom_paints.push((node_bounds, callback));
                        }
                    }
                }
                InlineCallbackResult::Ok(None) => {}
                InlineCallbackResult::Invalid(detail) => {
                    frame_diagnostics.push(ExtensionDiagnostic {
                        target: ExtensionNodeTarget::Inline {
                            block_id: custom_inline.block_id,
                            object_id: custom_inline.object_id,
                        },
                        kind: custom_inline.kind,
                        detail,
                    });
                }
            }
        }

        self.view.update(cx, |view, _| {
            view.extension_diagnostics = frame_diagnostics;
        });

        let popover_element = pending_chip_popover.map(|(popover, anchor_rect)| {
            let popover_w = 224.0f32;
            let dropdown = crate::gpui::view::components::build_dropdown_options(
                &popover,
                self.view.clone(),
                &theme,
                cx,
            );
            let popover_h = dropdown
                .as_ref()
                .map_or(84.0, |(_, count)| (14.0 + *count as f32 * 29.0).min(240.0));
            let anchor_bounds = rect_to_bounds(anchor_rect);
            let anchor_left: f32 = anchor_bounds.origin.x.into();
            let anchor_bottom: f32 = (anchor_bounds.origin.y + anchor_bounds.size.height).into();
            let anchor_top: f32 = anchor_bounds.origin.y.into();

            let clamped_x = anchor_left.clamp(
                origin_x + 8.0,
                (origin_x + viewport_width - popover_w - 8.0).max(origin_x + 8.0),
            );
            let pop_y = if anchor_bottom + 6.0 + popover_h <= origin_y + viewport_height - 8.0 {
                anchor_bottom + 6.0
            } else {
                (anchor_top - popover_h - 6.0).max(origin_y + 8.0)
            };
            let mut element = dropdown.map_or_else(
                || {
                    build_smart_chip_popover_element(
                        &popover,
                        &theme,
                        self.view.clone(),
                        popover_w,
                        popover_h,
                    )
                },
                |(menu, _)| menu,
            );
            let available = size(
                AvailableSpace::Definite(px(popover_w)),
                AvailableSpace::Definite(px(popover_h)),
            );
            element.prepaint_as_root(point(px(clamped_x), px(pop_y)), available, window, cx);
            element
        });

        RichPrepaintState {
            visible_blocks,
            selection_rects,
            decoration_highlights,
            block_inline_decorations,
            decoration_overlays,
            image_assets,
            selected_image_overlay,
            column_divider_guide,
            popover_element,
            handled_custom_blocks,
            handled_custom_inlines,
            chrome_custom_nodes,
            hosted_custom_elements,
            direct_custom_paints,
            caret_rect,
            preedit_overlay,
            scroll_x,
            scroll_y,
            total_height,
            horizontal,
            zoom,
            debug_overlay,
            chrome_controls,
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

                let sheet_height = (prepaint.total_height - sheet.top_y - sheet.desk_margin)
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

            // Layer 1: Block chrome (tables, blockquotes, code frames, rules, list markers, chevrons, custom chrome).
            for visible in &prepaint.visible_blocks {
                paint_block_chrome(
                    &visible.layout,
                    visible.x,
                    visible.y,
                    prepaint.zoom,
                    &prepaint.chrome_controls,
                    bounds,
                    &mut prepaint.chrome_custom_nodes,
                    theme,
                    &canvas_to_window,
                    window,
                    cx,
                );
            }

            // Layer 2: Ephemeral decoration highlights (find/comments) and active selection highlights.
            for (rect, color) in &prepaint.decoration_highlights {
                let top_left = canvas_to_window(rect.x, rect.y);
                window.paint_quad(fill(
                    Bounds::new(top_left, size(px(rect.width), px(rect.height))),
                    theme.resolve_inline_background(*color, false),
                ));
            }
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

            // Layer 4: Foreground glyphs, decorations, inline objects, and custom node extensions.
            for visible in &prepaint.visible_blocks {
                paint_block_foreground(
                    &visible.layout,
                    visible.x,
                    visible.y,
                    prepaint.zoom,
                    prepaint.debug_overlay,
                    theme,
                    &prepaint.block_inline_decorations,
                    &prepaint.image_assets,
                    &prepaint.handled_custom_blocks,
                    &prepaint.handled_custom_inlines,
                    &canvas_to_window,
                    window,
                    cx,
                );
            }
            for (node_bounds, paint_callback) in prepaint.direct_custom_paints.drain(..) {
                paint_callback(node_bounds, theme, window, cx);
            }
            for (node_bounds, mut element) in prepaint.hosted_custom_elements.drain(..) {
                let clipped_mask = bounds.intersect(&node_bounds);
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: clipped_mask,
                    }),
                    |window| {
                        element.paint(window, cx);
                    },
                );
            }

            // Layer 5: Ephemeral decoration overlays (ghost text, remote carets), image handles,
            // table column resize guide, active caret/IME preedit, smart chip popovers, and scrollbars.
            for overlay in &prepaint.decoration_overlays {
                paint_decoration_overlay(
                    overlay,
                    prepaint.zoom,
                    theme,
                    &canvas_to_window,
                    window,
                    cx,
                );
            }
            if let Some(guide) = &prepaint.column_divider_guide {
                let guide_w = if guide.dragging { 2.5 } else { 2.0 };
                let guide_color = if guide.dragging {
                    theme.accent
                } else {
                    theme.accent.alpha(0.65)
                };
                let guide_origin = canvas_to_window(guide.canvas_x - guide_w * 0.5, guide.table_y);
                window.paint_quad(fill(
                    Bounds::new(guide_origin, size(px(guide_w), px(guide.table_height))),
                    guide_color,
                ));
            }
            if let Some(selected_img) = &prepaint.selected_image_overlay {
                paint_selected_image_handles(selected_img, theme, &canvas_to_window, window, cx);
            }
            if let Some(preedit_overlay) = &prepaint.preedit_overlay {
                paint_ime_preedit(preedit_overlay, theme, &canvas_to_window, window, cx);
            } else if let Some(caret) = prepaint.caret_rect {
                let top_left = canvas_to_window(caret.x.round(), caret.y.round());
                window.paint_quad(fill(
                    Bounds::new(top_left, size(px(2.0), px(caret.height.max(12.0)))),
                    theme.caret,
                ));
            }
            if let Some(mut popover) = prepaint.popover_element.take() {
                popover.paint(window, cx);
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
                    point(
                        px(origin_x + viewport_width - thumb_width - 4.0),
                        px(thumb_y),
                    ),
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
                    point(
                        px(thumb_x),
                        px(origin_y + viewport_height - thumb_height - 4.0),
                    ),
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
