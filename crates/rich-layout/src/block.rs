use crate::geometry::{Point, Rect, pt_to_px};
use crate::line::{HitResult, ParagraphLayout, layout_paragraph};
use crate::shaper::{GlyphBackend, TextShaper};
use rich_model::{
    BlockId, BlockKind, BlockNode, CellVAlign, ColumnSpec, DocumentLayoutConfig, ListKind,
    NumberStyle, RgbaColor, StyleSheet, TableBlock, TableBorderStyle,
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
    pub col: usize,
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
        paragraph: ParagraphLayout<B>,
    },
    Blockquote {
        bar_rect: Rect,
        children: Vec<PlacedChildBlock<B>>,
    },
    CodeBlock {
        language: Option<String>,
        frame_rect: Rect,
        content_origin: Point,
        paragraph: ParagraphLayout<B>,
    },
    Table(TableLayout<B>),
    HorizontalRule {
        rule_y: f32,
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
            | BlockLayoutKind::CodeBlock { paragraph: p, .. } => Some(p),
            BlockLayoutKind::Blockquote { .. }
            | BlockLayoutKind::Table(_)
            | BlockLayoutKind::HorizontalRule { .. } => None,
        }
    }

    pub fn content_origin(&self) -> Point {
        match &self.kind {
            BlockLayoutKind::CodeBlock { content_origin, .. } => *content_origin,
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
}

impl<B: GlyphBackend> Default for LayoutCache<B> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            latest_by_id: HashMap::new(),
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

        let layout = Arc::new(layout_block(
            block,
            list_marker,
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
        BlockKind::Paragraph(content) => {
            let para = layout_paragraph(content, &block.style, sheet, config, max_width, zoom, shaper);
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
        BlockKind::Heading {
            level,
            collapsed,
            content,
        } => {
            let para = layout_paragraph(content, &block.style, sheet, config, max_width, zoom, shaper);
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
                    level: *level,
                    collapsed: *collapsed,
                    chevron_rect,
                    paragraph: para,
                },
            }
        }
        BlockKind::ListItem {
            kind,
            indent_level,
            checked,
            content,
            ..
        } => {
            let para = layout_paragraph(content, &block.style, sheet, config, max_width, zoom, shaper);
            let height = para.height;
            let marker = list_marker.map(str::to_string).unwrap_or_else(|| {
                default_list_marker(*kind, *indent_level, 1, *checked)
            });
            let indent_start = pt_to_px(block.style.indent_start_pt, zoom);
            let gutter_x = (indent_start - pt_to_px(18.0, zoom)).max(0.0);
            let baseline_y = para
                .lines
                .first()
                .map(|line| line.baseline_y)
                .unwrap_or(pt_to_px(11.0, zoom));
            BlockLayout {
                id: block.id,
                version: block.version,
                width: max_width,
                height,
                space_before_px,
                space_after_px,
                kind: BlockLayoutKind::ListItem {
                    marker,
                    checked: *checked,
                    marker_origin: Point::new(gutter_x, baseline_y),
                    paragraph: para,
                },
            }
        }
        BlockKind::Blockquote(children) => {
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
        }
        BlockKind::CodeBlock { language, content } => {
            let pad_x = pt_to_px(12.0, zoom);
            let pad_y = pt_to_px(10.0, zoom);
            let header_h = if language.is_some() {
                pt_to_px(18.0, zoom)
            } else {
                0.0
            };
            let inner_w = (max_width - 2.0 * pad_x).max(40.0);
            let para = layout_paragraph(content, &block.style, sheet, config, inner_w, zoom, shaper);
            let total_h = header_h + para.height + 2.0 * pad_y;
            BlockLayout {
                id: block.id,
                version: block.version,
                width: max_width,
                height: total_h,
                space_before_px,
                space_after_px,
                kind: BlockLayoutKind::CodeBlock {
                    language: language.clone(),
                    frame_rect: Rect::new(0.0, 0.0, max_width, total_h),
                    content_origin: Point::new(pad_x, header_h + pad_y),
                    paragraph: para,
                },
            }
        }
        BlockKind::Table(table) => {
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
        }
        BlockKind::HorizontalRule => {
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
    let col_count = table
        .columns
        .len()
        .max(
            table
                .rows
                .iter()
                .map(|r| r.cells.iter().map(|c| c.colspan.max(1) as usize).sum())
                .max()
                .unwrap_or(1),
        )
        .max(1);

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

    let mut occupied = vec![vec![false; col_count]; row_count];
    let mut measured = Vec::new();

    for (r_idx, row) in table.rows.iter().enumerate() {
        let mut c_cursor = 0usize;
        for cell in &row.cells {
            while c_cursor < col_count && occupied[r_idx][c_cursor] {
                c_cursor += 1;
            }
            if c_cursor >= col_count {
                break;
            }
            let colspan = (cell.colspan.max(1) as usize).min(col_count - c_cursor);
            let rowspan = (cell.rowspan.max(1) as usize).min(row_count - r_idx);
            for rr in r_idx..(r_idx + rowspan) {
                for cc in c_cursor..(c_cursor + colspan) {
                    occupied[rr][cc] = true;
                }
            }

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
                let layout =
                    cache.get_or_layout(child, None, sheet, config, inner_w, zoom, shaper);
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
            c_cursor += colspan;
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

    for i in 0..col_count {
        match specs.get(i).copied().unwrap_or(ColumnSpec::Auto) {
            ColumnSpec::FixedPt(pt) => {
                let px = pt_to_px(pt, zoom).max(32.0);
                widths[i] = px;
                fixed_sum += px;
            }
            ColumnSpec::Ratio(r) => {
                ratio_sum += r.max(0.05);
            }
            ColumnSpec::Auto => {
                auto_count += 1;
            }
        }
    }

    let remaining = (max_width - fixed_sum).max(col_count as f32 * 40.0);
    let total_weight = ratio_sum + auto_count as f32;
    for i in 0..col_count {
        match specs.get(i).copied().unwrap_or(ColumnSpec::Auto) {
            ColumnSpec::FixedPt(_) => {}
            ColumnSpec::Ratio(r) => {
                widths[i] = (remaining * (r.max(0.05) / total_weight.max(0.01))).max(32.0);
            }
            ColumnSpec::Auto => {
                widths[i] = (remaining * (1.0 / total_weight.max(0.01))).max(32.0);
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
