//! Layer 1-5 painting helpers, custom node collectors, overlays, and popovers for [`RichDocumentElement`].

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_custom_chrome_or_fallback(
    block_id: BlockId,
    bounds: Rect,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    viewport_bounds: Bounds<Pixels>,
    chrome_custom_nodes: &mut HashMap<BlockId, PrepaintedCustomChrome>,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(custom_chrome) = chrome_custom_nodes.remove(&block_id) {
        match custom_chrome {
            PrepaintedCustomChrome::DirectPaint(node_bounds, paint_callback) => {
                paint_callback(node_bounds, theme, window, cx);
            }
            PrepaintedCustomChrome::Element(node_bounds, mut element) => {
                let clipped_mask = viewport_bounds.intersect(&node_bounds);
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: clipped_mask,
                    }),
                    |window| {
                        element.paint(window, cx);
                    },
                );
            }
        }
    } else {
        let origin = to_window(block_x + bounds.x, block_y + bounds.y);
        let node_bounds = Bounds::new(origin, size(px(bounds.width), px(bounds.height)));
        window.paint_quad(quad(
            node_bounds,
            Corners::all(px(pt_to_px(5.0, zoom))),
            theme.code_background,
            Edges::all(px(1.0)),
            theme.code_border,
            BorderStyle::Solid,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_table_chrome(
    table: &crate::layout::TableLayout<GpuiGlyphBackend>,
    origin_x: f32,
    origin_y: f32,
    zoom: f32,
    controls: &BlockChromeControls,
    viewport_bounds: Bounds<Pixels>,
    chrome_custom_nodes: &mut HashMap<BlockId, PrepaintedCustomChrome>,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let border_color = theme.resolve_table_border(table.border_style.color);
    let border_width = pt_to_px(table.border_style.width_pt, zoom).max(1.0);
    for cell in &table.cells {
        let cell_origin = to_window(origin_x + cell.rect.x, origin_y + cell.rect.y);
        let cell_bounds = Bounds::new(cell_origin, size(px(cell.rect.width), px(cell.rect.height)));
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
                origin_x + cell.rect.x + child.x,
                origin_y + cell.rect.y + child.y,
                zoom,
                controls,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_block_chrome(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    controls: &BlockChromeControls,
    viewport_bounds: Bounds<Pixels>,
    chrome_custom_nodes: &mut HashMap<BlockId, PrepaintedCustomChrome>,
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
            if !controls.heading_chevrons {
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
            checkbox_rect,
            paragraph,
        } => {
            let first_line = paragraph.lines.first();
            if let Some(is_checked) = checked {
                // Draw vector checkbox quads using the exact layout `checkbox_rect` so painting and hit-testing match.
                let rect = checkbox_rect.unwrap_or_else(|| {
                    let box_size = pt_to_px(10.5, zoom);
                    let center_y = first_line
                        .map(|line| line.baseline_y - line.ascent * 0.38)
                        .unwrap_or(marker_origin.y - box_size * 0.5);
                    Rect::new(marker_origin.x, center_y - box_size * 0.5, box_size, box_size)
                });
                let box_origin = to_window(block_x + rect.x, block_y + rect.y);
                let box_bounds = Bounds::new(box_origin, size(px(rect.width), px(rect.height)));
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
                    let inset = rect.width * 0.28;
                    let inner_origin =
                        to_window(block_x + rect.x + inset, block_y + rect.y + inset);
                    let inner_size = (rect.width - 2.0 * inset).max(2.0);
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
                    controls,
                    viewport_bounds,
                    chrome_custom_nodes,
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
            copy_button_rect,
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
                    block_y + pt_to_px(5.5, zoom),
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
            if !controls.code_copy {
                return;
            }
            let is_copied = controls.copied_code_block == Some(layout.id);
            let btn_origin = to_window(block_x + copy_button_rect.x, block_y + copy_button_rect.y);
            let btn_bounds = Bounds::new(
                btn_origin,
                size(px(copy_button_rect.width), px(copy_button_rect.height)),
            );
            let (btn_bg, btn_border, btn_fg, btn_text) = if is_copied {
                (theme.accent.alpha(0.14), theme.accent, theme.accent, "Copied")
            } else {
                (
                    theme.surface_background,
                    theme.code_border,
                    theme.muted_text,
                    "Copy",
                )
            };
            window.paint_quad(quad(
                btn_bounds,
                Corners::all(px(pt_to_px(3.5, zoom))),
                btn_bg,
                Edges::all(px(1.0)),
                btn_border,
                BorderStyle::Solid,
            ));
            let text_pad_x = if is_copied {
                pt_to_px(5.5, zoom)
            } else {
                pt_to_px(10.0, zoom)
            };
            let btn_text_origin = to_window(
                block_x + copy_button_rect.x + text_pad_x,
                block_y + copy_button_rect.y + pt_to_px(2.0, zoom),
            );
            paint_small_label(
                btn_text,
                btn_text_origin,
                pt_to_px(8.0, zoom),
                btn_fg,
                window,
                cx,
            );
        }
        BlockLayoutKind::Table(table) => {
            paint_table_chrome(
                table,
                block_x,
                block_y,
                zoom,
                controls,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::HorizontalRule { rule_y } => {
            let line_origin = to_window(block_x, block_y + *rule_y);
            window.paint_quad(fill(
                Bounds::new(line_origin, size(px(layout.width), px(1.0))),
                theme.rule_color,
            ));
        }
        BlockLayoutKind::CustomText { bounds, .. } => {
            paint_custom_chrome_or_fallback(
                layout.id,
                *bounds,
                block_x,
                block_y,
                zoom,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::CustomContainer {
            bounds, children, ..
        } => {
            paint_custom_chrome_or_fallback(
                layout.id,
                *bounds,
                block_x,
                block_y,
                zoom,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
            for child in children {
                paint_block_chrome(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    zoom,
                    controls,
                    viewport_bounds,
                    chrome_custom_nodes,
                    theme,
                    to_window,
                    window,
                    cx,
                );
            }
        }
        BlockLayoutKind::CustomGrid {
            bounds,
            content_origin,
            table,
            ..
        } => {
            paint_custom_chrome_or_fallback(
                layout.id,
                *bounds,
                block_x,
                block_y,
                zoom,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
            paint_table_chrome(
                table,
                block_x + content_origin.x,
                block_y + content_origin.y,
                zoom,
                controls,
                viewport_bounds,
                chrome_custom_nodes,
                theme,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::CustomLeaf { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_block_inline_backgrounds(
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
        }
        | BlockLayoutKind::CustomText {
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
        BlockLayoutKind::Blockquote { children, .. }
        | BlockLayoutKind::CustomContainer { children, .. } => {
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
        BlockLayoutKind::CustomGrid {
            content_origin,
            table,
            ..
        } => {
            for cell in &table.cells {
                for child in &cell.children {
                    paint_block_inline_backgrounds(
                        &child.layout,
                        block_x + content_origin.x + cell.rect.x + child.x,
                        block_y + content_origin.y + cell.rect.y + child.y,
                        zoom,
                        theme,
                        to_window,
                        window,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { .. } | BlockLayoutKind::CustomLeaf { .. } => {}
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
                        let raw_a = text_fragment.x
                            + text_fragment.local_x_for_byte(decoration.doc_range.start);
                        let raw_b = text_fragment.x
                            + text_fragment.local_x_for_byte(decoration.doc_range.end);
                        let start_x = raw_a.min(raw_b);
                        let end_x = raw_a.max(raw_b);
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
pub(super) fn paint_block_foreground(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    zoom: f32,
    debug_overlay: bool,
    theme: &RichTheme,
    block_inline_decorations: &HashMap<BlockId, Vec<InlineDecoration>>,
    image_assets: &HashMap<ImageAssetId, LoadedAsset>,
    handled_custom_blocks: &HashSet<BlockId>,
    handled_custom_inlines: &HashSet<(BlockId, InlineObjectId)>,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let extra_decorations = block_inline_decorations
        .get(&layout.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    match &layout.kind {
        BlockLayoutKind::Paragraph(paragraph)
        | BlockLayoutKind::Heading { paragraph, .. }
        | BlockLayoutKind::ListItem { paragraph, .. } => {
            paint_paragraph_foreground(
                layout.id,
                paragraph,
                block_x,
                block_y,
                zoom,
                debug_overlay,
                theme,
                extra_decorations,
                image_assets,
                handled_custom_inlines,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::CodeBlock {
            content_origin,
            paragraph,
            ..
        }
        | BlockLayoutKind::CustomText {
            content_origin,
            paragraph,
            ..
        } => {
            paint_paragraph_foreground(
                layout.id,
                paragraph,
                block_x + content_origin.x,
                block_y + content_origin.y,
                zoom,
                debug_overlay,
                theme,
                extra_decorations,
                image_assets,
                handled_custom_inlines,
                to_window,
                window,
                cx,
            );
        }
        BlockLayoutKind::Blockquote { children, .. }
        | BlockLayoutKind::CustomContainer { children, .. } => {
            for child in children {
                paint_block_foreground(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    zoom,
                    debug_overlay,
                    theme,
                    block_inline_decorations,
                    image_assets,
                    handled_custom_blocks,
                    handled_custom_inlines,
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
                        block_inline_decorations,
                        image_assets,
                        handled_custom_blocks,
                        handled_custom_inlines,
                        to_window,
                        window,
                        cx,
                    );
                }
            }
        }
        BlockLayoutKind::CustomGrid {
            content_origin,
            table,
            ..
        } => {
            for cell in &table.cells {
                for child in &cell.children {
                    paint_block_foreground(
                        &child.layout,
                        block_x + content_origin.x + cell.rect.x + child.x,
                        block_y + content_origin.y + cell.rect.y + child.y,
                        zoom,
                        debug_overlay,
                        theme,
                        block_inline_decorations,
                        image_assets,
                        handled_custom_blocks,
                        handled_custom_inlines,
                        to_window,
                        window,
                        cx,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { .. } => {}
        BlockLayoutKind::CustomLeaf { kind, bounds, .. } => {
            if !handled_custom_blocks.contains(&layout.id) {
                let origin = to_window(block_x + bounds.x, block_y + bounds.y);
                let node_bounds =
                    Bounds::new(origin, size(px(bounds.width), px(bounds.height)));
                window.paint_quad(quad(
                    node_bounds,
                    Corners::all(px(pt_to_px(5.0, zoom))),
                    theme.code_background,
                    Edges::all(px(1.0)),
                    theme.code_border,
                    BorderStyle::Solid,
                ));
                let label = format!("Custom block: {kind}");
                let label_origin = to_window(
                    block_x + bounds.x + pt_to_px(10.0, zoom),
                    block_y + bounds.y + (bounds.height * 0.5 - pt_to_px(6.0, zoom)).max(4.0),
                );
                paint_small_label(
                    &label,
                    label_origin,
                    pt_to_px(10.0, zoom),
                    theme.muted_text,
                    window,
                    cx,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_paragraph_foreground(
    block_id: BlockId,
    paragraph: &ParagraphLayout<GpuiGlyphBackend>,
    origin_x: f32,
    origin_y: f32,
    zoom: f32,
    debug_overlay: bool,
    theme: &RichTheme,
    extra_decorations: &[InlineDecoration],
    image_assets: &HashMap<ImageAssetId, LoadedAsset>,
    handled_custom_inlines: &HashSet<(BlockId, InlineObjectId)>,
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
                        extra_decorations,
                        to_window,
                        window,
                    );
                }
                LineFragment::InlineBox(inline_box) => {
                    if matches!(inline_box.object, InlineObject::Custom { .. })
                        && handled_custom_inlines.contains(&(block_id, inline_box.object_id))
                    {
                        continue;
                    }
                    let box_rect = line.inline_box_rect(inline_box);
                    paint_inline_box(
                        inline_box,
                        box_rect.translate(origin_x, origin_y),
                        zoom,
                        theme,
                        image_assets,
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

fn foreground_override_for_byte(
    extra_decorations: &[InlineDecoration],
    byte_index: usize,
) -> Option<RgbaColor> {
    let mut best: Option<(i32, RgbaColor)> = None;
    for decoration in extra_decorations {
        if let Some(color) = decoration.foreground
            && decoration.range().contains(&byte_index)
            && best.is_none_or(|(priority, _)| decoration.priority >= priority)
        {
            best = Some((decoration.priority, color));
        }
    }
    best.map(|(_, color)| color)
}

fn paint_text_fragment(
    fragment: &TextFragment<GpuiGlyphBackend>,
    origin_x: f32,
    line_baseline_y: f32,
    theme: &RichTheme,
    extra_decorations: &[InlineDecoration],
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
) {
    let baseline_y = line_baseline_y - fragment.baseline_shift_px;
    let font_size = px(fragment.font_size_px);
    let has_foreground_overrides = extra_decorations
        .iter()
        .any(|decoration| decoration.foreground.is_some());

    for run in &fragment.runs {
        let base_color = theme.resolve_text_color(run.color);
        for glyph in &run.glyphs {
            let color = if has_foreground_overrides {
                foreground_override_for_byte(extra_decorations, glyph.byte_index)
                    .map(|override_color| theme.resolve_text_color(override_color))
                    .unwrap_or(base_color)
            } else {
                base_color
            };
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
    let thickness = px((fragment.font_size_px * 0.065).clamp(1.0, 2.5));

    for decoration in &fragment.decorations {
        let raw_a = fragment.x + fragment.local_x_for_byte(decoration.doc_range.start);
        let raw_b = fragment.x + fragment.local_x_for_byte(decoration.doc_range.end);
        let start_x = raw_a.min(raw_b);
        let end_x = raw_a.max(raw_b);
        let width = (end_x - start_x).max(0.0);
        if width <= 0.0 {
            continue;
        }

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

    // Ephemeral Layer 4 underlines and strikethroughs from DecorationProviders (e.g. spellcheck).
    for decoration in extra_decorations {
        if decoration.underline.is_none() && !decoration.strikethrough {
            continue;
        }
        let range = decoration.range();
        let overlap_start = range.start.max(fragment.doc_range.start);
        let overlap_end = range.end.min(fragment.doc_range.end);
        if overlap_start >= overlap_end {
            continue;
        }
        let raw_a = fragment.x + fragment.local_x_for_byte(overlap_start);
        let raw_b = fragment.x + fragment.local_x_for_byte(overlap_end);
        let start_x = raw_a.min(raw_b);
        let end_x = raw_a.max(raw_b);
        let width = (end_x - start_x).max(0.0);
        if width <= 0.0 {
            continue;
        }

        if let Some(underline) = decoration.underline {
            let (color, wavy) = match underline {
                UnderlineKind::Solid => (
                    decoration
                        .foreground
                        .map(|fg| theme.resolve_text_color(fg))
                        .unwrap_or(default_run_color),
                    false,
                ),
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

#[allow(clippy::too_many_arguments)]
fn paint_inline_box(
    inline_box: &InlineBoxFragment,
    canvas_rect: Rect,
    zoom: f32,
    theme: &RichTheme,
    image_assets: &HashMap<ImageAssetId, LoadedAsset>,
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
        InlineObject::Image { asset_id, alt, .. } => {
            let radius = Corners::all(px(pt_to_px(4.0, zoom)));
            if let Some(LoadedAsset::Ready(render_image)) = image_assets.get(asset_id) {
                if let Err(error) = window.paint_image(
                    bounds,
                    bounds,
                    radius,
                    Arc::clone(render_image),
                    0,
                    false,
                ) {
                    eprintln!("rich-gpui: failed to paint inline image '{asset_id}': {error}");
                }
                return;
            }

            window.paint_quad(quad(
                bounds,
                radius,
                theme.image_placeholder_background,
                Edges::all(px(1.0)),
                theme.image_placeholder_border,
                BorderStyle::Solid,
            ));
            let caption = match image_assets.get(asset_id) {
                Some(LoadedAsset::Loading) => "Loading image...",
                Some(LoadedAsset::Failed(_)) if !alt.is_empty() => alt.as_str(),
                Some(LoadedAsset::Failed(ImageAssetError::NotFound { .. })) => "Image not found",
                Some(LoadedAsset::Failed(ImageAssetError::Decode { .. })) => "Image could not decode",
                Some(LoadedAsset::Failed(ImageAssetError::Provider { .. })) => "Image failed to load",
                _ if !alt.is_empty() => alt.as_str(),
                _ => "Image",
            };
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
        InlineObject::Custom { kind, .. } => {
            window.paint_quad(quad(
                bounds,
                Corners::all(px(pt_to_px(4.0, zoom))),
                theme.inline_code_background,
                Edges::all(px(1.0)),
                theme.code_border,
                BorderStyle::Solid,
            ));
            let text_origin = to_window(
                canvas_rect.x + pt_to_px(6.0, zoom),
                canvas_rect.y + pt_to_px(2.5, zoom),
            );
            paint_small_label(
                kind,
                text_origin,
                pt_to_px(9.5, zoom),
                theme.muted_text,
                window,
                cx,
            );
        }
    }
}

pub(super) fn collect_pending_custom_nodes(
    layout: &BlockLayout<GpuiGlyphBackend>,
    block_x: f32,
    block_y: f32,
    extensions: &ExtensionRegistry,
    out_blocks: &mut Vec<PendingCustomBlock>,
    out_inlines: &mut Vec<PendingCustomInline>,
    out_images: &mut HashSet<ImageAssetId>,
) {
    match &layout.kind {
        BlockLayoutKind::CustomLeaf { kind, data, bounds } => {
            if let Some(renderer) = extensions.get_block(kind) {
                out_blocks.push(PendingCustomBlock {
                    block_id: layout.id,
                    shape: BlockShapeKind::Leaf,
                    kind: kind.clone(),
                    data: data.clone(),
                    canvas_rect: bounds.translate(block_x, block_y),
                    chrome: Insets::ZERO,
                    is_chrome: false,
                    renderer,
                });
            }
        }
        BlockLayoutKind::CustomText {
            kind,
            data,
            bounds,
            chrome,
            content_origin,
            paragraph,
        } => {
            if let Some(renderer) = extensions.get_block(kind) {
                out_blocks.push(PendingCustomBlock {
                    block_id: layout.id,
                    shape: BlockShapeKind::Text,
                    kind: kind.clone(),
                    data: data.clone(),
                    canvas_rect: bounds.translate(block_x, block_y),
                    chrome: *chrome,
                    is_chrome: true,
                    renderer,
                });
            }
            collect_custom_inlines_in_paragraph(
                layout.id,
                paragraph,
                block_x + content_origin.x,
                block_y + content_origin.y,
                extensions,
                out_inlines,
                out_images,
            );
        }
        BlockLayoutKind::CustomContainer {
            kind,
            data,
            bounds,
            chrome,
            children,
        } => {
            if let Some(renderer) = extensions.get_block(kind) {
                out_blocks.push(PendingCustomBlock {
                    block_id: layout.id,
                    shape: BlockShapeKind::Container,
                    kind: kind.clone(),
                    data: data.clone(),
                    canvas_rect: bounds.translate(block_x, block_y),
                    chrome: *chrome,
                    is_chrome: true,
                    renderer,
                });
            }
            for child in children {
                collect_pending_custom_nodes(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    extensions,
                    out_blocks,
                    out_inlines,
                    out_images,
                );
            }
        }
        BlockLayoutKind::CustomGrid {
            kind,
            data,
            bounds,
            chrome,
            content_origin,
            table,
        } => {
            if let Some(renderer) = extensions.get_block(kind) {
                out_blocks.push(PendingCustomBlock {
                    block_id: layout.id,
                    shape: BlockShapeKind::Grid,
                    kind: kind.clone(),
                    data: data.clone(),
                    canvas_rect: bounds.translate(block_x, block_y),
                    chrome: *chrome,
                    is_chrome: true,
                    renderer,
                });
            }
            for cell in &table.cells {
                for child in &cell.children {
                    collect_pending_custom_nodes(
                        &child.layout,
                        block_x + content_origin.x + cell.rect.x + child.x,
                        block_y + content_origin.y + cell.rect.y + child.y,
                        extensions,
                        out_blocks,
                        out_inlines,
                        out_images,
                    );
                }
            }
        }
        BlockLayoutKind::Paragraph(paragraph)
        | BlockLayoutKind::Heading { paragraph, .. }
        | BlockLayoutKind::ListItem { paragraph, .. } => {
            collect_custom_inlines_in_paragraph(
                layout.id,
                paragraph,
                block_x,
                block_y,
                extensions,
                out_inlines,
                out_images,
            );
        }
        BlockLayoutKind::CodeBlock {
            content_origin,
            paragraph,
            ..
        } => {
            collect_custom_inlines_in_paragraph(
                layout.id,
                paragraph,
                block_x + content_origin.x,
                block_y + content_origin.y,
                extensions,
                out_inlines,
                out_images,
            );
        }
        BlockLayoutKind::Blockquote { children, .. } => {
            for child in children {
                collect_pending_custom_nodes(
                    &child.layout,
                    block_x + child.x,
                    block_y + child.y,
                    extensions,
                    out_blocks,
                    out_inlines,
                    out_images,
                );
            }
        }
        BlockLayoutKind::Table(table) => {
            for cell in &table.cells {
                for child in &cell.children {
                    collect_pending_custom_nodes(
                        &child.layout,
                        block_x + cell.rect.x + child.x,
                        block_y + cell.rect.y + child.y,
                        extensions,
                        out_blocks,
                        out_inlines,
                        out_images,
                    );
                }
            }
        }
        BlockLayoutKind::HorizontalRule { .. } => {}
    }
}

fn collect_custom_inlines_in_paragraph(
    block_id: BlockId,
    paragraph: &ParagraphLayout<GpuiGlyphBackend>,
    origin_x: f32,
    origin_y: f32,
    extensions: &ExtensionRegistry,
    out_inlines: &mut Vec<PendingCustomInline>,
    out_images: &mut HashSet<ImageAssetId>,
) {
    for line in &paragraph.lines {
        for fragment in &line.fragments {
            if let LineFragment::InlineBox(inline_box) = fragment {
                match &inline_box.object {
                    InlineObject::Custom { kind, data } => {
                        if let Some(renderer) = extensions.get_inline(kind) {
                            let canvas_rect = line
                                .inline_box_rect(inline_box)
                                .translate(origin_x, origin_y);
                            out_inlines.push(PendingCustomInline {
                                block_id,
                                object_id: inline_box.object_id,
                                kind: kind.clone(),
                                data: data.clone(),
                                canvas_rect,
                                renderer,
                            });
                        }
                    }
                    InlineObject::Image { asset_id, .. } if !asset_id.is_empty() => {
                        out_images.insert(asset_id.clone());
                    }
                    _ => {}
                }
            }
        }
    }
}

pub(super) fn paint_selected_image_handles(
    selected: &SelectedImageOverlay,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let active_rect = selected.preview_rect.unwrap_or(selected.canvas_rect);
    let top_left = to_window(active_rect.x, active_rect.y);
    let bounds = Bounds::new(top_left, size(px(active_rect.width), px(active_rect.height)));

    if selected.preview_rect.is_some() {
        window.paint_quad(quad(
            bounds,
            Corners::all(px(3.0)),
            theme.accent.alpha(0.08),
            Edges::all(px(1.5)),
            theme.accent,
            BorderStyle::Dashed,
        ));
    } else {
        window.paint_quad(quad(
            bounds,
            Corners::all(px(3.0)),
            gpui_kit::transparent_black(),
            Edges::all(px(1.5)),
            theme.accent,
            BorderStyle::Solid,
        ));
    }

    for (_, handle_rect) in image_resize_handle_rects(active_rect) {
        let origin = to_window(handle_rect.x, handle_rect.y);
        window.paint_quad(quad(
            Bounds::new(origin, size(px(handle_rect.width), px(handle_rect.height))),
            Corners::all(px(2.0)),
            theme.surface_background,
            Edges::all(px(1.5)),
            theme.accent,
            BorderStyle::Solid,
        ));
    }

    if let Some(label) = &selected.preview_label {
        let badge_w = (label.len() as f32) * 6.5 + 14.0;
        let badge_h = 20.0;
        let badge_origin = to_window(
            active_rect.x + (active_rect.width - badge_w) * 0.5,
            active_rect.bottom() + 6.0,
        );
        window.paint_quad(quad(
            Bounds::new(badge_origin, size(px(badge_w), px(badge_h))),
            Corners::all(px(4.0)),
            theme.accent,
            Edges::all(px(0.0)),
            gpui_kit::transparent_black(),
            BorderStyle::Solid,
        ));
        let text_origin = to_window(
            active_rect.x + (active_rect.width - badge_w) * 0.5 + 7.0,
            active_rect.bottom() + 9.0,
        );
        paint_small_label(label, text_origin, 11.0, theme.accent_foreground, window, cx);
    }
}

pub(super) fn build_smart_chip_popover_element(
    popover: &ActiveSmartChipPopover,
    theme: &RichTheme,
    view: Entity<RichDocumentView>,
    width_px: f32,
    height_px: f32,
) -> AnyElement {
    let chip_colors = theme.smart_chips.for_kind(popover.kind);
    let (badge_label, subtitle) = match popover.kind {
        ChipKind::Person => ("Person", "People mention"),
        ChipKind::Date => ("Date", "Calendar date chip"),
        ChipKind::Document => ("Document", "Linked workspace document"),
        ChipKind::Tag => ("Tag", "Status or category tag"),
    };
    let label_for_copy = popover.label.clone();
    let view_for_copy = view.clone();
    let view_for_close = view;
    let accent_border = popover
        .accent
        .map(rgba_to_hsla)
        .unwrap_or(chip_colors.border);

    div()
        .id("rich-smart-chip-popover")
        .w(px(width_px))
        .h(px(height_px))
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(theme.surface_background)
        .border_1()
        .border_color(theme.paper_border)
        .shadow(vec![BoxShadow {
            color: theme.paper_shadow,
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(12.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .flex()
        .flex_col()
        .justify_between()
        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded(px(99.0))
                                .bg(chip_colors.background)
                                .border_1()
                                .border_color(accent_border)
                                .text_size(px(10.0))
                                .text_color(chip_colors.text)
                                .child(badge_label),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(popover.label.clone()),
                        ),
                )
                .child(
                    div()
                        .id("rich-chip-popover-close")
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .text_size(px(11.0))
                        .text_color(theme.muted_text)
                        .cursor_pointer()
                        .on_click(move |_, _window, cx| {
                            view_for_close.update(cx, |this, cx| {
                                this.dismiss_chip_popover(cx);
                            });
                        })
                        .child("×"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_text)
                        .child(subtitle),
                )
                .child(
                    div()
                        .id("rich-chip-popover-copy")
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(theme.code_background)
                        .border_1()
                        .border_color(theme.code_border)
                        .text_size(px(10.5))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .on_click(move |_, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                label_for_copy.clone(),
                            ));
                            view_for_copy.update(cx, |this, cx| {
                                this.dismiss_chip_popover(cx);
                            });
                        })
                        .child("Copy"),
                ),
        )
        .into_any_element()
}

pub(super) fn paint_decoration_overlay(
    overlay: &PrepaintDecorationOverlay,
    zoom: f32,
    theme: &RichTheme,
    to_window: &impl Fn(f32, f32) -> Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    match overlay {
        PrepaintDecorationOverlay::GhostText {
            anchor_rect,
            font_size_px,
            text,
            color,
        } => {
            if text.is_empty() {
                return;
            }
            let ghost_color = color
                .map(|rgba| theme.resolve_text_color(rgba))
                .unwrap_or_else(|| theme.muted_text.alpha(0.65));
            let origin = to_window(anchor_rect.x, anchor_rect.y);
            paint_small_label(text, origin, *font_size_px, ghost_color, window, cx);
        }
        PrepaintDecorationOverlay::RemoteCaret {
            caret_rect,
            color,
            label,
        } => {
            let caret_hsla = rgba_to_hsla(*color);
            let top_left = to_window(caret_rect.x.round(), caret_rect.y.round());
            window.paint_quad(fill(
                Bounds::new(top_left, size(px(2.0), px(caret_rect.height.max(12.0)))),
                caret_hsla,
            ));
            if let Some(name) = label
                && !name.is_empty()
            {
                let badge_font_px = pt_to_px(8.5, zoom).max(9.0);
                let run = TextRun {
                    len: name.len(),
                    font: gpui_kit::font(".SystemUIFont"),
                    color: theme.inverse_text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(name.clone()),
                    px(badge_font_px),
                    &[run],
                    None,
                );
                let text_width: f32 = shaped.width.into();
                let badge_height = badge_font_px + 4.0;
                let badge_width = text_width + 8.0;
                let badge_origin = to_window(
                    caret_rect.x.round(),
                    (caret_rect.y - badge_height).max(0.0).round(),
                );
                window.paint_quad(quad(
                    Bounds::new(badge_origin, size(px(badge_width), px(badge_height))),
                    Corners::all(px(3.0)),
                    caret_hsla,
                    Edges::all(px(0.0)),
                    gpui_kit::transparent_black(),
                    BorderStyle::Solid,
                ));
                let text_origin = to_window(
                    caret_rect.x.round() + 4.0,
                    (caret_rect.y - badge_height + 1.0).max(0.0).round(),
                );
                if let Err(error) = shaped.paint(
                    text_origin,
                    px(badge_height),
                    gpui_kit::TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    eprintln!("rich-gpui: failed to paint remote caret badge: {error}");
                }
            }
        }
    }
}

pub(super) fn paint_ime_preedit(
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

pub(super) fn paint_small_label(
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
