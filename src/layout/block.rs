use crate::layout::geometry::{Insets, Point, Rect, pt_to_px};
use crate::layout::line::{HitResult, ParagraphLayout, layout_paragraph_for_block};
use crate::layout::shaper::{
    GlyphBackend, TextShaper, is_valid_custom_insets, is_valid_custom_size,
};
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockShapeKind, CellVAlign, ColumnSpec, CustomPayload, Document,
    DocumentLayoutConfig, ListKind, NumberStyle, RgbaColor, SharedStr, StyleSheet, TableBlock,
    TableBorderStyle, block_kinds,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedChildBlock<B: GlyphBackend> {
    pub x: f32,
    pub y: f32,
    pub layout: Arc<BlockLayout<B>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableCellLayout<B: GlyphBackend> {
    pub cell_id: BlockId,
    pub row: usize,
    /// Visual grid column index (accounting for preceding `colspan`/`rowspan`).
    pub col: usize,
    /// Storage index inside `TableRow::cells` (used for `PathSegment::TableCell`).
    pub cell_index: usize,
    pub colspan: u16,
    pub rowspan: u16,
    pub is_header: bool,
    pub rect: Rect,
    pub background: Option<RgbaColor>,
    pub children: Vec<PlacedChildBlock<B>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableLayout<B: GlyphBackend> {
    pub total_width: f32,
    pub total_height: f32,
    pub border_style: TableBorderStyle,
    pub column_widths: Vec<f32>,
    pub row_heights: Vec<f32>,
    pub cells: Vec<TableCellLayout<B>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockLayoutKind<B: GlyphBackend> {
    Paragraph(ParagraphLayout<B>),
    Heading {
        level: u8,
        collapsed: bool,
        chevron_rect: Rect,
        paragraph: ParagraphLayout<B>,
    },
    ListItem {
        marker: String,
        checked: Option<bool>,
        marker_origin: Point,
        checkbox_rect: Option<Rect>,
        paragraph: ParagraphLayout<B>,
    },
    Blockquote {
        bar_rect: Rect,
        children: Vec<PlacedChildBlock<B>>,
    },
    CodeBlock {
        language: Option<String>,
        frame_rect: Rect,
        copy_button_rect: Rect,
        content_origin: Point,
        paragraph: ParagraphLayout<B>,
    },
    Table(TableLayout<B>),
    HorizontalRule {
        rule_y: f32,
    },
    CustomText {
        kind: SharedStr,
        data: CustomPayload,
        bounds: Rect,
        chrome: Insets,
        content_origin: Point,
        paragraph: ParagraphLayout<B>,
    },
    CustomContainer {
        kind: SharedStr,
        data: CustomPayload,
        bounds: Rect,
        chrome: Insets,
        children: Vec<PlacedChildBlock<B>>,
    },
    CustomGrid {
        kind: SharedStr,
        data: CustomPayload,
        bounds: Rect,
        chrome: Insets,
        content_origin: Point,
        table: TableLayout<B>,
    },
    CustomLeaf {
        kind: SharedStr,
        data: CustomPayload,
        bounds: Rect,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockLayout<B: GlyphBackend> {
    pub id: BlockId,
    pub version: u64,
    pub width: f32,
    pub height: f32,
    pub space_before_px: f32,
    pub space_after_px: f32,
    pub kind: BlockLayoutKind<B>,
}

impl<B: GlyphBackend> BlockLayout<B> {
    pub fn paragraph(&self) -> Option<&ParagraphLayout<B>> {
        match &self.kind {
            BlockLayoutKind::Paragraph(p)
            | BlockLayoutKind::Heading { paragraph: p, .. }
            | BlockLayoutKind::ListItem { paragraph: p, .. }
            | BlockLayoutKind::CodeBlock { paragraph: p, .. }
            | BlockLayoutKind::CustomText { paragraph: p, .. } => Some(p),
            BlockLayoutKind::Blockquote { .. }
            | BlockLayoutKind::CustomContainer { .. }
            | BlockLayoutKind::Table(_)
            | BlockLayoutKind::CustomGrid { .. }
            | BlockLayoutKind::HorizontalRule { .. }
            | BlockLayoutKind::CustomLeaf { .. } => None,
        }
    }

    pub fn children(&self) -> Option<&[PlacedChildBlock<B>]> {
        match &self.kind {
            BlockLayoutKind::Blockquote { children, .. }
            | BlockLayoutKind::CustomContainer { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn table_layout(&self) -> Option<&TableLayout<B>> {
        match &self.kind {
            BlockLayoutKind::Table(table) | BlockLayoutKind::CustomGrid { table, .. } => {
                Some(table)
            }
            _ => None,
        }
    }

    pub fn content_origin(&self) -> Point {
        match &self.kind {
            BlockLayoutKind::CodeBlock { content_origin, .. }
            | BlockLayoutKind::CustomText { content_origin, .. }
            | BlockLayoutKind::CustomGrid { content_origin, .. } => *content_origin,
            _ => Point::new(0.0, 0.0),
        }
    }

    pub fn hit_test_local(&self, local_point: Point) -> Option<HitResult> {
        let origin = self.content_origin();
        let para = self.paragraph()?;
        Some(para.hit_test(Point::new(
            local_point.x - origin.x,
            local_point.y - origin.y,
        )))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    id: BlockId,
    version: u64,
    width_q: i32,
    zoom_q: i32,
}

/// Incremental layout cache keyed by `(BlockId, version, quantized_width, quantized_zoom)`.
pub struct LayoutCache<B: GlyphBackend> {
    entries: HashMap<CacheKey, Arc<BlockLayout<B>>>,
    latest_by_id: HashMap<BlockId, CacheKey>,
    ordered_markers: HashMap<BlockId, Arc<str>>,
    footnote_numbers: Arc<HashMap<crate::model::AnchorId, u32>>,
}

impl<B: GlyphBackend> Default for LayoutCache<B> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            latest_by_id: HashMap::new(),
            ordered_markers: HashMap::new(),
            footnote_numbers: Arc::default(),
        }
    }
}

impl<B: GlyphBackend> LayoutCache<B> {
    pub fn invalidate_block(&mut self, id: BlockId) {
        if let Some(old_key) = self.latest_by_id.remove(&id) {
            self.entries.remove(&old_key);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.latest_by_id.clear();
        self.ordered_markers.clear();
        self.footnote_numbers = Arc::default();
    }

    pub(crate) fn sync_ordered_markers(&mut self, document: &Document) {
        let next = crate::model::ordered_list_markers(document);
        let next_notes = if document.anchors.footnotes().next().is_some() {
            document.anchors.footnote_number_map(document)
        } else {
            HashMap::new()
        };
        let changed_notes = (next_notes != *self.footnote_numbers).then_some(&next_notes);
        fn invalidate_changed<B: GlyphBackend>(
            cache: &mut LayoutCache<B>,
            next: &HashMap<BlockId, String>,
            changed_notes: Option<&HashMap<crate::model::AnchorId, u32>>,
            block: &BlockNode,
        ) -> bool {
            let mut changed = cache.ordered_markers.get(&block.id).map(AsRef::as_ref)
                != next.get(&block.id).map(String::as_str);
            if let Some(numbers) = changed_notes
                && let Some(inline) = block.kind.inline_content()
            {
                changed |= inline.objects.iter().any(|entry| {
                    crate::model::footnote_reference_id(&entry.object)
                        .is_some_and(|id| cache.footnote_numbers.get(&id) != numbers.get(&id))
                });
            }
            match &block.kind {
                BlockKind::Container { children, .. } => {
                    for child in children {
                        changed |= invalidate_changed(cache, next, changed_notes, child);
                    }
                }
                BlockKind::Grid { table, .. } => {
                    for child in table
                        .rows
                        .iter()
                        .flat_map(|row| &row.cells)
                        .flat_map(|cell| &cell.blocks)
                    {
                        changed |= invalidate_changed(cache, next, changed_notes, child);
                    }
                }
                _ => {}
            }
            if changed {
                cache.invalidate_block(block.id);
            }
            changed
        }
        for block in &document.blocks {
            invalidate_changed(self, &next, changed_notes, block);
        }
        if changed_notes.is_some() {
            self.footnote_numbers = Arc::new(next_notes);
        }
        self.ordered_markers = next
            .into_iter()
            .map(|(id, marker)| {
                let marker = self
                    .ordered_markers
                    .remove(&id)
                    .filter(|previous| previous.as_ref() == marker)
                    .unwrap_or_else(|| Arc::from(marker));
                (id, marker)
            })
            .collect();
    }

    pub fn get_cached(
        &self,
        id: BlockId,
        version: u64,
        max_width: f32,
        zoom: f32,
    ) -> Option<Arc<BlockLayout<B>>> {
        let key = CacheKey {
            id,
            version,
            width_q: (max_width * 4.0).round() as i32,
            zoom_q: (zoom * 100.0).round() as i32,
        };
        self.entries.get(&key).cloned()
    }

    /// Returns the most recently cached layout for `id`, regardless of current viewport/zoom key.
    pub fn get_latest(&self, id: BlockId) -> Option<Arc<BlockLayout<B>>> {
        let key = self.latest_by_id.get(&id)?;
        self.entries.get(key).cloned()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_or_layout<S: TextShaper<Backend = B>>(
        &mut self,
        block: &BlockNode,
        list_marker: Option<&str>,
        sheet: &StyleSheet,
        config: &DocumentLayoutConfig,
        max_width: f32,
        zoom: f32,
        shaper: &mut S,
    ) -> Arc<BlockLayout<B>> {
        let key = CacheKey {
            id: block.id,
            version: block.version,
            width_q: (max_width * 4.0).round() as i32,
            zoom_q: (zoom * 100.0).round() as i32,
        };
        if let Some(cached) = self.entries.get(&key) {
            return Arc::clone(cached);
        }

        shaper.set_footnote_numbers(&self.footnote_numbers);
        let ordered_marker = self.ordered_markers.get(&block.id).cloned();
        let layout = Arc::new(layout_block(
            block,
            ordered_marker.as_deref().or(list_marker),
            sheet,
            config,
            max_width,
            zoom,
            self,
            shaper,
        ));

        if let Some(prev_key) = self.latest_by_id.insert(block.id, key)
            && prev_key != key
        {
            self.entries.remove(&prev_key);
        }
        self.entries.insert(key, Arc::clone(&layout));
        layout
    }
}

#[allow(clippy::too_many_arguments)]
pub fn layout_block<S: TextShaper>(
    block: &BlockNode,
    list_marker: Option<&str>,
    sheet: &StyleSheet,
    config: &DocumentLayoutConfig,
    max_width: f32,
    zoom: f32,
    cache: &mut LayoutCache<S::Backend>,
    shaper: &mut S,
) -> BlockLayout<S::Backend> {
    let space_before_px = pt_to_px(block.style.resolved_space_before(sheet), zoom);
    let space_after_px = pt_to_px(block.style.resolved_space_after(sheet), zoom);

    match &block.kind {
        BlockKind::Text {
            kind,
            data,
            content,
        } => match kind.as_ref() {
            block_kinds::PARAGRAPH => {
                let para = layout_paragraph_for_block(
                    Some(block.id),
                    content,
                    &block.style,
                    sheet,
                    config,
                    max_width,
                    zoom,
                    shaper,
                );
                let height = para.height;
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::Paragraph(para),
                }
            }
            block_kinds::HEADING => {
                let heading = block
                    .kind
                    .heading_data()
                    .unwrap_or(crate::model::HeadingData {
                        level: 1,
                        collapsed: false,
                    });
                let para = layout_paragraph_for_block(
                    Some(block.id),
                    content,
                    &block.style,
                    sheet,
                    config,
                    max_width,
                    zoom,
                    shaper,
                );
                let height = para.height;
                let chevron_size = pt_to_px(16.0, zoom);
                let first_line_center = para
                    .lines
                    .first()
                    .map(|line| line.baseline_y - line.ascent * 0.38)
                    .unwrap_or(height * 0.5);
                let chevron_rect = Rect::new(
                    -chevron_size - pt_to_px(4.0, zoom),
                    first_line_center - chevron_size * 0.5,
                    chevron_size,
                    chevron_size,
                );
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::Heading {
                        level: heading.level,
                        collapsed: heading.collapsed,
                        chevron_rect,
                        paragraph: para,
                    },
                }
            }
            block_kinds::LIST_ITEM => {
                let item = block
                    .kind
                    .list_item_data()
                    .unwrap_or(crate::model::ListItemData {
                        list_id: crate::model::ListId(1),
                        kind: ListKind::Bullet,
                        indent_level: 0,
                        checked: None,
                    });
                let para = layout_paragraph_for_block(
                    Some(block.id),
                    content,
                    &block.style,
                    sheet,
                    config,
                    max_width,
                    zoom,
                    shaper,
                );
                let height = para.height;
                let marker = list_marker.map(str::to_string).unwrap_or_else(|| {
                    default_list_marker(item.kind, item.indent_level, 1, item.checked)
                });
                let indent_start = pt_to_px(block.style.indent_start_pt, zoom);
                let gutter_x = (indent_start - pt_to_px(18.0, zoom)).max(0.0);
                let baseline_y = para
                    .lines
                    .first()
                    .map(|line| line.baseline_y)
                    .unwrap_or(pt_to_px(11.0, zoom));
                let checkbox_rect = item.checked.map(|_| {
                    let box_size = pt_to_px(10.5, zoom);
                    let center_y = para
                        .lines
                        .first()
                        .map(|line| line.baseline_y - line.ascent * 0.38)
                        .unwrap_or(baseline_y - box_size * 0.5);
                    Rect::new(gutter_x, center_y - box_size * 0.5, box_size, box_size)
                });
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::ListItem {
                        marker,
                        checked: item.checked,
                        marker_origin: Point::new(gutter_x, baseline_y),
                        checkbox_rect,
                        paragraph: para,
                    },
                }
            }
            block_kinds::CODE_BLOCK => {
                let language = block
                    .kind
                    .code_block_data()
                    .and_then(|code| code.language.clone());
                let pad_x = pt_to_px(12.0, zoom);
                let pad_y = pt_to_px(10.0, zoom);
                let header_h = pt_to_px(20.0, zoom);
                let inner_w = (max_width - 2.0 * pad_x).max(40.0);
                let para = layout_paragraph_for_block(
                    Some(block.id),
                    content,
                    &block.style,
                    sheet,
                    config,
                    inner_w,
                    zoom,
                    shaper,
                );
                let total_h = header_h + para.height + 2.0 * pad_y;
                let button_w = pt_to_px(44.0, zoom);
                let button_h = pt_to_px(15.0, zoom);
                let copy_button_rect = Rect::new(
                    (max_width - button_w - pt_to_px(8.0, zoom)).max(pt_to_px(4.0, zoom)),
                    pt_to_px(4.5, zoom),
                    button_w,
                    button_h,
                );
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height: total_h,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::CodeBlock {
                        language,
                        frame_rect: Rect::new(0.0, 0.0, max_width, total_h),
                        copy_button_rect,
                        content_origin: Point::new(pad_x, header_h + pad_y),
                        paragraph: para,
                    },
                }
            }
            _ => {
                let raw_chrome =
                    shaper.measure_custom_chrome(BlockShapeKind::Text, kind, data, max_width, zoom);
                let chrome = if is_valid_custom_insets(raw_chrome) {
                    raw_chrome
                } else {
                    Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
                };
                let inner_w = (max_width - chrome.horizontal()).max(40.0);
                let para = layout_paragraph_for_block(
                    Some(block.id),
                    content,
                    &block.style,
                    sheet,
                    config,
                    inner_w,
                    zoom,
                    shaper,
                );
                let total_h = (chrome.top + para.height + chrome.bottom).max(1.0);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height: total_h,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::CustomText {
                        kind: kind.clone(),
                        data: data.clone(),
                        bounds: Rect::new(0.0, 0.0, max_width, total_h),
                        chrome,
                        content_origin: Point::new(chrome.left, chrome.top),
                        paragraph: para,
                    },
                }
            }
        },
        BlockKind::Container {
            kind,
            data,
            children,
        } => {
            if kind.as_ref() == block_kinds::BLOCKQUOTE {
                let indent_px = pt_to_px(block.style.indent_start_pt.max(14.0), zoom);
                let child_max_w = (max_width - indent_px).max(40.0);
                let mut placed = Vec::with_capacity(children.len());
                let mut y = 0.0f32;
                let mut prev_after = 0.0f32;

                for (i, child) in children.iter().enumerate() {
                    let child_layout =
                        cache.get_or_layout(child, None, sheet, config, child_max_w, zoom, shaper);
                    if i > 0 {
                        y += prev_after.max(child_layout.space_before_px);
                    }
                    let ch = child_layout.height;
                    prev_after = child_layout.space_after_px;
                    placed.push(PlacedChildBlock {
                        x: indent_px,
                        y,
                        layout: child_layout,
                    });
                    y += ch;
                }
                let total_h = y.max(pt_to_px(16.0, zoom));
                let bar_w = pt_to_px(3.0, zoom).max(2.0);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height: total_h,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::Blockquote {
                        bar_rect: Rect::new(0.0, 0.0, bar_w, total_h),
                        children: placed,
                    },
                }
            } else {
                let raw_chrome = shaper.measure_custom_chrome(
                    BlockShapeKind::Container,
                    kind,
                    data,
                    max_width,
                    zoom,
                );
                let chrome = if is_valid_custom_insets(raw_chrome) {
                    raw_chrome
                } else {
                    Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
                };
                let child_max_w = (max_width - chrome.horizontal()).max(40.0);
                let mut placed = Vec::with_capacity(children.len());
                let mut y = chrome.top;
                let mut prev_after = 0.0f32;

                for (i, child) in children.iter().enumerate() {
                    let child_layout =
                        cache.get_or_layout(child, None, sheet, config, child_max_w, zoom, shaper);
                    if i > 0 {
                        y += prev_after.max(child_layout.space_before_px);
                    }
                    let ch = child_layout.height;
                    prev_after = child_layout.space_after_px;
                    placed.push(PlacedChildBlock {
                        x: chrome.left,
                        y,
                        layout: child_layout,
                    });
                    y += ch;
                }
                let min_h = chrome.vertical() + pt_to_px(16.0, zoom);
                let total_h = (y + chrome.bottom).max(min_h);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height: total_h,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::CustomContainer {
                        kind: kind.clone(),
                        data: data.clone(),
                        bounds: Rect::new(0.0, 0.0, max_width, total_h),
                        chrome,
                        children: placed,
                    },
                }
            }
        }
        BlockKind::Grid { kind, data, table } => {
            if kind.as_ref() == block_kinds::TABLE {
                let t_layout = layout_table(table, sheet, config, max_width, zoom, cache, shaper);
                let total_h = t_layout.total_height;
                let total_w = t_layout.total_width;
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: total_w,
                    height: total_h,
                    space_before_px: space_before_px.max(pt_to_px(6.0, zoom)),
                    space_after_px: space_after_px.max(pt_to_px(10.0, zoom)),
                    kind: BlockLayoutKind::Table(t_layout),
                }
            } else {
                let raw_chrome =
                    shaper.measure_custom_chrome(BlockShapeKind::Grid, kind, data, max_width, zoom);
                let chrome = if is_valid_custom_insets(raw_chrome) {
                    raw_chrome
                } else {
                    Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
                };
                let inner_w = (max_width - chrome.horizontal()).max(40.0);
                let t_layout = layout_table(table, sheet, config, inner_w, zoom, cache, shaper);
                let total_w = (t_layout.total_width + chrome.horizontal()).max(max_width);
                let total_h = (chrome.top + t_layout.total_height + chrome.bottom).max(1.0);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: total_w,
                    height: total_h,
                    space_before_px: space_before_px.max(pt_to_px(6.0, zoom)),
                    space_after_px: space_after_px.max(pt_to_px(10.0, zoom)),
                    kind: BlockLayoutKind::CustomGrid {
                        kind: kind.clone(),
                        data: data.clone(),
                        bounds: Rect::new(0.0, 0.0, total_w, total_h),
                        chrome,
                        content_origin: Point::new(chrome.left, chrome.top),
                        table: t_layout,
                    },
                }
            }
        }
        BlockKind::Leaf { kind, data } => {
            if kind.as_ref() == block_kinds::HORIZONTAL_RULE {
                let h = pt_to_px(12.0, zoom);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width: max_width,
                    height: h,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::HorizontalRule { rule_y: h * 0.5 },
                }
            } else {
                let raw_measured =
                    shaper.measure_custom_block(block.id, kind, data, max_width, zoom);
                let measured = if is_valid_custom_size(raw_measured) {
                    raw_measured
                } else {
                    crate::layout::geometry::Size::new(max_width.max(1.0), pt_to_px(44.0, zoom))
                };
                let width = measured.width.clamp(1.0, max_width.max(1.0));
                let height = measured.height.max(1.0);
                BlockLayout {
                    id: block.id,
                    version: block.version,
                    width,
                    height,
                    space_before_px,
                    space_after_px,
                    kind: BlockLayoutKind::CustomLeaf {
                        kind: kind.clone(),
                        data: data.clone(),
                        bounds: Rect::new(0.0, 0.0, width, height),
                    },
                }
            }
        }
    }
}

fn layout_table<S: TextShaper>(
    table: &TableBlock,
    sheet: &StyleSheet,
    config: &DocumentLayoutConfig,
    max_width: f32,
    zoom: f32,
    cache: &mut LayoutCache<S::Backend>,
    shaper: &mut S,
) -> TableLayout<S::Backend> {
    let (col_count, placements, _) = table.resolve_grid();

    let column_widths = resolve_column_widths(&table.columns, col_count, max_width, zoom);
    let total_width: f32 = column_widths.iter().sum();

    let mut col_x = Vec::with_capacity(col_count + 1);
    let mut acc_x = 0.0f32;
    for &w in &column_widths {
        col_x.push(acc_x);
        acc_x += w;
    }
    col_x.push(acc_x);

    let row_count = table.rows.len();
    let mut row_heights = vec![pt_to_px(24.0, zoom); row_count];

    struct MeasuredCell<B: GlyphBackend> {
        cell_id: BlockId,
        row: usize,
        col: usize,
        cell_index: usize,
        colspan: u16,
        rowspan: u16,
        is_header: bool,
        x: f32,
        width: f32,
        content_height: f32,
        pad_top: f32,
        pad_bottom: f32,
        pad_left: f32,
        valign: CellVAlign,
        background: Option<RgbaColor>,
        children: Vec<PlacedChildBlock<B>>,
    }

    let mut placements = placements.into_iter().peekable();
    let mut measured = Vec::with_capacity(placements.len());

    for (r_idx, row) in table.rows.iter().enumerate() {
        while let Some(placement) = placements.next_if(|placement| placement.row_index == r_idx) {
            let cell_index = placement.cell_index;
            let cell = &row.cells[cell_index];
            let c_cursor = placement.visual_col;
            let colspan = placement.colspan;
            let rowspan = placement.rowspan;

            let cell_x = col_x[c_cursor];
            let cell_w = col_x[c_cursor + colspan] - cell_x;
            let pad_l = pt_to_px(cell.padding_pt.left, zoom);
            let pad_r = pt_to_px(cell.padding_pt.right, zoom);
            let pad_t = pt_to_px(cell.padding_pt.top, zoom);
            let pad_b = pt_to_px(cell.padding_pt.bottom, zoom);
            let inner_w = (cell_w - pad_l - pad_r).max(20.0);

            let mut children = Vec::with_capacity(cell.blocks.len());
            let mut cy = 0.0f32;
            let mut prev_after = 0.0f32;
            for (bi, child) in cell.blocks.iter().enumerate() {
                let layout = cache.get_or_layout(child, None, sheet, config, inner_w, zoom, shaper);
                if bi > 0 {
                    cy += prev_after.max(layout.space_before_px);
                }
                let h = layout.height;
                prev_after = layout.space_after_px;
                children.push(PlacedChildBlock {
                    x: 0.0,
                    y: cy,
                    layout,
                });
                cy += h;
            }

            let min_cell_h = cy + pad_t + pad_b;
            if rowspan == 1 {
                row_heights[r_idx] = row_heights[r_idx].max(min_cell_h);
            }

            measured.push(MeasuredCell {
                cell_id: cell.id,
                row: r_idx,
                col: c_cursor,
                cell_index,
                colspan: colspan as u16,
                rowspan: rowspan as u16,
                is_header: row.is_header,
                x: cell_x,
                width: cell_w,
                content_height: cy,
                pad_top: pad_t,
                pad_bottom: pad_b,
                pad_left: pad_l,
                valign: cell.valign,
                background: cell.background,
                children,
            });
        }
    }

    // Distribute surplus height from rowspan > 1 cells across their spanned rows.
    for mc in &measured {
        if mc.rowspan > 1 {
            let span_end = (mc.row + mc.rowspan as usize).min(row_count);
            let cur_sum: f32 = row_heights[mc.row..span_end].iter().sum();
            let needed = mc.content_height + mc.pad_top + mc.pad_bottom;
            if needed > cur_sum {
                let extra = (needed - cur_sum) / ((span_end - mc.row) as f32);
                for h in &mut row_heights[mc.row..span_end] {
                    *h += extra;
                }
            }
        }
    }

    let mut row_y = Vec::with_capacity(row_count + 1);
    let mut acc_y = 0.0f32;
    for &h in &row_heights {
        row_y.push(acc_y);
        acc_y += h;
    }
    row_y.push(acc_y);

    let cells = measured
        .into_iter()
        .map(|mut mc| {
            let y = row_y[mc.row];
            let h = row_y[mc.row + mc.rowspan as usize] - y;
            let inner_avail = (h - mc.pad_top - mc.pad_bottom).max(0.0);
            let valign_offset = match mc.valign {
                CellVAlign::Top => 0.0,
                CellVAlign::Middle => ((inner_avail - mc.content_height) * 0.5).max(0.0),
                CellVAlign::Bottom => (inner_avail - mc.content_height).max(0.0),
            };
            for ch in &mut mc.children {
                ch.x += mc.pad_left;
                ch.y += mc.pad_top + valign_offset;
            }
            TableCellLayout {
                cell_id: mc.cell_id,
                row: mc.row,
                col: mc.col,
                cell_index: mc.cell_index,
                colspan: mc.colspan,
                rowspan: mc.rowspan,
                is_header: mc.is_header,
                rect: Rect::new(mc.x, y, mc.width, h),
                background: mc.background,
                children: mc.children,
            }
        })
        .collect();

    TableLayout {
        total_width,
        total_height: acc_y,
        border_style: table.border_style,
        column_widths,
        row_heights,
        cells,
    }
}

fn resolve_column_widths(
    specs: &[ColumnSpec],
    col_count: usize,
    max_width: f32,
    zoom: f32,
) -> Vec<f32> {
    let mut widths = vec![0.0f32; col_count];
    let mut fixed_sum = 0.0f32;
    let mut ratio_sum = 0.0f32;
    let mut auto_count = 0usize;

    for (index, width) in widths.iter_mut().enumerate() {
        match specs.get(index).copied().unwrap_or(ColumnSpec::Auto) {
            ColumnSpec::FixedPt(pt) => {
                let px = pt_to_px(pt, zoom).max(32.0);
                *width = px;
                fixed_sum += px;
            }
            ColumnSpec::Ratio(ratio) => {
                ratio_sum += ratio.max(0.05);
            }
            ColumnSpec::Auto => {
                auto_count += 1;
            }
        }
    }

    let remaining = (max_width - fixed_sum).max(col_count as f32 * 40.0);
    let total_weight = ratio_sum + auto_count as f32;
    for (index, width) in widths.iter_mut().enumerate() {
        match specs.get(index).copied().unwrap_or(ColumnSpec::Auto) {
            ColumnSpec::FixedPt(_) => {}
            ColumnSpec::Ratio(ratio) => {
                *width = (remaining * (ratio.max(0.05) / total_weight.max(0.01))).max(32.0);
            }
            ColumnSpec::Auto => {
                *width = (remaining * (1.0 / total_weight.max(0.01))).max(32.0);
            }
        }
    }
    widths
}

pub fn default_list_marker(
    kind: ListKind,
    indent_level: u8,
    ordinal: usize,
    checked: Option<bool>,
) -> String {
    match kind {
        ListKind::Bullet => match indent_level % 3 {
            0 => "•".into(),
            1 => "◦".into(),
            _ => "▪".into(),
        },
        ListKind::Checklist => {
            if checked.unwrap_or(false) {
                "☑".into()
            } else {
                "☐".into()
            }
        }
        ListKind::Ordered(style) => match style {
            NumberStyle::Decimal => format!("{ordinal}."),
            NumberStyle::LowerAlpha => {
                let idx = (ordinal.saturating_sub(1)) % 26;
                format!("{}.", (b'a' + idx as u8) as char)
            }
            NumberStyle::LowerRoman => format!("{}.", to_lower_roman(ordinal)),
        },
    }
}

fn to_lower_roman(mut n: usize) -> String {
    if n == 0 {
        return "0".into();
    }
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for &(val, sym) in &table {
        while n >= val {
            out.push_str(sym);
            n -= val;
        }
    }
    out
}
