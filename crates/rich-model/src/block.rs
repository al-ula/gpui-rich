use crate::inline::InlineContent;
use crate::style::{
    BlockStyle, BlockWidth, DocumentLayoutConfig, Edges, RgbaColor, StyleId, StyleSheet,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockId(pub u64);

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BlockKind {
    Paragraph(InlineContent),
    Heading {
        level: u8,
        collapsed: bool,
        content: InlineContent,
    },
    ListItem {
        list_id: ListId,
        kind: ListKind,
        indent_level: u8,
        checked: Option<bool>,
        content: InlineContent,
    },
    Blockquote(Vec<BlockNode>),
    CodeBlock {
        language: Option<String>,
        content: InlineContent,
    },
    Table(TableBlock),
    HorizontalRule,
}

impl BlockKind {
    pub fn inline_content(&self) -> Option<&InlineContent> {
        match self {
            BlockKind::Paragraph(c)
            | BlockKind::Heading { content: c, .. }
            | BlockKind::ListItem { content: c, .. }
            | BlockKind::CodeBlock { content: c, .. } => Some(c),
            BlockKind::Blockquote(_) | BlockKind::Table(_) | BlockKind::HorizontalRule => None,
        }
    }

    pub fn inline_content_mut(&mut self) -> Option<&mut InlineContent> {
        match self {
            BlockKind::Paragraph(c)
            | BlockKind::Heading { content: c, .. }
            | BlockKind::ListItem { content: c, .. }
            | BlockKind::CodeBlock { content: c, .. } => Some(c),
            BlockKind::Blockquote(_) | BlockKind::Table(_) | BlockKind::HorizontalRule => None,
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
        self.push_block(BlockNode::new(id, BlockKind::Paragraph(content), style))
    }

    pub fn add_heading(&mut self, level: u8, content: InlineContent) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle::named(StyleId::heading(level));
        self.push_block(BlockNode::new(
            id,
            BlockKind::Heading {
                level: level.clamp(1, 6),
                collapsed: false,
                content,
            },
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
            BlockKind::ListItem {
                list_id,
                kind,
                indent_level,
                checked,
                content,
            },
            style,
        ))
    }

    pub fn add_code_block(&mut self, language: Option<String>, content: InlineContent) -> BlockId {
        let id = self.alloc_block_id();
        let style = BlockStyle::named(StyleId::CODE_BLOCK);
        self.push_block(BlockNode::new(
            id,
            BlockKind::CodeBlock { language, content },
            style,
        ))
    }

    pub fn add_blockquote(&mut self, children: Vec<BlockNode>) -> BlockId {
        let id = self.alloc_block_id();
        let mut style = BlockStyle::default();
        style.indent_start_pt = 16.0;
        style.space_before_pt = Some(6.0);
        style.space_after_pt = Some(10.0);
        self.push_block(BlockNode::new(id, BlockKind::Blockquote(children), style))
    }

    pub fn add_horizontal_rule(&mut self) -> BlockId {
        let id = self.alloc_block_id();
        let mut style = BlockStyle::default();
        style.space_before_pt = Some(10.0);
        style.space_after_pt = Some(10.0);
        self.push_block(BlockNode::new(id, BlockKind::HorizontalRule, style))
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

    /// Collects all text-bearing leaf blocks (including blockquote and table cell children) in reading order.
    pub fn collect_leaf_blocks(&self) -> Vec<&BlockNode> {
        let mut leaves = Vec::new();
        collect_leaves_in_slice(&self.blocks, &mut leaves);
        leaves
    }

    pub fn toggle_heading_collapsed(&mut self, id: BlockId) -> Option<bool> {
        let block = self.find_block_mut(id)?;
        if let BlockKind::Heading { collapsed, .. } = &mut block.kind {
            *collapsed = !*collapsed;
            let state = *collapsed;
            block.bump_version();
            Some(state)
        } else {
            None
        }
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
            BlockKind::Blockquote(children) => collect_leaves_in_slice(children, out),
            BlockKind::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_leaves_in_slice(&cell.blocks, out);
                    }
                }
            }
            BlockKind::Paragraph(_)
            | BlockKind::Heading { .. }
            | BlockKind::ListItem { .. }
            | BlockKind::CodeBlock { .. } => out.push(block),
            BlockKind::HorizontalRule => {}
        }
    }
}

fn find_in_slice(blocks: &[BlockNode], id: BlockId) -> Option<&BlockNode> {
    for block in blocks {
        if block.id == id {
            return Some(block);
        }
        match &block.kind {
            BlockKind::Blockquote(children) => {
                if let Some(found) = find_in_slice(children, id) {
                    return Some(found);
                }
            }
            BlockKind::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        if let Some(found) = find_in_slice(&cell.blocks, id) {
                            return Some(found);
                        }
                    }
                }
            }
            _ => {}
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
            BlockKind::Blockquote(children) => {
                if let Some(found) = find_in_slice_mut(children, id) {
                    return Some(found);
                }
            }
            BlockKind::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        if let Some(found) = find_in_slice_mut(&mut cell.blocks, id) {
                            return Some(found);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}
