use crate::inline::InlineContent;
use crate::payload::CustomPayload;
use crate::selection::{DocPos, PathSegment};
use crate::style::{
    BlockStyle, BlockWidth, DocumentLayoutConfig, Edges, RgbaColor, SharedStr, StyleId, StyleSheet,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::sync::Arc;

/// Built-in semantic kind identifiers for [`BlockKind`].
pub mod block_kinds {
    pub const PARAGRAPH: &str = "paragraph";
    pub const HEADING: &str = "heading";
    pub const LIST_ITEM: &str = "list_item";
    pub const CODE_BLOCK: &str = "code_block";
    pub const BLOCKQUOTE: &str = "blockquote";
    pub const TABLE: &str = "table";
    pub const HORIZONTAL_RULE: &str = "horizontal_rule";
}

/// Built-in typed metadata for [`block_kinds::HEADING`] text blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadingData {
    pub level: u8,
    pub collapsed: bool,
}

/// Built-in typed metadata for [`block_kinds::LIST_ITEM`] text blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItemData {
    pub list_id: ListId,
    pub kind: ListKind,
    pub indent_level: u8,
    pub checked: Option<bool>,
}

/// Built-in typed metadata for [`block_kinds::CODE_BLOCK`] text blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBlockData {
    pub language: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockId(pub u64);

/// Identifies a block list within the document tree (top-level or nested inside a container).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContainerTarget {
    /// Top-level `Document::blocks`.
    Root,
    /// Children of any [`BlockKind::Container`] (blockquote or custom container) with the given [`BlockId`].
    Container(BlockId),
    /// Children of a specific cell inside a [`BlockKind::Grid`].
    TableCell {
        table_id: BlockId,
        row: usize,
        col: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberStyle {
    Decimal,
    LowerAlpha,
    LowerRoman,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListKind {
    Bullet,
    Ordered(NumberStyle),
    Checklist,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColumnSpec {
    FixedPt(f32),
    Ratio(f32),
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableBorderStyle {
    pub width_pt: f32,
    pub color: RgbaColor,
}

impl Default for TableBorderStyle {
    fn default() -> Self {
        Self {
            width_pt: 0.75,
            color: RgbaColor::rgb(0xd0, 0xd7, 0xde),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellVAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    pub id: BlockId,
    pub colspan: u16,
    pub rowspan: u16,
    pub background: Option<RgbaColor>,
    pub padding_pt: Edges,
    pub valign: CellVAlign,
    pub blocks: Vec<BlockNode>,
}

impl TableCell {
    pub fn simple(id: BlockId, blocks: Vec<BlockNode>) -> Self {
        Self {
            id,
            colspan: 1,
            rowspan: 1,
            background: None,
            padding_pt: Edges::xy(8.0, 6.0),
            valign: CellVAlign::Top,
            blocks,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub is_header: bool,
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableBlock {
    pub columns: Vec<ColumnSpec>,
    pub rows: Vec<TableRow>,
    pub border_style: TableBorderStyle,
}

/// Visual grid placement for one cell inside a [`TableBlock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableCellPlacement {
    pub row_index: usize,
    pub cell_index: usize,
    pub visual_col: usize,
    pub colspan: usize,
    pub rowspan: usize,
}

impl TableBlock {
    /// Returns the number of visual grid columns spanned by this table.
    pub fn grid_col_count(&self) -> usize {
        let row_count = self.rows.len();
        let mut occupied: Vec<Vec<bool>> = Vec::with_capacity(row_count);
        let mut max_cols = self.columns.len();

        for (row_index, row) in self.rows.iter().enumerate() {
            if occupied.len() <= row_index {
                occupied.push(Vec::new());
            }
            let mut visual_col = 0usize;
            for cell in &row.cells {
                while occupied
                    .get(row_index)
                    .and_then(|slots| slots.get(visual_col))
                    .copied()
                    .unwrap_or(false)
                {
                    visual_col += 1;
                }
                let colspan = cell.colspan.max(1) as usize;
                let rowspan = (cell.rowspan.max(1) as usize).min(row_count.saturating_sub(row_index));
                let end_col = visual_col + colspan;
                max_cols = max_cols.max(end_col);
                for span_row in row_index..(row_index + rowspan) {
                    while occupied.len() <= span_row {
                        occupied.push(Vec::new());
                    }
                    if occupied[span_row].len() < end_col {
                        occupied[span_row].resize(end_col, false);
                    }
                    for slot in &mut occupied[span_row][visual_col..end_col] {
                        *slot = true;
                    }
                }
                visual_col = end_col;
            }
        }
        max_cols.max(1)
    }

    /// Computes visual grid placements for every stored [`TableCell`] and an occupancy matrix
    /// mapping `(row, visual_col)` to the index into the returned placements vector.
    pub fn resolve_grid(&self) -> (usize, Vec<TableCellPlacement>, Vec<Vec<Option<usize>>>) {
        let row_count = self.rows.len();
        let col_count = self.grid_col_count();
        let mut occupancy = vec![vec![None; col_count]; row_count];
        let mut placements = Vec::new();

        for (row_index, row) in self.rows.iter().enumerate() {
            let mut visual_col = 0usize;
            for (cell_index, cell) in row.cells.iter().enumerate() {
                while visual_col < col_count && occupancy[row_index][visual_col].is_some() {
                    visual_col += 1;
                }
                if visual_col >= col_count {
                    break;
                }
                let colspan = (cell.colspan.max(1) as usize).min(col_count - visual_col);
                let rowspan = (cell.rowspan.max(1) as usize).min(row_count - row_index);
                let placement_index = placements.len();
                placements.push(TableCellPlacement {
                    row_index,
                    cell_index,
                    visual_col,
                    colspan,
                    rowspan,
                });
                for row_slots in occupancy.iter_mut().skip(row_index).take(rowspan) {
                    for slot in row_slots.iter_mut().skip(visual_col).take(colspan) {
                        *slot = Some(placement_index);
                    }
                }
                visual_col += colspan;
            }
        }

        (col_count, placements, occupancy)
    }

    /// Returns the [`TableCellPlacement`] for the stored `(row_index, cell_index)` cell.
    pub fn placement_for_stored_cell(
        &self,
        row_index: usize,
        cell_index: usize,
    ) -> Option<TableCellPlacement> {
        let (_, placements, _) = self.resolve_grid();
        placements
            .into_iter()
            .find(|placement| placement.row_index == row_index && placement.cell_index == cell_index)
    }
}

/// Structural block shape with an open semantic `kind` identifier and typed [`CustomPayload`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BlockKind {
    /// Text-bearing leaf block (paragraphs, headings, list items, code blocks, and custom text blocks).
    Text {
        kind: SharedStr,
        #[serde(default)]
        data: CustomPayload,
        content: InlineContent,
    },
    /// Vertical container of editable child blocks (blockquotes, callouts, toggles, admonitions).
    Container {
        kind: SharedStr,
        #[serde(default)]
        data: CustomPayload,
        children: Vec<BlockNode>,
    },
    /// Two-dimensional table/grid container.
    Grid {
        kind: SharedStr,
        #[serde(default)]
        data: CustomPayload,
        table: TableBlock,
    },
    /// Atomic non-text leaf block (horizontal rules, embeds, charts, custom widgets).
    Leaf {
        kind: SharedStr,
        #[serde(default)]
        data: CustomPayload,
    },
}

impl BlockKind {
    pub fn paragraph(content: InlineContent) -> Self {
        Self::Text {
            kind: Arc::from(block_kinds::PARAGRAPH),
            data: CustomPayload::empty(),
            content,
        }
    }

    pub fn heading(level: u8, collapsed: bool, content: InlineContent) -> Self {
        Self::Text {
            kind: Arc::from(block_kinds::HEADING),
            data: CustomPayload::typed(HeadingData {
                level: level.clamp(1, 6),
                collapsed,
            }),
            content,
        }
    }

    pub fn list_item(
        list_id: ListId,
        kind: ListKind,
        indent_level: u8,
        checked: Option<bool>,
        content: InlineContent,
    ) -> Self {
        Self::Text {
            kind: Arc::from(block_kinds::LIST_ITEM),
            data: CustomPayload::typed(ListItemData {
                list_id,
                kind,
                indent_level,
                checked,
            }),
            content,
        }
    }

    pub fn code_block(language: Option<String>, content: InlineContent) -> Self {
        Self::Text {
            kind: Arc::from(block_kinds::CODE_BLOCK),
            data: CustomPayload::typed(CodeBlockData { language }),
            content,
        }
    }

    pub fn blockquote(children: Vec<BlockNode>) -> Self {
        Self::Container {
            kind: Arc::from(block_kinds::BLOCKQUOTE),
            data: CustomPayload::empty(),
            children,
        }
    }

    pub fn table(table: TableBlock) -> Self {
        Self::Grid {
            kind: Arc::from(block_kinds::TABLE),
            data: CustomPayload::empty(),
            table,
        }
    }

    pub fn horizontal_rule() -> Self {
        Self::Leaf {
            kind: Arc::from(block_kinds::HORIZONTAL_RULE),
            data: CustomPayload::empty(),
        }
    }

    pub fn text(
        kind: impl Into<SharedStr>,
        data: CustomPayload,
        content: InlineContent,
    ) -> Self {
        Self::Text {
            kind: kind.into(),
            data,
            content,
        }
    }

    pub fn container(
        kind: impl Into<SharedStr>,
        data: CustomPayload,
        children: Vec<BlockNode>,
    ) -> Self {
        Self::Container {
            kind: kind.into(),
            data,
            children,
        }
    }

    pub fn grid(kind: impl Into<SharedStr>, data: CustomPayload, table: TableBlock) -> Self {
        Self::Grid {
            kind: kind.into(),
            data,
            table,
        }
    }

    pub fn leaf(kind: impl Into<SharedStr>, data: CustomPayload) -> Self {
        Self::Leaf {
            kind: kind.into(),
            data,
        }
    }

    pub fn semantic_kind(&self) -> &str {
        match self {
            Self::Text { kind, .. }
            | Self::Container { kind, .. }
            | Self::Grid { kind, .. }
            | Self::Leaf { kind, .. } => kind.as_ref(),
        }
    }

    pub fn data(&self) -> &CustomPayload {
        match self {
            Self::Text { data, .. }
            | Self::Container { data, .. }
            | Self::Grid { data, .. }
            | Self::Leaf { data, .. } => data,
        }
    }

    pub fn data_mut(&mut self) -> &mut CustomPayload {
        match self {
            Self::Text { data, .. }
            | Self::Container { data, .. }
            | Self::Grid { data, .. }
            | Self::Leaf { data, .. } => data,
        }
    }

    pub fn inline_content(&self) -> Option<&InlineContent> {
        match self {
            Self::Text { content, .. } => Some(content),
            Self::Container { .. } | Self::Grid { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn inline_content_mut(&mut self) -> Option<&mut InlineContent> {
        match self {
            Self::Text { content, .. } => Some(content),
            Self::Container { .. } | Self::Grid { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn children(&self) -> Option<&[BlockNode]> {
        match self {
            Self::Container { children, .. } => Some(children),
            Self::Text { .. } | Self::Grid { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<BlockNode>> {
        match self {
            Self::Container { children, .. } => Some(children),
            Self::Text { .. } | Self::Grid { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn table_block(&self) -> Option<&TableBlock> {
        match self {
            Self::Grid { table, .. } => Some(table),
            Self::Text { .. } | Self::Container { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn table_block_mut(&mut self) -> Option<&mut TableBlock> {
        match self {
            Self::Grid { table, .. } => Some(table),
            Self::Text { .. } | Self::Container { .. } | Self::Leaf { .. } => None,
        }
    }

    pub fn is_paragraph(&self) -> bool {
        matches!(self, Self::Text { kind, .. } if kind.as_ref() == block_kinds::PARAGRAPH)
    }

    pub fn is_blockquote(&self) -> bool {
        matches!(self, Self::Container { kind, .. } if kind.as_ref() == block_kinds::BLOCKQUOTE)
    }

    pub fn is_horizontal_rule(&self) -> bool {
        matches!(self, Self::Leaf { kind, .. } if kind.as_ref() == block_kinds::HORIZONTAL_RULE)
    }

    pub fn is_builtin_table(&self) -> bool {
        matches!(self, Self::Grid { kind, .. } if kind.as_ref() == block_kinds::TABLE)
    }

    pub fn heading_data(&self) -> Option<HeadingData> {
        match self {
            Self::Text { kind, data, .. } if kind.as_ref() == block_kinds::HEADING => {
                data.as_typed::<HeadingData>().map(|value| *value)
            }
            _ => None,
        }
    }

    pub fn list_item_data(&self) -> Option<ListItemData> {
        match self {
            Self::Text { kind, data, .. } if kind.as_ref() == block_kinds::LIST_ITEM => {
                data.as_typed::<ListItemData>().map(|value| *value)
            }
            _ => None,
        }
    }

    pub fn code_block_data(&self) -> Option<Cow<'_, CodeBlockData>> {
        match self {
            Self::Text { kind, data, .. } if kind.as_ref() == block_kinds::CODE_BLOCK => {
                data.as_typed::<CodeBlockData>()
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockNode {
    pub id: BlockId,
    pub version: u64,
    pub width_mode: BlockWidth,
    pub kind: BlockKind,
    pub style: BlockStyle,
}

impl BlockNode {
    pub fn new(id: BlockId, kind: BlockKind, style: BlockStyle) -> Self {
        Self {
            id,
            version: 1,
            width_mode: BlockWidth::Column,
            kind,
            style,
        }
    }

    pub fn with_width_mode(mut self, mode: BlockWidth) -> Self {
        self.width_mode = mode;
        self
    }

    pub fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub config: DocumentLayoutConfig,
    pub stylesheet: StyleSheet,
    pub blocks: Vec<BlockNode>,
    next_block_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            config: DocumentLayoutConfig::default(),
            stylesheet: StyleSheet::default(),
            blocks: Vec::new(),
            next_block_id: 1,
        }
    }

    pub fn alloc_block_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    pub fn push_block(&mut self, block: BlockNode) -> BlockId {
        let id = block.id;
        self.next_block_id = self.next_block_id.max(id.0 + 1);
        self.blocks.push(block);
        id
    }

    pub fn add_paragraph(&mut self, content: InlineContent, style: BlockStyle) -> BlockId {
        let id = self.alloc_block_id();
        self.push_block(BlockNode::new(id, BlockKind::paragraph(content), style))
    }

    pub fn add_heading(&mut self, level: u8, content: InlineContent) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle::named(StyleId::heading(level));
        self.push_block(BlockNode::new(
            id,
            BlockKind::heading(level.clamp(1, 6), false, content),
            style,
        ))
    }

    pub fn add_list_item(
        &mut self,
        list_id: ListId,
        kind: ListKind,
        indent_level: u8,
        checked: Option<bool>,
        content: InlineContent,
    ) -> BlockId {
        let id = self.alloc_block_id();
        let indent = 24.0 * (indent_level as f32 + 1.0);
        let mut style = BlockStyle::default().with_indent(indent, 0.0);
        style.space_after_pt = Some(4.0);
        self.push_block(BlockNode::new(
            id,
            BlockKind::list_item(list_id, kind, indent_level, checked, content),
            style,
        ))
    }

    pub fn add_code_block(&mut self, language: Option<String>, content: InlineContent) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle::named(StyleId::CODE_BLOCK);
        self.push_block(BlockNode::new(
            id,
            BlockKind::code_block(language, content),
            style,
        ))
    }

    pub fn add_blockquote(&mut self, children: Vec<BlockNode>) -> BlockId {
        let id = self.alloc_block_id();
        let mut style = BlockStyle::default();
        style.indent_start_pt = 16.0;
        style.space_before_pt = Some(6.0);
        style.space_after_pt = Some(10.0);
        self.push_block(BlockNode::new(id, BlockKind::blockquote(children), style))
    }

    pub fn add_horizontal_rule(&mut self) -> BlockId {
        let id = self.alloc_block_id();
        let mut style = BlockStyle::default();
        style.space_before_pt = Some(10.0);
        style.space_after_pt = Some(10.0);
        self.push_block(BlockNode::new(id, BlockKind::horizontal_rule(), style))
    }

    /// Appends a host-defined custom leaf block node.
    pub fn add_custom_block(
        &mut self,
        kind: impl Into<SharedStr>,
        data: CustomPayload,
    ) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle {
            space_before_pt: Some(6.0),
            space_after_pt: Some(8.0),
            ..BlockStyle::default()
        };
        self.push_block(BlockNode::new(id, BlockKind::leaf(kind, data), style))
    }

    /// Appends a host-defined custom text block node with editable [`InlineContent`].
    pub fn add_custom_text(
        &mut self,
        kind: impl Into<SharedStr>,
        data: CustomPayload,
        content: InlineContent,
        style: BlockStyle,
    ) -> BlockId {
        let id = self.alloc_block_id();
        self.push_block(BlockNode::new(
            id,
            BlockKind::text(kind, data, content),
            style,
        ))
    }

    /// Appends a host-defined custom container block node hosting editable child blocks.
    pub fn add_custom_container(
        &mut self,
        kind: impl Into<SharedStr>,
        data: CustomPayload,
        children: Vec<BlockNode>,
    ) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle {
            space_before_pt: Some(6.0),
            space_after_pt: Some(8.0),
            ..BlockStyle::default()
        };
        self.push_block(BlockNode::new(
            id,
            BlockKind::container(kind, data, children),
            style,
        ))
    }

    /// Allocates a [`ListId`] greater than any list currently in this document.
    pub fn alloc_list_id(&self) -> ListId {
        let max_id = max_list_id_in_slice(&self.blocks);
        ListId(max_id.saturating_add(1))
    }

    pub fn top_level_index(&self, id: BlockId) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    pub fn find_block(&self, id: BlockId) -> Option<&BlockNode> {
        find_in_slice(&self.blocks, id)
    }

    pub fn find_block_mut(&mut self, id: BlockId) -> Option<&mut BlockNode> {
        find_in_slice_mut(&mut self.blocks, id)
    }

    /// Collects all text-bearing leaf blocks (including container and table cell children) in reading order.
    pub fn collect_leaf_blocks(&self) -> Vec<&BlockNode> {
        let mut leaves = Vec::new();
        collect_leaves_in_slice(&self.blocks, &mut leaves);
        leaves
    }

    /// Collects text-bearing leaf blocks currently visible under collapsible heading rules.
    pub fn collect_visible_leaf_blocks(&self) -> Vec<&BlockNode> {
        let mut leaves = Vec::new();
        let mut collapsed_level: Option<u8> = None;

        for block in &self.blocks {
            let visible = if let Some(heading) = block.kind.heading_data() {
                if let Some(current_level) = collapsed_level {
                    if heading.level <= current_level {
                        collapsed_level = if heading.collapsed {
                            Some(heading.level)
                        } else {
                            None
                        };
                        true
                    } else {
                        false
                    }
                } else {
                    if heading.collapsed {
                        collapsed_level = Some(heading.level);
                    }
                    true
                }
            } else {
                collapsed_level.is_none()
            };
            if visible {
                collect_leaves_in_slice(std::slice::from_ref(block), &mut leaves);
            }
        }
        leaves
    }

    /// Finds the enclosing [`ContainerTarget`] and child index of `block_id`.
    pub fn locate_block(&self, block_id: BlockId) -> Option<(ContainerTarget, usize)> {
        locate_in_slice(&self.blocks, ContainerTarget::Root, block_id)
    }

    /// Resolves the full [`PathSegment`] chain leading to `block_id`.
    pub fn path_for_block(&self, block_id: BlockId) -> Option<SmallVec<[PathSegment; 4]>> {
        let mut path = SmallVec::new();
        if build_path_in_slice(&self.blocks, block_id, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    /// Constructs a [`DocPos`] targeting `block_id` at UTF-8 byte `offset`, preserving nested container paths.
    pub fn doc_pos_for_block(&self, block_id: BlockId, offset: usize) -> Option<DocPos> {
        let path = self.path_for_block(block_id)?;
        Some(DocPos { path, offset })
    }

    /// Returns an immutable slice of the blocks inside `target`.
    pub fn container_blocks(&self, target: ContainerTarget) -> Option<&[BlockNode]> {
        match target {
            ContainerTarget::Root => Some(&self.blocks),
            ContainerTarget::Container(container_id) => {
                let block = self.find_block(container_id)?;
                block.kind.children()
            }
            ContainerTarget::TableCell { table_id, row, col } => {
                let block = self.find_block(table_id)?;
                let table = block.kind.table_block()?;
                table
                    .rows
                    .get(row)
                    .and_then(|table_row| table_row.cells.get(col))
                    .map(|cell| cell.blocks.as_slice())
            }
        }
    }

    /// Returns a mutable reference to the block list inside `target`.
    pub fn container_blocks_mut(&mut self, target: ContainerTarget) -> Option<&mut Vec<BlockNode>> {
        match target {
            ContainerTarget::Root => Some(&mut self.blocks),
            ContainerTarget::Container(container_id) => {
                let block = self.find_block_mut(container_id)?;
                block.kind.children_mut()
            }
            ContainerTarget::TableCell { table_id, row, col } => {
                let block = self.find_block_mut(table_id)?;
                let table = block.kind.table_block_mut()?;
                table
                    .rows
                    .get_mut(row)
                    .and_then(|table_row| table_row.cells.get_mut(col))
                    .map(|cell| &mut cell.blocks)
            }
        }
    }

    pub fn toggle_heading_collapsed(&mut self, id: BlockId) -> Option<bool> {
        let block = self.find_block_mut(id)?;
        let mut heading = block.kind.heading_data()?;
        heading.collapsed = !heading.collapsed;
        let state = heading.collapsed;
        *block.kind.data_mut() = CustomPayload::typed(heading);
        block.bump_version();
        Some(state)
    }

    /// Toggles the checked state of a checklist list item, returning the new state.
    pub fn toggle_checklist_item(&mut self, id: BlockId) -> Option<bool> {
        let block = self.find_block_mut(id)?;
        let mut item = block.kind.list_item_data()?;
        let is_checked = item.checked.as_mut()?;
        *is_checked = !*is_checked;
        let state = *is_checked;
        *block.kind.data_mut() = CustomPayload::typed(item);
        block.bump_version();
        Some(state)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

fn collect_leaves_in_slice<'doc>(blocks: &'doc [BlockNode], out: &mut Vec<&'doc BlockNode>) {
    for block in blocks {
        match &block.kind {
            BlockKind::Container { children, .. } => collect_leaves_in_slice(children, out),
            BlockKind::Grid { table, .. } => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_leaves_in_slice(&cell.blocks, out);
                    }
                }
            }
            BlockKind::Text { .. } => out.push(block),
            BlockKind::Leaf { .. } => {}
        }
    }
}

fn max_list_id_in_slice(blocks: &[BlockNode]) -> u64 {
    let mut max_id = 0u64;
    for block in blocks {
        if let Some(item) = block.kind.list_item_data() {
            max_id = max_id.max(item.list_id.0);
        }
        match &block.kind {
            BlockKind::Container { children, .. } => {
                max_id = max_id.max(max_list_id_in_slice(children));
            }
            BlockKind::Grid { table, .. } => {
                for row in &table.rows {
                    for cell in &row.cells {
                        max_id = max_id.max(max_list_id_in_slice(&cell.blocks));
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    max_id
}

fn locate_in_slice(
    blocks: &[BlockNode],
    current_target: ContainerTarget,
    block_id: BlockId,
) -> Option<(ContainerTarget, usize)> {
    for (index, block) in blocks.iter().enumerate() {
        if block.id == block_id {
            return Some((current_target, index));
        }
        match &block.kind {
            BlockKind::Container { children, .. } => {
                if let Some(found) =
                    locate_in_slice(children, ContainerTarget::Container(block.id), block_id)
                {
                    return Some(found);
                }
            }
            BlockKind::Grid { table, .. } => {
                for (row_index, row) in table.rows.iter().enumerate() {
                    for (col_index, cell) in row.cells.iter().enumerate() {
                        let cell_target = ContainerTarget::TableCell {
                            table_id: block.id,
                            row: row_index,
                            col: col_index,
                        };
                        if let Some(found) = locate_in_slice(&cell.blocks, cell_target, block_id) {
                            return Some(found);
                        }
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    None
}

fn build_path_in_slice(
    blocks: &[BlockNode],
    block_id: BlockId,
    path: &mut SmallVec<[PathSegment; 4]>,
) -> bool {
    for block in blocks {
        if block.id == block_id {
            path.push(PathSegment::Block(block.id));
            return true;
        }
        match &block.kind {
            BlockKind::Container { children, .. } => {
                path.push(PathSegment::Block(block.id));
                if build_path_in_slice(children, block_id, path) {
                    return true;
                }
                path.pop();
            }
            BlockKind::Grid { table, .. } => {
                for (row_index, row) in table.rows.iter().enumerate() {
                    for (col_index, cell) in row.cells.iter().enumerate() {
                        path.push(PathSegment::TableCell {
                            table_id: block.id,
                            row: row_index,
                            col: col_index,
                        });
                        if build_path_in_slice(&cell.blocks, block_id, path) {
                            return true;
                        }
                        path.pop();
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    false
}

fn find_in_slice(blocks: &[BlockNode], id: BlockId) -> Option<&BlockNode> {
    for block in blocks {
        if block.id == id {
            return Some(block);
        }
        match &block.kind {
            BlockKind::Container { children, .. } => {
                if let Some(found) = find_in_slice(children, id) {
                    return Some(found);
                }
            }
            BlockKind::Grid { table, .. } => {
                for row in &table.rows {
                    for cell in &row.cells {
                        if let Some(found) = find_in_slice(&cell.blocks, id) {
                            return Some(found);
                        }
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    None
}

fn find_in_slice_mut(blocks: &mut [BlockNode], id: BlockId) -> Option<&mut BlockNode> {
    for block in blocks {
        if block.id == id {
            return Some(block);
        }
        match &mut block.kind {
            BlockKind::Container { children, .. } => {
                if let Some(found) = find_in_slice_mut(children, id) {
                    return Some(found);
                }
            }
            BlockKind::Grid { table, .. } => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        if let Some(found) = find_in_slice_mut(&mut cell.blocks, id) {
                            return Some(found);
                        }
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    None
}
