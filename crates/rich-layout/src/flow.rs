use crate::block::{BlockLayout, LayoutCache, default_list_marker};
use crate::geometry::{Point, Rect, pt_to_px};
use crate::line::{HitResult, LineFragment, ParagraphLayout};
use crate::shaper::{GlyphBackend, TextShaper};
use rich_model::{
    Affinity, BlockId, BlockKind, BlockNode, BlockWidth, DocPos, Document, DocumentLayoutConfig,
    DocumentSelection, Edges, HorizontalLayout, InlineObjectId, ListId, PathSegment,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Fenwick tree (Binary Indexed Tree) storing per-block vertical slot heights.
/// Provides `O(log N)` prefix sum (`y_of`), `O(log N)` height updates, and `O(log N)` `index_at_y`.
#[derive(Clone, Debug, Default)]
pub struct HeightIndex {
    slots: Vec<f32>,
    tree: Vec<f32>,
}

impl HeightIndex {
    pub fn new(len: usize) -> Self {
        Self {
            slots: vec![0.0; len],
            tree: vec![0.0; len + 1],
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn slot(&self, idx: usize) -> f32 {
        self.slots[idx]
    }

    pub fn set_slot(&mut self, idx: usize, new_val: f32) {
        let clamped = new_val.max(0.0);
        let delta = clamped - self.slots[idx];
        if delta.abs() < 1e-5 {
            return;
        }
        self.slots[idx] = clamped;
        let mut i = idx + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();
        }
    }

    /// Returns the sum of slots `0..count` (the Y offset of block `count`).
    pub fn prefix_sum(&self, count: usize) -> f32 {
        let mut i = count.min(self.slots.len());
        let mut sum = 0.0f32;
        while i > 0 {
            sum += self.tree[i];
            i -= i & i.wrapping_neg();
        }
        sum
    }

    pub fn total_height(&self) -> f32 {
        self.prefix_sum(self.slots.len())
    }

    /// Returns the smallest index `idx` such that `prefix_sum(idx + 1) > y` (or `len - 1`).
    pub fn index_at_y(&self, y: f32) -> usize {
        if self.slots.is_empty() || y <= 0.0 {
            return 0;
        }
        let n = self.slots.len();
        let mut idx = 0usize;
        let mut acc = 0.0f32;
        let mut bit = 1usize << (usize::BITS - 1 - n.leading_zeros());
        while bit > 0 {
            let next = idx + bit;
            if next <= n && acc + self.tree[next] <= y {
                idx = next;
                acc += self.tree[next];
            }
            bit >>= 1;
        }
        idx.min(n - 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAnchor {
    pub block_id: BlockId,
    pub local_y_offset: f32,
}

#[derive(Clone, Debug)]
pub struct VisibleBlock<B: GlyphBackend> {
    pub block_index: usize,
    pub id: BlockId,
    pub x: f32,
    pub y: f32,
    pub layout: Arc<BlockLayout<B>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanvasHitResult {
    pub pos: DocPos,
    pub affinity: Affinity,
    pub hit: HitResult,
    pub toggled_heading: Option<BlockId>,
    pub toggled_checklist: Option<BlockId>,
    pub code_block_copy: Option<BlockId>,
}

/// Hit-test metadata for dragging an internal vertical column border of a table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumnDividerHit {
    pub table_id: BlockId,
    /// Visual column index to the left of the divider (`0..column_widths_px.len() - 1`).
    pub column_index: usize,
    /// Canvas X coordinate of the column border.
    pub divider_x: f32,
    /// Canvas Y top coordinate of the table.
    pub table_y: f32,
    /// Canvas height of the table.
    pub table_height: f32,
    /// Current resolved column widths in logical pixels.
    pub column_widths_px: Vec<f32>,
    /// Total table width in logical pixels.
    pub total_width_px: f32,
}

/// Canvas-space horizontal bounds of the continuous infinite-length paper sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperSheetGeometry {
    pub x: f32,
    pub top_y: f32,
    pub width: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub desk_margin: f32,
}

/// Resolved horizontal metrics separating viewport-filling reflow from infinite-length paper layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalMetrics {
    pub canvas_width: f32,
    pub paper_sheet: Option<PaperSheetGeometry>,
    pub column_x: f32,
    pub column_width: f32,
}

/// Computes horizontal canvas geometry for `Fill` or `Paper` mode.
pub fn resolve_horizontal_metrics(
    viewport_width: f32,
    config: &DocumentLayoutConfig,
    zoom: f32,
) -> HorizontalMetrics {
    match config.horizontal_layout {
        HorizontalLayout::Fill => {
            let mut margin_left = pt_to_px(config.fill_margins_pt.left.max(0.0), zoom);
            let mut margin_right = pt_to_px(config.fill_margins_pt.right.max(0.0), zoom);
            let max_horizontal = (viewport_width - 160.0).max(0.0);
            let total_horizontal = margin_left + margin_right;
            if total_horizontal > max_horizontal && total_horizontal > 0.0 {
                let scale = max_horizontal / total_horizontal;
                margin_left *= scale;
                margin_right *= scale;
            }
            let column_width = (viewport_width - margin_left - margin_right).max(160.0);
            HorizontalMetrics {
                canvas_width: viewport_width.max(column_width + margin_left + margin_right),
                paper_sheet: None,
                column_x: margin_left,
                column_width,
            }
        }
        HorizontalLayout::Paper(spec) => {
            let desk_margin = pt_to_px(spec.desk_margin_pt, zoom);
            let paper_width = pt_to_px(spec.width_pt, zoom).max(200.0);
            let mut margin_left = pt_to_px(spec.margins_pt.left.max(0.0), zoom);
            let mut margin_right = pt_to_px(spec.margins_pt.right.max(0.0), zoom);
            let margin_top = pt_to_px(spec.margins_pt.top.max(0.0), zoom);
            let margin_bottom = pt_to_px(spec.margins_pt.bottom.max(0.0), zoom);
            let max_horizontal = (paper_width - 120.0).max(0.0);
            let total_horizontal = margin_left + margin_right;
            if total_horizontal > max_horizontal && total_horizontal > 0.0 {
                let scale = max_horizontal / total_horizontal;
                margin_left *= scale;
                margin_right *= scale;
            }
            let canvas_width = viewport_width.max(paper_width + 2.0 * desk_margin);
            let sheet_x = ((canvas_width - paper_width) * 0.5).max(desk_margin);
            let column_width = (paper_width - margin_left - margin_right).max(120.0);
            HorizontalMetrics {
                canvas_width,
                paper_sheet: Some(PaperSheetGeometry {
                    x: sheet_x,
                    top_y: desk_margin,
                    width: paper_width,
                    margin_left,
                    margin_right,
                    margin_top,
                    margin_bottom,
                    desk_margin,
                }),
                column_x: sheet_x + margin_left,
                column_width,
            }
        }
    }
}

/// Resolves `(origin_x, max_width)` for a block within the active [`HorizontalLayout`].
pub fn resolve_block_horizontal_span(
    viewport_width: f32,
    config: &DocumentLayoutConfig,
    width_mode: BlockWidth,
    zoom: f32,
) -> (f32, f32) {
    let metrics = resolve_horizontal_metrics(viewport_width, config, zoom);
    match metrics.paper_sheet {
        Some(sheet) => match width_mode {
            BlockWidth::Column => (metrics.column_x, metrics.column_width),
            BlockWidth::Wide => {
                let left_inset = sheet.margin_left * 0.5;
                let right_inset = sheet.margin_right * 0.5;
                (
                    sheet.x + left_inset,
                    (sheet.width - left_inset - right_inset).max(120.0),
                )
            }
            BlockWidth::Full => {
                let left_inset = sheet.margin_left * 0.25;
                let right_inset = sheet.margin_right * 0.25;
                (
                    sheet.x + left_inset,
                    (sheet.width - left_inset - right_inset).max(120.0),
                )
            }
        },
        None => (metrics.column_x, metrics.column_width),
    }
}

/// Computes which top-level blocks are visible under collapsible heading rules.
pub fn compute_visibility_mask(doc: &Document) -> Vec<bool> {
    doc.top_level_visibility().collect()
}

/// Precomputes display markers (`•`, `1.`, `a.`, `☐`, `☑`) for top-level list items.
pub fn compute_list_markers(doc: &Document) -> Vec<Option<String>> {
    let mut markers = vec![None; doc.blocks.len()];
    let mut counters: HashMap<(ListId, u8), usize> = HashMap::new();

    for (idx, block) in doc.blocks.iter().enumerate() {
        if let Some(item) = block.kind.list_item_data() {
            // Reset deeper sub-list counters whenever a parent level increments.
            counters.retain(|&(lid, lvl), _| lid != item.list_id || lvl <= item.indent_level);
            let entry = counters.entry((item.list_id, item.indent_level)).or_insert(0);
            *entry += 1;
            markers[idx] = Some(default_list_marker(
                item.kind,
                item.indent_level,
                *entry,
                item.checked,
            ));
        }
    }
    markers
}

#[derive(Clone, Copy, Debug)]
struct BlockMetrics {
    content_height: f32,
    space_before: f32,
    space_after: f32,
    measured: bool,
}

/// Non-paginated continuous document flow manager with virtualized layout and scroll anchoring.
pub struct FlowLayout<B: GlyphBackend> {
    pub cache: LayoutCache<B>,
    height_index: HeightIndex,
    metrics: Vec<BlockMetrics>,
    visibility: Vec<bool>,
    list_markers: Vec<Option<String>>,
    block_ids: Vec<BlockId>,
    id_to_index: HashMap<BlockId, usize>,
    leaf_order: HashMap<BlockId, usize>,
    top_padding_px: f32,
    bottom_padding_px: f32,
    last_column_width: f32,
    last_zoom: f32,
    last_horizontal_layout: HorizontalLayout,
    last_margins_pt: Edges,
}

impl<B: GlyphBackend> Default for FlowLayout<B> {
    fn default() -> Self {
        Self {
            cache: LayoutCache::default(),
            height_index: HeightIndex::default(),
            metrics: Vec::new(),
            visibility: Vec::new(),
            list_markers: Vec::new(),
            block_ids: Vec::new(),
            id_to_index: HashMap::new(),
            leaf_order: HashMap::new(),
            top_padding_px: 0.0,
            bottom_padding_px: 0.0,
            last_column_width: 0.0,
            last_zoom: 1.0,
            last_horizontal_layout: HorizontalLayout::default(),
            last_margins_pt: DocumentLayoutConfig::default().active_margins_pt(),
        }
    }
}

impl<B: GlyphBackend> FlowLayout<B> {
    pub fn total_height(&self) -> f32 {
        self.top_padding_px + self.height_index.total_height() + self.bottom_padding_px
    }

    pub fn block_y(&self, block_idx: usize) -> f32 {
        self.top_padding_px + self.height_index.prefix_sum(block_idx)
    }

    pub fn block_y_by_id(&self, id: BlockId) -> Option<f32> {
        let &idx = self.id_to_index.get(&id)?;
        Some(self.block_y(idx))
    }

    pub fn is_visible(&self, block_idx: usize) -> bool {
        self.visibility.get(block_idx).copied().unwrap_or(false)
    }

    /// Synchronizes structural metadata and height estimates with `doc`.
    pub fn sync_document(&mut self, doc: &Document, viewport_width: f32, zoom: f32) {
        let horizontal = resolve_horizontal_metrics(viewport_width, &doc.config, zoom);
        if let Some(sheet) = horizontal.paper_sheet {
            self.top_padding_px = sheet.top_y + sheet.margin_top;
            self.bottom_padding_px = sheet.margin_bottom + sheet.desk_margin;
        } else {
            self.top_padding_px = pt_to_px(doc.config.fill_margins_pt.top.max(0.0), zoom);
            self.bottom_padding_px = pt_to_px(doc.config.fill_margins_pt.bottom.max(0.0), zoom);
        }

        let active_margins_pt = doc.config.active_margins_pt();
        let geometry_changed = (horizontal.column_width - self.last_column_width).abs() > 0.5
            || (zoom - self.last_zoom).abs() > 0.001
            || doc.config.horizontal_layout != self.last_horizontal_layout
            || active_margins_pt != self.last_margins_pt;
        self.last_column_width = horizontal.column_width;
        self.last_zoom = zoom;
        self.last_horizontal_layout = doc.config.horizontal_layout;
        self.last_margins_pt = active_margins_pt;

        let prev_metrics: HashMap<BlockId, BlockMetrics> = self
            .block_ids
            .iter()
            .copied()
            .zip(self.metrics.iter().copied())
            .collect();

        self.visibility = compute_visibility_mask(doc);
        self.cache.sync_ordered_markers(doc);
        self.list_markers = compute_list_markers(doc);
        self.block_ids = doc.blocks.iter().map(|b| b.id).collect();
        self.id_to_index.clear();
        for (i, id) in self.block_ids.iter().copied().enumerate() {
            self.id_to_index.insert(id, i);
        }
        self.leaf_order.clear();
        for (order_index, leaf_block) in doc.collect_leaf_blocks().into_iter().enumerate() {
            self.leaf_order.insert(leaf_block.id, order_index);
        }

        self.metrics.clear();
        self.metrics.reserve(doc.blocks.len());

        for block in &doc.blocks {
            let (_, max_w) =
                resolve_block_horizontal_span(viewport_width, &doc.config, block.width_mode, zoom);
            if let Some(cached) = self.cache.get_cached(block.id, block.version, max_w, zoom) {
                self.metrics.push(BlockMetrics {
                    content_height: cached.height,
                    space_before: cached.space_before_px,
                    space_after: cached.space_after_px,
                    measured: true,
                });
            } else if !geometry_changed
                && let Some(prev) = prev_metrics.get(&block.id)
            {
                self.metrics.push(*prev);
            } else {
                self.metrics.push(estimate_block_metrics(
                    block,
                    &doc.stylesheet,
                    max_w,
                    zoom,
                ));
            }
        }

        self.rebuild_height_index();
    }

    pub fn capture_anchor(&self, scroll_y: f32) -> Option<ScrollAnchor> {
        if self.block_ids.is_empty() {
            return None;
        }
        let content_y = (scroll_y - self.top_padding_px).max(0.0);
        let mut idx = self.height_index.index_at_y(content_y);
        while idx < self.block_ids.len() && !self.is_visible(idx) {
            idx += 1;
        }
        if idx >= self.block_ids.len() {
            return None;
        }
        let block_top = self.block_y(idx);
        Some(ScrollAnchor {
            block_id: self.block_ids[idx],
            local_y_offset: scroll_y - block_top,
        })
    }

    pub fn restore_anchor(&self, anchor: ScrollAnchor, viewport_height: f32) -> f32 {
        let max_scroll = (self.total_height() - viewport_height).max(0.0);
        if let Some(top) = self.block_y_by_id(anchor.block_id) {
            (top + anchor.local_y_offset).clamp(0.0, max_scroll)
        } else {
            0.0
        }
    }

    /// Ensures all blocks inside `[scroll_y - overscan, scroll_y + viewport_h + overscan]`
    /// are measured and returns them in visual top-to-bottom order.
    pub fn layout_visible_blocks<S: TextShaper<Backend = B>>(
        &mut self,
        doc: &Document,
        scroll_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        overscan: f32,
        zoom: f32,
        shaper: &mut S,
    ) -> Vec<VisibleBlock<B>> {
        if doc.blocks.is_empty() {
            return Vec::new();
        }
        self.sync_geometry_if_needed(doc, viewport_width, zoom);

        let start_y = (scroll_y - overscan - self.top_padding_px).max(0.0);
        let end_y = (scroll_y + viewport_height + overscan - self.top_padding_px).max(0.0);

        let start_idx = self.height_index.index_at_y(start_y);
        let mut idx = start_idx;
        let mut measured_indices = Vec::new();

        while idx < doc.blocks.len() {
            let y_rel = self.height_index.prefix_sum(idx);
            if y_rel > end_y && !measured_indices.is_empty() {
                break;
            }
            if self.is_visible(idx) {
                let block = &doc.blocks[idx];
                let (x, max_w) = resolve_block_horizontal_span(
                    viewport_width,
                    &doc.config,
                    block.width_mode,
                    zoom,
                );
                let marker = self.list_markers[idx].as_deref();
                let layout = self.cache.get_or_layout(
                    block,
                    marker,
                    &doc.stylesheet,
                    &doc.config,
                    max_w,
                    zoom,
                    shaper,
                );
                let changed = !self.metrics[idx].measured
                    || (self.metrics[idx].content_height - layout.height).abs() > 0.05;
                if changed {
                    self.metrics[idx] = BlockMetrics {
                        content_height: layout.height,
                        space_before: layout.space_before_px,
                        space_after: layout.space_after_px,
                        measured: true,
                    };
                    self.update_slot_and_prev_neighbor(idx);
                }
                measured_indices.push((idx, x, layout));
            }
            idx += 1;
        }

        measured_indices
            .into_iter()
            .map(|(block_index, x, layout)| VisibleBlock {
                block_index,
                id: doc.blocks[block_index].id,
                x,
                y: self.block_y(block_index),
                layout,
            })
            .collect()
    }

    fn sync_geometry_if_needed(&mut self, doc: &Document, viewport_width: f32, zoom: f32) {
        let horizontal = resolve_horizontal_metrics(viewport_width, &doc.config, zoom);
        if self.block_ids.len() != doc.blocks.len()
            || (horizontal.column_width - self.last_column_width).abs() > 0.5
            || (zoom - self.last_zoom).abs() > 0.001
            || doc.config.horizontal_layout != self.last_horizontal_layout
            || doc.config.active_margins_pt() != self.last_margins_pt
        {
            self.sync_document(doc, viewport_width, zoom);
        }
    }
    /// Measures the top-level block containing `pos`, including off-screen nested table cells,
    /// and returns the resulting canvas-space caret rectangle.
    pub fn measure_caret_rect<S: TextShaper<Backend = B>>(
        &mut self,
        doc: &Document,
        pos: &DocPos,
        affinity: Affinity,
        viewport_width: f32,
        zoom: f32,
        shaper: &mut S,
    ) -> Option<Rect> {
        self.sync_geometry_if_needed(doc, viewport_width, zoom);
        let top_id = pos.top_level_block_id()?;
        let leaf_id = pos.leaf_block_id()?;
        let idx = *self.id_to_index.get(&top_id)?;
        if !self.is_visible(idx) {
            return None;
        }
        let block = doc.blocks.get(idx)?;
        let (block_x, max_w) =
            resolve_block_horizontal_span(viewport_width, &doc.config, block.width_mode, zoom);
        let marker = self.list_markers.get(idx).and_then(Option::as_deref);
        let layout = self.cache.get_or_layout(
            block,
            marker,
            &doc.stylesheet,
            &doc.config,
            max_w,
            zoom,
            shaper,
        );
        self.metrics[idx] = BlockMetrics {
            content_height: layout.height,
            space_before: layout.space_before_px,
            space_after: layout.space_after_px,
            measured: true,
        };
        self.update_slot_and_prev_neighbor(idx);
        let (paragraph, origin) =
            find_leaf_paragraph(&layout, leaf_id, Point::new(block_x, self.block_y(idx)))?;
        Some(paragraph.caret_rect(pos.offset, affinity).translate(origin.x, origin.y))
    }

    pub fn hit_test_canvas<S: TextShaper<Backend = B>>(
        &mut self,
        canvas_point: Point,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
        shaper: &mut S,
    ) -> Option<CanvasHitResult> {
        if doc.blocks.is_empty() {
            return None;
        }
        let content_y = (canvas_point.y - self.top_padding_px).max(0.0);
        let mut idx = self.height_index.index_at_y(content_y);
        while idx < doc.blocks.len() && !self.is_visible(idx) {
            idx += 1;
        }
        if idx >= doc.blocks.len() {
            idx = (0..doc.blocks.len()).rev().find(|&i| self.is_visible(i))?;
        }

        let block = &doc.blocks[idx];
        let (bx, max_w) =
            resolve_block_horizontal_span(viewport_width, &doc.config, block.width_mode, zoom);
        let by = self.block_y(idx);
        let marker = self.list_markers[idx].as_deref();
        let layout = self.cache.get_or_layout(
            block,
            marker,
            &doc.stylesheet,
            &doc.config,
            max_w,
            zoom,
            shaper,
        );
        let local = Point::new(canvas_point.x - bx, canvas_point.y - by);

        // Check interactive block chrome hit targets (heading chevron, checklist box, code copy button).
        if let Some(chrome_hit) = hit_test_block_chrome(&layout, local, block.id) {
            return Some(chrome_hit);
        }

        if let Some(hit) = layout.hit_test_local(local) {
            return Some(CanvasHitResult {
                pos: DocPos::top_level(block.id, hit.offset),
                affinity: hit.affinity,
                hit,
                toggled_heading: None,
                toggled_checklist: None,
                code_block_copy: None,
            });
        }

        let mut path_prefix = smallvec::SmallVec::new();
        hit_test_layout_recursive(&layout, local, &mut path_prefix)
    }

    /// Hit-tests internal vertical column dividers of cached top-level tables within `tolerance_px`.
    pub fn hit_test_table_column_border(
        &self,
        canvas_point: Point,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
        tolerance_px: f32,
    ) -> Option<TableColumnDividerHit> {
        if doc.blocks.is_empty() {
            return None;
        }
        let content_y = (canvas_point.y - self.top_padding_px).max(0.0);
        let idx = self.height_index.index_at_y(content_y);
        let block = doc.blocks.get(idx)?;
        if !self.is_visible(idx) || block.kind.table_block().is_none() {
            return None;
        }
        let layout = self.cache.get_latest(block.id)?;
        let table = layout.table_layout()?;
        if table.column_widths.len() < 2 {
            return None;
        }
        let content_origin = layout.content_origin();
        let (block_x, _) =
            resolve_block_horizontal_span(viewport_width, &doc.config, block.width_mode, zoom);
        let block_y = self.block_y(idx) + content_origin.y;
        if canvas_point.y < block_y || canvas_point.y > block_y + table.total_height {
            return None;
        }

        let mut acc_x = block_x + content_origin.x;
        for (column_index, &width) in table
            .column_widths
            .iter()
            .take(table.column_widths.len() - 1)
            .enumerate()
        {
            acc_x += width;
            if (canvas_point.x - acc_x).abs() <= tolerance_px {
                return Some(TableColumnDividerHit {
                    table_id: block.id,
                    column_index,
                    divider_x: acc_x,
                    table_y: block_y,
                    table_height: table.total_height,
                    column_widths_px: table.column_widths.clone(),
                    total_width_px: table.total_width,
                });
            }
        }
        None
    }

    /// Returns the canvas-space rectangle of an inline object inside the visible blocks.
    pub fn inline_object_rect_in_visible(
        &self,
        block_id: BlockId,
        object_id: InlineObjectId,
        visible_blocks: &[VisibleBlock<B>],
    ) -> Option<Rect> {
        let mut leaves = Vec::new();
        for visible in visible_blocks {
            collect_leaf_paragraphs(&visible.layout, visible.x, visible.y, &mut leaves);
        }
        let target = leaves.into_iter().find(|leaf| leaf.block_id == block_id)?;
        for line in &target.paragraph.lines {
            for fragment in &line.fragments {
                if let LineFragment::InlineBox(inline_box) = fragment
                    && inline_box.object_id == object_id
                {
                    return Some(
                        line.inline_box_rect(inline_box)
                            .translate(target.origin_x, target.origin_y),
                    );
                }
            }
        }
        None
    }

    /// Returns the canvas-space rectangle of an inline object using cached layouts.
    pub fn inline_object_rect_cached(
        &self,
        block_id: BlockId,
        object_id: InlineObjectId,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
    ) -> Option<Rect> {
        let pos = doc.doc_pos_for_block(block_id, 0)?;
        let top_id = pos.top_level_block_id()?;
        let top_index = doc.top_level_index(top_id)?;
        let top_block = doc.blocks.get(top_index)?;
        let layout = self.cache.get_latest(top_id)?;
        let (block_x, _) =
            resolve_block_horizontal_span(viewport_width, &doc.config, top_block.width_mode, zoom);
        let block_y = self.block_y(top_index);

        let mut leaves = Vec::new();
        collect_leaf_paragraphs(&layout, block_x, block_y, &mut leaves);
        let target = leaves.into_iter().find(|leaf| leaf.block_id == block_id)?;
        for line in &target.paragraph.lines {
            for fragment in &line.fragments {
                if let LineFragment::InlineBox(inline_box) = fragment
                    && inline_box.object_id == object_id
                {
                    return Some(
                        line.inline_box_rect(inline_box)
                            .translate(target.origin_x, target.origin_y),
                    );
                }
            }
        }
        None
    }

    /// Computes canvas-space selection rectangles for the visible blocks.
    pub fn selection_rects_for_visible(
        &self,
        selection: &DocumentSelection,
        visible_blocks: &[VisibleBlock<B>],
    ) -> Vec<Rect> {
        let mut rects = Vec::new();
        match selection {
            DocumentSelection::Text { anchor, focus, .. } => {
                if anchor == focus {
                    return rects;
                }
                let Some(anchor_leaf_id) = anchor.leaf_block_id() else {
                    return rects;
                };
                let Some(focus_leaf_id) = focus.leaf_block_id() else {
                    return rects;
                };
                let Some(&anchor_order) = self.leaf_order.get(&anchor_leaf_id) else {
                    return rects;
                };
                let Some(&focus_order) = self.leaf_order.get(&focus_leaf_id) else {
                    return rects;
                };

                let ((start_order, start_offset), (end_order, end_offset)) =
                    if (anchor_order, anchor.offset) <= (focus_order, focus.offset) {
                        ((anchor_order, anchor.offset), (focus_order, focus.offset))
                    } else {
                        ((focus_order, focus.offset), (anchor_order, anchor.offset))
                    };

                let mut leaf_paragraphs = Vec::new();
                for visible in visible_blocks {
                    collect_leaf_paragraphs(
                        &visible.layout,
                        visible.x,
                        visible.y,
                        &mut leaf_paragraphs,
                    );
                }

                for leaf in leaf_paragraphs {
                    let Some(&leaf_order) = self.leaf_order.get(&leaf.block_id) else {
                        continue;
                    };
                    if leaf_order < start_order || leaf_order > end_order {
                        continue;
                    }
                    let block_len = leaf
                        .paragraph
                        .lines
                        .last()
                        .map(|line| line.doc_range.end)
                        .unwrap_or(0);
                    let range = if start_order == end_order {
                        start_offset..end_offset
                    } else if leaf_order == start_order {
                        start_offset..block_len
                    } else if leaf_order == end_order {
                        0..end_offset
                    } else {
                        0..block_len
                    };
                    let trailing = leaf_order < end_order;
                    for rect in leaf.paragraph.selection_rects(range, trailing) {
                        rects.push(rect.translate(leaf.origin_x, leaf.origin_y));
                    }
                }
            }
            DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                focus_cell,
            } => {
                let row_min = anchor_cell.0.min(focus_cell.0);
                let row_max = anchor_cell.0.max(focus_cell.0);
                let col_min = anchor_cell.1.min(focus_cell.1);
                let col_max = anchor_cell.1.max(focus_cell.1);
                if let Some((table, origin)) = visible_blocks.iter().find_map(|visible| {
                    find_table_layout(&visible.layout, *table_id, Point::new(visible.x, visible.y))
                }) {
                    let (mut top, mut bottom, mut left, mut right) = (row_min, row_max, col_min, col_max);
                    loop {
                        let previous = (top, bottom, left, right);
                        for cell in &table.cells {
                            let last_row = cell.row + usize::from(cell.rowspan.max(1)) - 1;
                            let last_col = cell.col + usize::from(cell.colspan.max(1)) - 1;
                            if last_row >= top && cell.row <= bottom && last_col >= left && cell.col <= right {
                                top = top.min(cell.row);
                                bottom = bottom.max(last_row);
                                left = left.min(cell.col);
                                right = right.max(last_col);
                            }
                        }
                        if previous == (top, bottom, left, right) {
                            break;
                        }
                    }
                    rects.extend(table.cells.iter().filter(|cell| {
                        (top..=bottom).contains(&cell.row) && (left..=right).contains(&cell.col)
                    }).map(|cell| cell.rect.translate(origin.x, origin.y)));
                }
            }
        }
        rects
    }

    pub fn caret_rect_in_visible(
        &self,
        pos: &DocPos,
        affinity: Affinity,
        visible_blocks: &[VisibleBlock<B>],
    ) -> Option<Rect> {
        let top_id = pos.top_level_block_id()?;
        let leaf_id = pos.leaf_block_id()?;
        let visible = visible_blocks.iter().find(|entry| entry.id == top_id)?;
        let mut leaf_paragraphs = Vec::new();
        collect_leaf_paragraphs(&visible.layout, visible.x, visible.y, &mut leaf_paragraphs);
        let target = leaf_paragraphs
            .into_iter()
            .find(|leaf| leaf.block_id == leaf_id)?;
        Some(
            target
                .paragraph
                .caret_rect(pos.offset, affinity)
                .translate(target.origin_x, target.origin_y),
        )
    }

    /// Returns the canvas-space caret rectangle for `pos` using the latest cached block layout
    /// (without requiring a [`TextShaper`]).
    pub fn cached_caret_rect(
        &self,
        pos: &DocPos,
        affinity: Affinity,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
    ) -> Option<Rect> {
        let top_id = pos.top_level_block_id()?;
        let leaf_id = pos.leaf_block_id()?;
        let top_index = doc.top_level_index(top_id)?;
        let top_block = doc.blocks.get(top_index)?;
        let layout = self.cache.get_latest(top_id)?;
        let (block_x, _) =
            resolve_block_horizontal_span(viewport_width, &doc.config, top_block.width_mode, zoom);
        let block_y = self.block_y(top_index);

        let mut leaf_paragraphs = Vec::new();
        collect_leaf_paragraphs(&layout, block_x, block_y, &mut leaf_paragraphs);
        let target = leaf_paragraphs
            .into_iter()
            .find(|leaf| leaf.block_id == leaf_id)?;
        Some(
            target
                .paragraph
                .caret_rect(pos.offset, affinity)
                .translate(target.origin_x, target.origin_y),
        )
    }

    /// Returns the visual line start or end byte offset and affinity for `pos` using cached layout.
    pub fn cached_line_boundary(
        &self,
        pos: &DocPos,
        affinity: Affinity,
        to_end: bool,
    ) -> Option<(usize, Affinity)> {
        let top_id = pos.top_level_block_id()?;
        let leaf_id = pos.leaf_block_id()?;
        let layout = self.cache.get_latest(top_id)?;
        let mut leaf_paragraphs = Vec::new();
        collect_leaf_paragraphs(&layout, 0.0, 0.0, &mut leaf_paragraphs);
        let target = leaf_paragraphs
            .into_iter()
            .find(|leaf| leaf.block_id == leaf_id)?;
        if target.paragraph.lines.is_empty() {
            return Some((0, Affinity::Downstream));
        }
        let line_index = target.paragraph.line_index_for_pos(pos.offset, affinity);
        let line = target.paragraph.lines.get(line_index)?;
        if to_end {
            let end_affinity = if line.soft_wrapped {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            };
            Some((line.doc_range.end, end_affinity))
        } else {
            Some((line.doc_range.start, Affinity::Downstream))
        }
    }

    /// Moves the caret vertically by `delta_lines` (`-1` up, `+1` down), preserving `goal_x`
    /// across soft-wrapped lines and visible leaf block boundaries.
    #[allow(clippy::too_many_arguments)]
    pub fn cached_vertical_move(
        &self,
        pos: &DocPos,
        affinity: Affinity,
        goal_x: Option<f32>,
        delta_lines: isize,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
    ) -> Option<(DocPos, Affinity, f32)> {
        if delta_lines == 0 {
            return None;
        }
        let top_id = pos.top_level_block_id()?;
        let leaf_id = pos.leaf_block_id()?;
        let top_index = doc.top_level_index(top_id)?;
        let top_block = doc.blocks.get(top_index)?;
        let (block_x, _) =
            resolve_block_horizontal_span(viewport_width, &doc.config, top_block.width_mode, zoom);
        let block_y = self.block_y(top_index);

        let effective_goal_x = goal_x.unwrap_or_else(|| {
            self.cached_caret_rect(pos, affinity, doc, viewport_width, zoom)
                .map(|rect| rect.x)
                .unwrap_or(block_x)
        });

        // 1. Try moving inside the current leaf paragraph first.
        if let Some(layout) = self.cache.get_latest(top_id) {
            let mut leaf_paragraphs = Vec::new();
            collect_leaf_paragraphs(&layout, block_x, block_y, &mut leaf_paragraphs);
            if let Some(current_leaf) = leaf_paragraphs.into_iter().find(|leaf| leaf.block_id == leaf_id)
                && let Some((next_offset, next_affinity)) = current_leaf.paragraph.vertical_step(
                    pos.offset,
                    affinity,
                    effective_goal_x - current_leaf.origin_x,
                    delta_lines,
                )
            {
                let mut next_pos = pos.clone();
                next_pos.offset = next_offset;
                return Some((next_pos, next_affinity, effective_goal_x));
            }
        }

        // 2. Cross into the previous or next visible leaf block.
        let visible_leaves = doc.collect_visible_leaf_blocks();
        let current_leaf_index = visible_leaves.iter().position(|block| block.id == leaf_id)?;
        let target_leaf_index = if delta_lines < 0 {
            current_leaf_index.checked_sub(1)?
        } else {
            let next = current_leaf_index + 1;
            if next < visible_leaves.len() {
                next
            } else {
                return None;
            }
        };
        let target_leaf = visible_leaves.get(target_leaf_index)?;
        let target_path = doc.path_for_block(target_leaf.id)?;
        let target_top_id = target_path.first().map(|segment| match *segment {
            PathSegment::Block(id) => id,
            PathSegment::TableCell { table_id, .. } => table_id,
        })?;
        let target_top_index = doc.top_level_index(target_top_id)?;
        let target_top_block = doc.blocks.get(target_top_index)?;
        let (target_block_x, _) = resolve_block_horizontal_span(
            viewport_width,
            &doc.config,
            target_top_block.width_mode,
            zoom,
        );
        let target_block_y = self.block_y(target_top_index);

        if let Some(target_layout) = self.cache.get_latest(target_top_id) {
            let mut leaf_paragraphs = Vec::new();
            collect_leaf_paragraphs(
                &target_layout,
                target_block_x,
                target_block_y,
                &mut leaf_paragraphs,
            );
            if let Some(leaf_ref) = leaf_paragraphs
                .into_iter()
                .find(|leaf| leaf.block_id == target_leaf.id)
                && let Some(line) = if delta_lines < 0 {
                    leaf_ref.paragraph.lines.last()
                } else {
                    leaf_ref.paragraph.lines.first()
                }
            {
                let hit = leaf_ref.paragraph.hit_test(Point::new(
                    effective_goal_x - leaf_ref.origin_x,
                    line.rect.center().y,
                ));
                return Some((
                    DocPos {
                        path: target_path,
                        offset: hit.offset,
                    },
                    hit.affinity,
                    effective_goal_x,
                ));
            }
        }

        // Fallback when the adjacent block is off-screen and not yet laid out.
        let fallback_offset = if delta_lines < 0 {
            target_leaf
                .kind
                .inline_content()
                .map(|inline| inline.len())
                .unwrap_or(0)
        } else {
            0
        };
        Some((
            DocPos {
                path: target_path,
                offset: fallback_offset,
            },
            Affinity::Downstream,
            effective_goal_x,
        ))
    }

    /// Computes canvas-space bounding rectangle for UTF-8 byte `range` inside `block_id`
    /// (used to position platform IME candidate popups).
    #[allow(clippy::too_many_arguments)]
    pub fn range_bounds_in_block<S: TextShaper<Backend = B>>(
        &mut self,
        block_id: BlockId,
        range: std::ops::Range<usize>,
        doc: &Document,
        viewport_width: f32,
        zoom: f32,
        shaper: &mut S,
    ) -> Option<Rect> {
        let path = doc.path_for_block(block_id)?;
        let top_id = path.first().map(|segment| match *segment {
            PathSegment::Block(id) => id,
            PathSegment::TableCell { table_id, .. } => table_id,
        })?;
        let top_index = doc.top_level_index(top_id)?;
        let top_block = doc.blocks.get(top_index)?;
        let (block_x, max_width) =
            resolve_block_horizontal_span(viewport_width, &doc.config, top_block.width_mode, zoom);
        let block_y = self.block_y(top_index);
        let marker = self.list_markers.get(top_index).and_then(|m| m.as_deref());
        let layout = self.cache.get_or_layout(
            top_block,
            marker,
            &doc.stylesheet,
            &doc.config,
            max_width,
            zoom,
            shaper,
        );

        let mut leaf_paragraphs = Vec::new();
        collect_leaf_paragraphs(&layout, block_x, block_y, &mut leaf_paragraphs);
        let leaf = leaf_paragraphs
            .into_iter()
            .find(|entry| entry.block_id == block_id)?;

        if range.is_empty() {
            return Some(
                leaf.paragraph
                    .caret_rect(range.start, Affinity::Downstream)
                    .translate(leaf.origin_x, leaf.origin_y),
            );
        }

        let rects = leaf.paragraph.selection_rects(range.clone(), false);
        if let Some(first) = rects.first() {
            let mut union = *first;
            for rect in rects.iter().skip(1) {
                let min_x = union.x.min(rect.x);
                let min_y = union.y.min(rect.y);
                let max_x = union.right().max(rect.right());
                let max_y = union.bottom().max(rect.bottom());
                union = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y);
            }
            Some(union.translate(leaf.origin_x, leaf.origin_y))
        } else {
            Some(
                leaf.paragraph
                    .caret_rect(range.start, Affinity::Downstream)
                    .translate(leaf.origin_x, leaf.origin_y),
            )
        }
    }

    /// Returns the top canvas Y offset of the top-level block containing `pos`.
    pub fn block_canvas_y(&self, pos: &DocPos, doc: &Document) -> Option<f32> {
        let top_id = pos.top_level_block_id()?;
        let top_index = doc.top_level_index(top_id)?;
        Some(self.block_y(top_index))
    }

    fn rebuild_height_index(&mut self) {
        self.height_index = HeightIndex::new(self.metrics.len());
        for i in 0..self.metrics.len() {
            let slot = self.compute_slot_height(i);
            self.height_index.set_slot(i, slot);
        }
    }

    fn update_slot_and_prev_neighbor(&mut self, idx: usize) {
        let slot = self.compute_slot_height(idx);
        self.height_index.set_slot(idx, slot);
        if let Some(prev) = self.prev_visible(idx) {
            let prev_slot = self.compute_slot_height(prev);
            self.height_index.set_slot(prev, prev_slot);
        }
    }

    fn compute_slot_height(&self, idx: usize) -> f32 {
        if !self.is_visible(idx) {
            return 0.0;
        }
        let m = self.metrics[idx];
        let gap = if let Some(next) = self.next_visible(idx) {
            m.space_after.max(self.metrics[next].space_before)
        } else {
            0.0
        };
        m.content_height + gap
    }

    fn next_visible(&self, idx: usize) -> Option<usize> {
        ((idx + 1)..self.visibility.len()).find(|&i| self.visibility[i])
    }

    fn prev_visible(&self, idx: usize) -> Option<usize> {
        (0..idx).rev().find(|&i| self.visibility[i])
    }
}

/// Canvas-space placement of a visible text-bearing leaf paragraph (top-level or nested).
pub struct VisibleLeafParagraph<'layout, B: GlyphBackend> {
    pub block_id: BlockId,
    pub origin_x: f32,
    pub origin_y: f32,
    pub paragraph: &'layout crate::line::ParagraphLayout<B>,
}

/// Collects all visible text-bearing leaf paragraphs (including blockquote and table cell children)
/// with their canvas-space content origins.
pub fn collect_visible_leaf_paragraphs<'layout, B: GlyphBackend>(
    visible_blocks: &'layout [VisibleBlock<B>],
) -> Vec<VisibleLeafParagraph<'layout, B>> {
    let mut leaves = Vec::new();
    for visible in visible_blocks {
        collect_leaf_paragraphs(&visible.layout, visible.x, visible.y, &mut leaves);
    }
    leaves
}

fn collect_leaf_paragraphs<'layout, B: GlyphBackend>(
    layout: &'layout BlockLayout<B>,
    origin_x: f32,
    origin_y: f32,
    out: &mut Vec<VisibleLeafParagraph<'layout, B>>,
) {
    if let Some(paragraph) = layout.paragraph() {
        let content_origin = layout.content_origin();
        out.push(VisibleLeafParagraph {
            block_id: layout.id,
            origin_x: origin_x + content_origin.x,
            origin_y: origin_y + content_origin.y,
            paragraph,
        });
        return;
    }
    if let Some(children) = layout.children() {
        for child in children {
            collect_leaf_paragraphs(
                &child.layout,
                origin_x + child.x,
                origin_y + child.y,
                out,
            );
        }
    } else if let Some(table) = layout.table_layout() {
        let content_origin = layout.content_origin();
        for cell in &table.cells {
            for child in &cell.children {
                collect_leaf_paragraphs(
                    &child.layout,
                    origin_x + content_origin.x + cell.rect.x + child.x,
                    origin_y + content_origin.y + cell.rect.y + child.y,
                    out,
                );
            }
        }
    }
}

fn find_leaf_paragraph<B: GlyphBackend>(
    layout: &BlockLayout<B>,
    id: BlockId,
    origin: Point,
) -> Option<(&ParagraphLayout<B>, Point)> {
    let content = layout.content_origin();
    if layout.id == id {
        return Some((layout.paragraph()?, Point::new(origin.x + content.x, origin.y + content.y)));
    }
    if let Some(children) = layout.children() {
        return children.iter().find_map(|child| {
            find_leaf_paragraph(&child.layout, id, Point::new(origin.x + child.x, origin.y + child.y))
        });
    }
    layout.table_layout()?.cells.iter().find_map(|cell| {
        cell.children.iter().find_map(|child| {
            find_leaf_paragraph(&child.layout, id, Point::new(
                origin.x + content.x + cell.rect.x + child.x,
                origin.y + content.y + cell.rect.y + child.y,
            ))
        })
    })
}

fn find_table_layout<B: GlyphBackend>(
    layout: &BlockLayout<B>,
    id: BlockId,
    origin: Point,
) -> Option<(&crate::block::TableLayout<B>, Point)> {
    if let Some(children) = layout.children() {
        return children.iter().find_map(|child| {
            find_table_layout(&child.layout, id, Point::new(origin.x + child.x, origin.y + child.y))
        });
    }
    let table = layout.table_layout()?;
    let content = layout.content_origin();
    let origin = Point::new(origin.x + content.x, origin.y + content.y);
    if layout.id == id {
        return Some((table, origin));
    }
    table.cells.iter().find_map(|cell| {
        cell.children.iter().find_map(|child| {
            find_table_layout(&child.layout, id, Point::new(
                origin.x + cell.rect.x + child.x, origin.y + cell.rect.y + child.y,
            ))
        })
    })
}

fn hit_test_layout_recursive<B: GlyphBackend>(
    layout: &BlockLayout<B>,
    local: Point,
    path_prefix: &mut smallvec::SmallVec<[PathSegment; 4]>,
) -> Option<CanvasHitResult> {
    if let Some(chrome_hit) = hit_test_block_chrome(layout, local, layout.id) {
        return Some(chrome_hit);
    }
    if let Some(hit) = layout.hit_test_local(local) {
        let mut path = path_prefix.clone();
        path.push(PathSegment::Block(layout.id));
        return Some(CanvasHitResult {
            pos: DocPos {
                path,
                offset: hit.offset,
            },
            affinity: hit.affinity,
            hit,
            toggled_heading: None,
            toggled_checklist: None,
            code_block_copy: None,
        });
    }
    if let Some(children) = layout.children() {
        if let Some(child) = nearest_child_for_y(children, local.y) {
            let child_local = Point::new(local.x - child.x, local.y - child.y);
            path_prefix.push(PathSegment::Block(layout.id));
            let result = hit_test_layout_recursive(&child.layout, child_local, path_prefix);
            path_prefix.pop();
            return result;
        }
        return None;
    }
    if let Some(table) = layout.table_layout() {
        let origin = layout.content_origin();
        let table_local = Point::new(local.x - origin.x, local.y - origin.y);
        let target_cell = table
            .cells
            .iter()
            .find(|cell| cell.rect.contains(table_local))
            .or_else(|| {
                table.cells.iter().min_by(|left, right| {
                    let left_dist = rect_distance_sq(left.rect, table_local);
                    let right_dist = rect_distance_sq(right.rect, table_local);
                    left_dist.total_cmp(&right_dist)
                })
            });
        if let Some(cell) = target_cell {
            let cell_local = Point::new(table_local.x - cell.rect.x, table_local.y - cell.rect.y);
            if let Some(child) = nearest_child_for_y(&cell.children, cell_local.y) {
                let child_local = Point::new(cell_local.x - child.x, cell_local.y - child.y);
                path_prefix.push(PathSegment::TableCell {
                    table_id: layout.id,
                    row: cell.row,
                    col: cell.cell_index,
                });
                let result = hit_test_layout_recursive(&child.layout, child_local, path_prefix);
                path_prefix.pop();
                return result;
            }
        }
    }
    None
}

fn nearest_child_for_y<B: GlyphBackend>(
    children: &[crate::block::PlacedChildBlock<B>],
    local_y: f32,
) -> Option<&crate::block::PlacedChildBlock<B>> {
    if children.is_empty() {
        return None;
    }
    for child in children {
        if local_y >= child.y && local_y <= child.y + child.layout.height {
            return Some(child);
        }
    }
    if local_y < children[0].y {
        children.first()
    } else {
        children.last()
    }
}

fn hit_test_block_chrome<B: GlyphBackend>(
    layout: &BlockLayout<B>,
    local: Point,
    block_id: BlockId,
) -> Option<CanvasHitResult> {
    let empty_hit = HitResult {
        offset: 0,
        affinity: Affinity::Downstream,
        inline_object: None,
        link: None,
    };
    match &layout.kind {
        crate::block::BlockLayoutKind::Heading { chevron_rect, .. }
            if chevron_rect.contains(local) =>
        {
            Some(CanvasHitResult {
                pos: DocPos::top_level(block_id, 0),
                affinity: Affinity::Downstream,
                hit: empty_hit,
                toggled_heading: Some(block_id),
                toggled_checklist: None,
                code_block_copy: None,
            })
        }
        crate::block::BlockLayoutKind::ListItem {
            checkbox_rect: Some(rect),
            ..
        } => {
            // Expand the checkbox hit target slightly so small checkboxes click comfortably.
            let expanded = Rect::new(
                rect.x - 3.0,
                rect.y - 3.0,
                rect.width + 6.0,
                rect.height + 6.0,
            );
            if expanded.contains(local) {
                Some(CanvasHitResult {
                    pos: DocPos::top_level(block_id, 0),
                    affinity: Affinity::Downstream,
                    hit: empty_hit,
                    toggled_heading: None,
                    toggled_checklist: Some(block_id),
                    code_block_copy: None,
                })
            } else {
                None
            }
        }
        crate::block::BlockLayoutKind::CodeBlock {
            copy_button_rect, ..
        } if copy_button_rect.contains(local) => Some(CanvasHitResult {
            pos: DocPos::top_level(block_id, 0),
            affinity: Affinity::Downstream,
            hit: empty_hit,
            toggled_heading: None,
            toggled_checklist: None,
            code_block_copy: Some(block_id),
        }),
        _ => None,
    }
}

fn rect_distance_sq(rect: Rect, point: Point) -> f32 {
    let clamped_x = point.x.clamp(rect.x, rect.right());
    let clamped_y = point.y.clamp(rect.y, rect.bottom());
    let dx = point.x - clamped_x;
    let dy = point.y - clamped_y;
    dx * dx + dy * dy
}

fn estimate_block_metrics(
    block: &BlockNode,
    sheet: &rich_model::StyleSheet,
    max_width: f32,
    zoom: f32,
) -> BlockMetrics {
    let space_before = pt_to_px(block.style.resolved_space_before(sheet), zoom);
    let space_after = pt_to_px(block.style.resolved_space_after(sheet), zoom);
    let base_line = pt_to_px(16.0, zoom);

    let est_height = match &block.kind {
        BlockKind::Text { content, .. } => {
            if let Some(heading) = block.kind.heading_data() {
                let scale = match heading.level {
                    1 => 2.0,
                    2 => 1.6,
                    3 => 1.35,
                    _ => 1.15,
                };
                let chars_per_line = (max_width / pt_to_px(6.5 * scale, zoom)).max(8.0);
                let lines = ((content.len().max(1) as f32) / chars_per_line)
                    .ceil()
                    .max(1.0);
                lines * base_line * scale
            } else if block.kind.code_block_data().is_some() {
                let lines = content.text.lines().count().max(1) as f32;
                lines * pt_to_px(15.0, zoom) + pt_to_px(24.0, zoom)
            } else {
                let chars_per_line = (max_width / pt_to_px(6.5, zoom)).max(12.0);
                let lines = ((content.len().max(1) as f32) / chars_per_line)
                    .ceil()
                    .max(1.0);
                lines * base_line
            }
        }
        BlockKind::Container { children, .. } => {
            (children.len().max(1) as f32) * base_line * 1.4
        }
        BlockKind::Grid { table, .. } => (table.rows.len().max(1) as f32) * pt_to_px(30.0, zoom),
        BlockKind::Leaf { .. } => {
            if block.kind.is_horizontal_rule() {
                pt_to_px(12.0, zoom)
            } else {
                pt_to_px(44.0, zoom)
            }
        }
    };

    BlockMetrics {
        content_height: est_height,
        space_before,
        space_after,
        measured: false,
    }
}
