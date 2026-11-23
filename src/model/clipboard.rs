//! Rich clipboard fragment extraction, `text/html` + `text/plain` serialization,
//! and normalized HTML parsing for copy/paste.

use crate::model::block::{
    BlockKind, BlockNode, BlockShapeKind, ColumnSpec, Document, ListId, ListKind, NumberStyle,
    TableBlock, TableBorderStyle, TableCell, TableRow, block_kinds,
};
use crate::model::components::PlaceholderKind;
use crate::model::inline::{
    ChipKind, ImageAssetId, InlineContent, InlineObject, InlineVAlign, OBJECT_REPLACEMENT_CHAR,
    OBJECT_REPLACEMENT_UTF8_LEN,
};
use crate::model::payload::CustomPayload;
use crate::model::selection::DocumentSelection;
use crate::model::style::{
    BaselineShift, BlockStyle, FontWeight, InlineStyle, LinkTarget, RgbaColor, SharedStr, StyleId,
    TextAlign, UnderlineKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

mod html;

/// Current wire version for same-process structured [`ClipboardFragment`] serialization.
pub const CLIPBOARD_FRAGMENT_VERSION: u32 = 1;

/// Failure returned when decoding or validating a versioned structured [`ClipboardFragment`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardFragmentError {
    /// Input was not valid JSON or did not match the versioned fragment envelope.
    Parse(String),
    /// Envelope version is not supported by this library version.
    UnsupportedVersion { found: u32, expected: u32 },
    /// Fragment violated structural invariants such as UTF-8 boundaries or object sentinels.
    Malformed(String),
}

impl fmt::Display for ClipboardFragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "clipboard fragment parse error: {message}"),
            Self::UnsupportedVersion { found, expected } => write!(
                formatter,
                "unsupported clipboard fragment version {found} (expected {expected})"
            ),
            Self::Malformed(message) => {
                write!(formatter, "malformed clipboard fragment: {message}")
            }
        }
    }
}

impl std::error::Error for ClipboardFragmentError {}

/// Custom node clipboard presentation formatter implemented by host registries
/// (such as `ExtensionRegistry`).
///
/// Controls readable plain-text and HTML output only. Same-process structured clipboard fragments
/// preserve custom node kinds, shapes, and payloads independently of this formatter.
pub trait ClipboardCustomFormatter {
    fn plain_text_block(
        &self,
        shape: BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        let _ = (shape, kind, data);
        None
    }

    fn html_block(
        &self,
        shape: BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        let _ = (shape, kind, data);
        None
    }

    fn plain_text_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        let _ = (kind, data);
        None
    }

    fn html_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        let _ = (kind, data);
        None
    }
}

/// Default clipboard formatter used when no custom node extensions are registered.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultClipboardFormatter;

impl ClipboardCustomFormatter for DefaultClipboardFormatter {}

/// Serializable block inside a [`ClipboardFragment`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClipboardBlock {
    Paragraph {
        content: InlineContent,
        align: TextAlign,
    },
    Heading {
        level: u8,
        content: InlineContent,
        align: TextAlign,
    },
    ListItem {
        list_group: u64,
        kind: ListKind,
        indent_level: u8,
        checked: Option<bool>,
        content: InlineContent,
        align: TextAlign,
    },
    Blockquote(Vec<ClipboardBlock>),
    CodeBlock {
        language: Option<String>,
        content: InlineContent,
    },
    Table(ClipboardTable),
    HorizontalRule,
    Custom {
        kind: SharedStr,
        data: CustomPayload,
    },
    CustomText {
        kind: SharedStr,
        data: CustomPayload,
        content: InlineContent,
        align: TextAlign,
    },
    CustomContainer {
        kind: SharedStr,
        data: CustomPayload,
        children: Vec<ClipboardBlock>,
    },
    CustomGrid {
        kind: SharedStr,
        data: CustomPayload,
        table: ClipboardTable,
    },
}

/// Portable table representation inside a [`ClipboardFragment`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipboardTable {
    pub rows: Vec<ClipboardTableRow>,
    #[serde(default)]
    pub column_kinds: BTreeMap<usize, PlaceholderKind>,
}

/// Portable table row inside a [`ClipboardTable`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipboardTableRow {
    pub is_header: bool,
    pub cells: Vec<ClipboardTableCell>,
}

/// Portable table cell inside a [`ClipboardTableRow`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipboardTableCell {
    pub colspan: u16,
    pub rowspan: u16,
    pub blocks: Vec<ClipboardBlock>,
}

/// Structured rich clipboard payload convertible to/from `text/html`, `text/plain`, and versioned
/// same-process JSON metadata.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipboardFragment {
    pub blocks: Vec<ClipboardBlock>,
    /// The leading block boundary was not selected; this fragment starts inside its text.
    pub start_open: bool,
    /// The trailing block boundary was not selected; this fragment ends inside its text.
    pub end_open: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VersionedClipboardFragment {
    version: u32,
    fragment: ClipboardFragment,
}

impl ClipboardFragment {
    /// Returns true when the fragment contains no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Builds a fragment from plain text lines split on `\n`.
    pub fn from_plain_text(text: &str) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let blocks = normalized
            .split('\n')
            .map(|line| ClipboardBlock::Paragraph {
                content: InlineContent::plain(line),
                align: TextAlign::Left,
            })
            .collect();
        Self {
            blocks,
            start_open: false,
            end_open: false,
        }
    }

    /// Serializes this fragment as plain UTF-8 text (TSV for tables) with default custom fallbacks.
    pub fn to_plain_text(&self) -> String {
        self.to_plain_text_with(&DefaultClipboardFormatter)
    }

    /// Serializes this fragment as plain UTF-8 text using `formatter` for custom nodes.
    pub fn to_plain_text_with(&self, formatter: &dyn ClipboardCustomFormatter) -> String {
        let mut parts = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            let rendered = block_to_plain_text(block, formatter);
            parts.push(rendered);
        }
        parts.join("\n")
    }

    /// Serializes this fragment as normalized semantic HTML with default custom fallbacks.
    pub fn to_html(&self) -> String {
        self.to_html_with(&DefaultClipboardFormatter)
    }

    /// Serializes this fragment as normalized semantic HTML using `formatter` for custom nodes.
    pub fn to_html_with(&self, formatter: &dyn ClipboardCustomFormatter) -> String {
        let mut output = String::new();
        if self.is_open_inline()
            && let Some(ClipboardBlock::Paragraph { content, .. }) = self.blocks.first()
        {
            html::write_inline_html(content, formatter, &mut output);
        } else {
            html::write_blocks_html(&self.blocks, formatter, &mut output);
        }
        output
    }

    /// Parses and normalizes an HTML clipboard string into a [`ClipboardFragment`].
    pub fn from_html(html: &str) -> Self {
        html::parse_html_fragment(html)
    }

    /// Builds a fragment capturing every top-level block of `document`.
    pub fn from_document(document: &Document) -> Self {
        Self {
            blocks: document
                .blocks
                .iter()
                .map(block_node_to_clipboard_block)
                .collect(),
            start_open: false,
            end_open: false,
        }
    }

    /// Converts all blocks in this fragment into fresh [`BlockNode`]s allocating IDs from `document`.
    pub fn to_block_nodes(&self, document: &mut Document) -> Vec<BlockNode> {
        let mut list_id_map: HashMap<u64, ListId> = HashMap::new();
        self.blocks
            .iter()
            .map(|block| clipboard_block_to_node(block, document, &mut list_id_map))
            .collect()
    }
    /// Builds a structured fragment from already-normalized blocks.
    pub fn from_blocks(blocks: Vec<ClipboardBlock>) -> Self {
        Self {
            blocks,
            start_open: false,
            end_open: false,
        }
    }

    /// Whether both edges can merge into surrounding text without paragraph boundaries.
    pub fn is_open_inline(&self) -> bool {
        self.start_open
            && self.end_open
            && matches!(self.blocks.as_slice(), [ClipboardBlock::Paragraph { .. }])
    }

    /// Serializes this fragment into a versioned JSON envelope for same-process rich clipboard paste.
    pub fn to_versioned_json(&self) -> Result<String, serde_json::Error> {
        let envelope = VersionedClipboardFragment {
            version: CLIPBOARD_FRAGMENT_VERSION,
            fragment: self.clone(),
        };
        serde_json::to_string(&envelope)
    }

    /// Parses and structurally validates a versioned JSON fragment envelope.
    ///
    /// Unknown custom node kinds are accepted as opaque data. Unsupported versions, invalid JSON,
    /// broken UTF-8 span boundaries, and missing inline object sentinels are rejected.
    pub fn from_versioned_json(input: &str) -> Result<Self, ClipboardFragmentError> {
        let envelope: VersionedClipboardFragment = serde_json::from_str(input)
            .map_err(|error| ClipboardFragmentError::Parse(error.to_string()))?;
        if envelope.version != CLIPBOARD_FRAGMENT_VERSION {
            return Err(ClipboardFragmentError::UnsupportedVersion {
                found: envelope.version,
                expected: CLIPBOARD_FRAGMENT_VERSION,
            });
        }
        let mut fragment = envelope.fragment;
        validate_clipboard_blocks(&mut fragment.blocks)?;
        Ok(fragment)
    }
}

fn validate_clipboard_blocks(blocks: &mut [ClipboardBlock]) -> Result<(), ClipboardFragmentError> {
    for block in blocks {
        match block {
            ClipboardBlock::Paragraph { content, .. }
            | ClipboardBlock::Heading { content, .. }
            | ClipboardBlock::ListItem { content, .. }
            | ClipboardBlock::CodeBlock { content, .. }
            | ClipboardBlock::CustomText { content, .. } => {
                validate_inline_content(content)?;
            }
            ClipboardBlock::Blockquote(children)
            | ClipboardBlock::CustomContainer { children, .. } => {
                validate_clipboard_blocks(children)?;
            }
            ClipboardBlock::Table(table) | ClipboardBlock::CustomGrid { table, .. } => {
                validate_clipboard_table(table)?;
            }
            ClipboardBlock::HorizontalRule | ClipboardBlock::Custom { .. } => {}
        }
    }
    Ok(())
}

fn validate_clipboard_table(table: &mut ClipboardTable) -> Result<(), ClipboardFragmentError> {
    for row in &mut table.rows {
        for cell in &mut row.cells {
            if cell.colspan == 0 || cell.rowspan == 0 {
                return Err(ClipboardFragmentError::Malformed(
                    "table cell colspan and rowspan must be at least 1".to_owned(),
                ));
            }
            validate_clipboard_blocks(&mut cell.blocks)?;
        }
    }
    Ok(())
}

fn validate_inline_content(content: &mut InlineContent) -> Result<(), ClipboardFragmentError> {
    let text_len = content.text.len();
    let mut previous_end = 0usize;
    for span in &content.spans {
        if span.range.start > span.range.end || span.range.end > text_len {
            return Err(ClipboardFragmentError::Malformed(format!(
                "inline span range {:?} out of bounds for text length {text_len}",
                span.range
            )));
        }
        if !content.text.is_char_boundary(span.range.start)
            || !content.text.is_char_boundary(span.range.end)
        {
            return Err(ClipboardFragmentError::Malformed(format!(
                "inline span range {:?} does not align to UTF-8 boundaries",
                span.range
            )));
        }
        if span.range.start < previous_end {
            return Err(ClipboardFragmentError::Malformed(
                "inline spans overlap or are out of order".to_owned(),
            ));
        }
        previous_end = span.range.end;
    }

    let mut previous_object_end = 0usize;
    for entry in &content.objects {
        let Some(end) = entry.byte_offset.checked_add(OBJECT_REPLACEMENT_UTF8_LEN) else {
            return Err(ClipboardFragmentError::Malformed(
                "inline object byte offset overflow".to_owned(),
            ));
        };
        if end > text_len
            || !content.text.is_char_boundary(entry.byte_offset)
            || !content.text[entry.byte_offset..].starts_with(OBJECT_REPLACEMENT_CHAR)
        {
            return Err(ClipboardFragmentError::Malformed(format!(
                "inline object at byte {} does not match U+FFFC sentinel",
                entry.byte_offset
            )));
        }
        if entry.byte_offset < previous_object_end {
            return Err(ClipboardFragmentError::Malformed(
                "inline objects overlap or are out of order".to_owned(),
            ));
        }
        previous_object_end = end;
    }

    content.normalize();
    Ok(())
}

impl Document {
    /// Extracts a rich [`ClipboardFragment`] covering `selection`.
    pub fn extract_selection_fragment(
        &self,
        selection: &DocumentSelection,
    ) -> Option<ClipboardFragment> {
        match selection {
            DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                focus_cell,
            } => {
                let table = self.find_block(*table_id)?.kind.table_block()?;
                let (col_count, placements, occupancy) = table.resolve_grid();
                if col_count == 0 || table.rows.is_empty() {
                    return None;
                }
                let mut row_min = anchor_cell.0.min(focus_cell.0).min(table.rows.len() - 1);
                let mut row_max = anchor_cell.0.max(focus_cell.0).min(table.rows.len() - 1);
                let mut col_min = anchor_cell.1.min(focus_cell.1).min(col_count - 1);
                let mut col_max = anchor_cell.1.max(focus_cell.1).min(col_count - 1);
                loop {
                    let (start_row, end_row, start_col, end_col) =
                        (row_min, row_max, col_min, col_max);
                    for occupancy_row in occupancy.get(start_row..=end_row).into_iter().flatten() {
                        for &placement_index in occupancy_row
                            .get(start_col..=end_col)
                            .into_iter()
                            .flatten()
                            .flatten()
                        {
                            let Some(placement) = placements.get(placement_index) else {
                                continue;
                            };
                            row_min = row_min.min(placement.row_index);
                            row_max = row_max.max(placement.row_index + placement.rowspan - 1);
                            col_min = col_min.min(placement.visual_col);
                            col_max = col_max.max(placement.visual_col + placement.colspan - 1);
                        }
                    }
                    if (start_row, end_row, start_col, end_col)
                        == (row_min, row_max, col_min, col_max)
                    {
                        break;
                    }
                }
                let mut rows = Vec::new();
                for (row_index, row) in table
                    .rows
                    .iter()
                    .enumerate()
                    .take(row_max + 1)
                    .skip(row_min)
                {
                    let mut cells = Vec::new();
                    for placement in placements.iter().filter(|placement| {
                        placement.row_index == row_index
                            && placement.visual_col <= col_max
                            && placement.visual_col + placement.colspan > col_min
                    }) {
                        let cell = &row.cells[placement.cell_index];
                        cells.push(ClipboardTableCell {
                            colspan: cell.colspan,
                            rowspan: cell.rowspan,
                            blocks: cell
                                .blocks
                                .iter()
                                .map(block_node_to_clipboard_block)
                                .collect(),
                        });
                    }
                    rows.push(ClipboardTableRow {
                        is_header: row.is_header,
                        cells,
                    });
                }
                Some(ClipboardFragment {
                    blocks: vec![ClipboardBlock::Table(ClipboardTable {
                        rows,
                        column_kinds: table
                            .column_kinds
                            .range(col_min..=col_max)
                            .map(|(&column, &kind)| (column - col_min, kind))
                            .collect(),
                    })],
                    start_open: false,
                    end_open: false,
                })
            }
            DocumentSelection::Text { anchor, focus, .. } => {
                if anchor == focus {
                    return None;
                }
                let anchor_leaf_id = anchor.leaf_block_id()?;
                let focus_leaf_id = focus.leaf_block_id()?;
                let leaves = self.collect_leaf_blocks();
                let anchor_index = leaves.iter().position(|block| block.id == anchor_leaf_id)?;
                let focus_index = leaves.iter().position(|block| block.id == focus_leaf_id)?;

                let ((start_index, start_offset), (end_index, end_offset)) =
                    if (anchor_index, anchor.offset) <= (focus_index, focus.offset) {
                        ((anchor_index, anchor.offset), (focus_index, focus.offset))
                    } else {
                        ((focus_index, focus.offset), (anchor_index, anchor.offset))
                    };
                let anchor_cell_path = anchor.path.iter().rev().find_map(|segment| match segment {
                    crate::model::selection::PathSegment::TableCell { table_id, row, col } => {
                        Some((*table_id, *row, *col))
                    }
                    crate::model::selection::PathSegment::Block(_) => None,
                });
                let focus_cell_path = focus.path.iter().rev().find_map(|segment| match segment {
                    crate::model::selection::PathSegment::TableCell { table_id, row, col } => {
                        Some((*table_id, *row, *col))
                    }
                    crate::model::selection::PathSegment::Block(_) => None,
                });
                let same_table_cell =
                    anchor_cell_path.is_some() && anchor_cell_path == focus_cell_path;
                let crosses_table_cells = anchor_cell_path.is_some()
                    && focus_cell_path.is_some()
                    && anchor_cell_path != focus_cell_path;
                let mut full_structures = Vec::new();
                let mut root_leaf_start = 0usize;
                for root in &self.blocks {
                    let leaf_count = block_text_leaf_count(root);
                    let root_leaf_end = root_leaf_start + leaf_count;
                    let covers_start = root_leaf_start > start_index
                        || (root_leaf_start == start_index && start_offset == 0);
                    let covers_end = root_leaf_end <= end_index
                        || (root_leaf_end == end_index + 1
                            && leaves[end_index]
                                .kind
                                .inline_content()
                                .is_some_and(|inline| end_offset >= inline.len()));
                    if leaf_count == 0 {
                        if root_leaf_start > start_index && root_leaf_start <= end_index {
                            full_structures.push((
                                root_leaf_start,
                                root_leaf_end,
                                block_node_to_clipboard_block(root),
                            ));
                        }
                    } else if covers_start
                        && covers_end
                        && (matches!(root.kind, BlockKind::Container { .. })
                            || (!same_table_cell && matches!(root.kind, BlockKind::Grid { .. })))
                    {
                        full_structures.push((
                            root_leaf_start,
                            root_leaf_end,
                            block_node_to_clipboard_block(root),
                        ));
                    } else if leaf_count > 0
                        && root_leaf_start <= end_index
                        && root_leaf_end > start_index
                        && (matches!(root.kind, BlockKind::Container { .. })
                            || (!same_table_cell && matches!(root.kind, BlockKind::Grid { .. })))
                    {
                        let mut cursor = root_leaf_start;
                        if let Some(fragment) = extract_partial_node(
                            root,
                            &mut cursor,
                            start_index,
                            start_offset,
                            end_index,
                            end_offset,
                            crosses_table_cells,
                        ) {
                            full_structures.push((
                                root_leaf_start.max(start_index),
                                root_leaf_end.min(end_index + 1),
                                fragment,
                            ));
                        }
                    }
                    root_leaf_start = root_leaf_end;
                }
                let mut blocks = Vec::new();
                for index in start_index..=end_index {
                    for (_, _, structure) in full_structures
                        .iter()
                        .filter(|(start, _, _)| *start == index)
                    {
                        blocks.push(structure.clone());
                    }
                    if full_structures
                        .iter()
                        .any(|(start, end, _)| index >= *start && index < *end)
                    {
                        continue;
                    }
                    let Some(block) = leaves.get(index) else {
                        continue;
                    };
                    let Some(inline) = block.kind.inline_content() else {
                        continue;
                    };
                    let slice_start = if index == start_index {
                        clamp_char_boundary(&inline.text, start_offset)
                    } else {
                        0
                    };
                    let slice_end = if index == end_index {
                        clamp_char_boundary(&inline.text, end_offset).max(slice_start)
                    } else {
                        inline.len()
                    };
                    let sliced = inline.slice(slice_start..slice_end);
                    let align = block.style.align;
                    let clipboard_block = if block.kind.is_paragraph() {
                        ClipboardBlock::Paragraph {
                            content: sliced,
                            align,
                        }
                    } else if let Some(heading) = block.kind.heading_data() {
                        ClipboardBlock::Heading {
                            level: heading.level,
                            content: sliced,
                            align,
                        }
                    } else if let Some(item) = block.kind.list_item_data() {
                        ClipboardBlock::ListItem {
                            list_group: item.list_id.0,
                            kind: item.kind,
                            indent_level: item.indent_level,
                            checked: item.checked,
                            content: sliced,
                            align,
                        }
                    } else if let Some(code) = block.kind.code_block_data() {
                        ClipboardBlock::CodeBlock {
                            language: code.language.clone(),
                            content: sliced,
                        }
                    } else if let BlockKind::Text { kind, data, .. } = &block.kind {
                        ClipboardBlock::CustomText {
                            kind: kind.clone(),
                            data: data.clone(),
                            content: sliced,
                            align,
                        }
                    } else {
                        ClipboardBlock::Paragraph {
                            content: sliced,
                            align,
                        }
                    };
                    blocks.push(clipboard_block);
                }
                if blocks.is_empty() {
                    None
                } else {
                    let start_len = leaves[start_index]
                        .kind
                        .inline_content()
                        .map_or(0, |inline| inline.len());
                    let end_len = leaves[end_index]
                        .kind
                        .inline_content()
                        .map_or(0, |inline| inline.len());
                    let start_open = if start_index == end_index {
                        true
                    } else {
                        start_offset > 0 && start_offset < start_len
                    };
                    let end_open = if start_index == end_index {
                        true
                    } else {
                        end_offset > 0 && end_offset < end_len
                    };
                    Some(ClipboardFragment {
                        blocks,
                        start_open,
                        end_open,
                    })
                }
            }
        }
    }
}

fn block_node_to_clipboard_block(block: &BlockNode) -> ClipboardBlock {
    let align = block.style.align;
    match &block.kind {
        BlockKind::Text {
            kind,
            data,
            content,
        } => match kind.as_ref() {
            block_kinds::PARAGRAPH => ClipboardBlock::Paragraph {
                content: content.clone(),
                align,
            },
            block_kinds::HEADING => ClipboardBlock::Heading {
                level: block.kind.heading_data().map(|h| h.level).unwrap_or(1),
                content: content.clone(),
                align,
            },
            block_kinds::LIST_ITEM => {
                if let Some(item) = block.kind.list_item_data() {
                    ClipboardBlock::ListItem {
                        list_group: item.list_id.0,
                        kind: item.kind,
                        indent_level: item.indent_level,
                        checked: item.checked,
                        content: content.clone(),
                        align,
                    }
                } else {
                    ClipboardBlock::Paragraph {
                        content: content.clone(),
                        align,
                    }
                }
            }
            block_kinds::CODE_BLOCK => ClipboardBlock::CodeBlock {
                language: block
                    .kind
                    .code_block_data()
                    .and_then(|code| code.language.clone()),
                content: content.clone(),
            },
            _ => ClipboardBlock::CustomText {
                kind: kind.clone(),
                data: data.clone(),
                content: content.clone(),
                align,
            },
        },
        BlockKind::Container {
            kind,
            data,
            children,
        } => {
            let mapped: Vec<ClipboardBlock> =
                children.iter().map(block_node_to_clipboard_block).collect();
            if kind.as_ref() == block_kinds::BLOCKQUOTE {
                ClipboardBlock::Blockquote(mapped)
            } else {
                ClipboardBlock::CustomContainer {
                    kind: kind.clone(),
                    data: data.clone(),
                    children: mapped,
                }
            }
        }
        BlockKind::Grid { kind, data, table } => {
            let clipboard_table = ClipboardTable {
                column_kinds: table.column_kinds.clone(),
                rows: table
                    .rows
                    .iter()
                    .map(|row| ClipboardTableRow {
                        is_header: row.is_header,
                        cells: row
                            .cells
                            .iter()
                            .map(|cell| ClipboardTableCell {
                                colspan: cell.colspan,
                                rowspan: cell.rowspan,
                                blocks: cell
                                    .blocks
                                    .iter()
                                    .map(block_node_to_clipboard_block)
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            };
            if kind.as_ref() == block_kinds::TABLE {
                ClipboardBlock::Table(clipboard_table)
            } else {
                ClipboardBlock::CustomGrid {
                    kind: kind.clone(),
                    data: data.clone(),
                    table: clipboard_table,
                }
            }
        }
        BlockKind::Leaf { kind, data } => {
            if kind.as_ref() == block_kinds::HORIZONTAL_RULE {
                ClipboardBlock::HorizontalRule
            } else {
                ClipboardBlock::Custom {
                    kind: kind.clone(),
                    data: data.clone(),
                }
            }
        }
    }
}
fn block_text_leaf_count(block: &BlockNode) -> usize {
    match &block.kind {
        BlockKind::Text { .. } => 1,
        BlockKind::Container { children, .. } => children.iter().map(block_text_leaf_count).sum(),
        BlockKind::Grid { table, .. } => table
            .rows
            .iter()
            .flat_map(|row| &row.cells)
            .flat_map(|cell| &cell.blocks)
            .map(block_text_leaf_count)
            .sum(),
        BlockKind::Leaf { .. } => 0,
    }
}
fn extract_partial_node(
    block: &BlockNode,
    cursor: &mut usize,
    start_index: usize,
    start_offset: usize,
    end_index: usize,
    end_offset: usize,
    whole_cells: bool,
) -> Option<ClipboardBlock> {
    match &block.kind {
        BlockKind::Text { .. } => {
            let index = *cursor;
            *cursor += 1;
            if !(start_index..=end_index).contains(&index) {
                return None;
            }
            let inline = block.kind.inline_content()?;
            let slice_start = if index == start_index {
                clamp_char_boundary(&inline.text, start_offset)
            } else {
                0
            };
            let slice_end = if index == end_index {
                clamp_char_boundary(&inline.text, end_offset).max(slice_start)
            } else {
                inline.len()
            };
            Some(clipboard_text_block(
                block,
                inline.slice(slice_start..slice_end),
            ))
        }
        BlockKind::Container {
            kind,
            data,
            children,
        } => {
            let mut extracted = Vec::new();
            for child in children {
                let child_count = block_text_leaf_count(child);
                if child_count == 0 {
                    if *cursor > start_index && *cursor <= end_index {
                        extracted.push(block_node_to_clipboard_block(child));
                    }
                    continue;
                }
                let child_start = *cursor;
                if let Some(fragment) = extract_partial_node(
                    child,
                    cursor,
                    start_index,
                    start_offset,
                    end_index,
                    end_offset,
                    whole_cells,
                ) {
                    extracted.push(fragment);
                } else {
                    *cursor = child_start + child_count;
                }
            }
            if extracted.is_empty() {
                None
            } else if kind.as_ref() == block_kinds::BLOCKQUOTE {
                Some(ClipboardBlock::Blockquote(extracted))
            } else {
                Some(ClipboardBlock::CustomContainer {
                    kind: kind.clone(),
                    data: data.clone(),
                    children: extracted,
                })
            }
        }
        BlockKind::Grid { kind, data, table } => {
            let mut rows = Vec::with_capacity(table.rows.len());
            for row in &table.rows {
                let mut cells = Vec::new();
                for cell in &row.cells {
                    let cell_start = *cursor;
                    let cell_count: usize = cell.blocks.iter().map(block_text_leaf_count).sum();
                    if whole_cells {
                        if cell_count > 0
                            && cell_start <= end_index
                            && cell_start + cell_count > start_index
                        {
                            cells.push(ClipboardTableCell {
                                colspan: cell.colspan,
                                rowspan: cell.rowspan,
                                blocks: cell
                                    .blocks
                                    .iter()
                                    .map(block_node_to_clipboard_block)
                                    .collect(),
                            });
                        }
                        *cursor += cell_count;
                        continue;
                    }
                    let mut cell_blocks = Vec::new();
                    for child in &cell.blocks {
                        let child_count = block_text_leaf_count(child);
                        if child_count == 0 {
                            if *cursor > start_index && *cursor <= end_index {
                                cell_blocks.push(block_node_to_clipboard_block(child));
                            }
                        } else if let Some(fragment) = extract_partial_node(
                            child,
                            cursor,
                            start_index,
                            start_offset,
                            end_index,
                            end_offset,
                            whole_cells,
                        ) {
                            cell_blocks.push(fragment);
                        } else {
                            *cursor += child_count;
                        }
                    }
                    if !cell_blocks.is_empty() {
                        cells.push(ClipboardTableCell {
                            colspan: cell.colspan,
                            rowspan: cell.rowspan,
                            blocks: cell_blocks,
                        });
                    }
                }
                rows.push(ClipboardTableRow {
                    is_header: row.is_header,
                    cells,
                });
            }
            let clipboard_table = ClipboardTable {
                rows,
                column_kinds: Default::default(),
            };
            if kind.as_ref() == block_kinds::TABLE {
                Some(ClipboardBlock::Table(clipboard_table))
            } else {
                Some(ClipboardBlock::CustomGrid {
                    kind: kind.clone(),
                    data: data.clone(),
                    table: clipboard_table,
                })
            }
        }
        BlockKind::Leaf { .. } => {
            if *cursor > start_index && *cursor <= end_index {
                Some(block_node_to_clipboard_block(block))
            } else {
                None
            }
        }
    }
}

fn clipboard_text_block(block: &BlockNode, content: InlineContent) -> ClipboardBlock {
    let align = block.style.align;
    if block.kind.is_paragraph() {
        ClipboardBlock::Paragraph { content, align }
    } else if let Some(heading) = block.kind.heading_data() {
        ClipboardBlock::Heading {
            level: heading.level,
            content,
            align,
        }
    } else if let Some(item) = block.kind.list_item_data() {
        ClipboardBlock::ListItem {
            list_group: item.list_id.0,
            kind: item.kind,
            indent_level: item.indent_level,
            checked: item.checked,
            content,
            align,
        }
    } else if let Some(code) = block.kind.code_block_data() {
        ClipboardBlock::CodeBlock {
            language: code.language.clone(),
            content,
        }
    } else if let BlockKind::Text { kind, data, .. } = &block.kind {
        ClipboardBlock::CustomText {
            kind: kind.clone(),
            data: data.clone(),
            content,
            align,
        }
    } else {
        ClipboardBlock::Paragraph { content, align }
    }
}

fn clipboard_block_to_node(
    block: &ClipboardBlock,
    document: &mut Document,
    list_id_map: &mut HashMap<u64, ListId>,
) -> BlockNode {
    let id = document.alloc_block_id();
    match block {
        ClipboardBlock::Paragraph { content, align } => BlockNode::new(
            id,
            BlockKind::paragraph(content.clone()),
            BlockStyle::default().with_align(*align),
        ),
        ClipboardBlock::Heading {
            level,
            content,
            align,
        } => {
            let clamped = (*level).clamp(1, 6);
            BlockNode::new(
                id,
                BlockKind::heading(clamped, false, content.clone()),
                BlockStyle::named(StyleId::heading(clamped)).with_align(*align),
            )
        }
        ClipboardBlock::ListItem {
            list_group,
            kind,
            indent_level,
            checked,
            content,
            align,
        } => {
            let list_id = if let Some(existing) = list_id_map.get(list_group) {
                *existing
            } else {
                let next_base = document.alloc_list_id().0;
                let allocated = ListId(next_base.saturating_add(list_id_map.len() as u64));
                list_id_map.insert(*list_group, allocated);
                allocated
            };
            let indent = 24.0 * (*indent_level as f32 + 1.0);
            let mut style = BlockStyle::default()
                .with_indent(indent, 0.0)
                .with_align(*align);
            style.space_after_pt = Some(4.0);
            BlockNode::new(
                id,
                BlockKind::list_item(list_id, *kind, *indent_level, *checked, content.clone()),
                style,
            )
        }
        ClipboardBlock::Blockquote(children) => {
            let mut child_nodes: Vec<BlockNode> = children
                .iter()
                .map(|child| clipboard_block_to_node(child, document, list_id_map))
                .collect();
            if child_nodes.is_empty() {
                let child_id = document.alloc_block_id();
                child_nodes.push(BlockNode::new(
                    child_id,
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                ));
            }
            let style = BlockStyle {
                indent_start_pt: 16.0,
                space_before_pt: Some(6.0),
                space_after_pt: Some(10.0),
                ..BlockStyle::default()
            };
            BlockNode::new(id, BlockKind::blockquote(child_nodes), style)
        }
        ClipboardBlock::CodeBlock { language, content } => BlockNode::new(
            id,
            BlockKind::code_block(language.clone(), content.clone()),
            BlockStyle::named(StyleId::CODE_BLOCK),
        ),
        ClipboardBlock::Table(table) => {
            let column_count = table
                .rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|cell| cell.colspan.max(1) as usize)
                        .sum::<usize>()
                })
                .max()
                .unwrap_or(1)
                .max(1);
            let rows = table
                .rows
                .iter()
                .map(|row| TableRow {
                    is_header: row.is_header,
                    cells: row
                        .cells
                        .iter()
                        .map(|cell| {
                            let cell_id = document.alloc_block_id();
                            let mut cell_blocks: Vec<BlockNode> = cell
                                .blocks
                                .iter()
                                .map(|inner| clipboard_block_to_node(inner, document, list_id_map))
                                .collect();
                            if cell_blocks.is_empty() {
                                let para_id = document.alloc_block_id();
                                cell_blocks.push(BlockNode::new(
                                    para_id,
                                    BlockKind::paragraph(InlineContent::empty()),
                                    BlockStyle::default(),
                                ));
                            }
                            let mut table_cell = TableCell::simple(cell_id, cell_blocks);
                            table_cell.colspan = cell.colspan.max(1);
                            table_cell.rowspan = cell.rowspan.max(1);
                            table_cell
                        })
                        .collect(),
                })
                .collect();
            BlockNode::new(
                id,
                BlockKind::table(TableBlock {
                    columns: vec![ColumnSpec::Auto; column_count],
                    column_kinds: table.column_kinds.clone(),
                    rows,
                    border_style: TableBorderStyle::default(),
                }),
                BlockStyle::default(),
            )
        }
        ClipboardBlock::HorizontalRule => {
            let style = BlockStyle {
                space_before_pt: Some(10.0),
                space_after_pt: Some(10.0),
                ..BlockStyle::default()
            };
            BlockNode::new(id, BlockKind::horizontal_rule(), style)
        }
        ClipboardBlock::Custom { kind, data } => {
            let style = BlockStyle {
                space_before_pt: Some(6.0),
                space_after_pt: Some(8.0),
                ..BlockStyle::default()
            };
            BlockNode::new(id, BlockKind::raw_leaf(kind.clone(), data.clone()), style)
        }
        ClipboardBlock::CustomText {
            kind,
            data,
            content,
            align,
        } => BlockNode::new(
            id,
            BlockKind::raw_text(kind.clone(), data.clone(), content.clone()),
            BlockStyle::default().with_align(*align),
        ),
        ClipboardBlock::CustomContainer {
            kind,
            data,
            children,
        } => {
            let mut child_nodes: Vec<BlockNode> = children
                .iter()
                .map(|child| clipboard_block_to_node(child, document, list_id_map))
                .collect();
            if child_nodes.is_empty() {
                let child_id = document.alloc_block_id();
                child_nodes.push(BlockNode::new(
                    child_id,
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                ));
            }
            let style = BlockStyle {
                space_before_pt: Some(6.0),
                space_after_pt: Some(8.0),
                ..BlockStyle::default()
            };
            BlockNode::new(
                id,
                BlockKind::raw_container(kind.clone(), data.clone(), child_nodes),
                style,
            )
        }
        ClipboardBlock::CustomGrid { kind, data, table } => {
            let column_count = table
                .rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|cell| cell.colspan.max(1) as usize)
                        .sum::<usize>()
                })
                .max()
                .unwrap_or(1)
                .max(1);
            let rows = table
                .rows
                .iter()
                .map(|row| TableRow {
                    is_header: row.is_header,
                    cells: row
                        .cells
                        .iter()
                        .map(|cell| {
                            let cell_id = document.alloc_block_id();
                            let mut cell_blocks: Vec<BlockNode> = cell
                                .blocks
                                .iter()
                                .map(|inner| clipboard_block_to_node(inner, document, list_id_map))
                                .collect();
                            if cell_blocks.is_empty() {
                                let para_id = document.alloc_block_id();
                                cell_blocks.push(BlockNode::new(
                                    para_id,
                                    BlockKind::paragraph(InlineContent::empty()),
                                    BlockStyle::default(),
                                ));
                            }
                            let mut table_cell = TableCell::simple(cell_id, cell_blocks);
                            table_cell.colspan = cell.colspan.max(1);
                            table_cell.rowspan = cell.rowspan.max(1);
                            table_cell
                        })
                        .collect(),
                })
                .collect();
            BlockNode::new(
                id,
                BlockKind::raw_grid(
                    kind.clone(),
                    data.clone(),
                    TableBlock {
                        columns: vec![ColumnSpec::Auto; column_count],
                        column_kinds: table.column_kinds.clone(),
                        rows,
                        border_style: TableBorderStyle::default(),
                    },
                ),
                BlockStyle::default(),
            )
        }
    }
}

// ============================================================================
// Plain-Text Serialization
// ============================================================================

fn table_to_plain_text(table: &ClipboardTable, formatter: &dyn ClipboardCustomFormatter) -> String {
    table
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| {
                    cell.blocks
                        .iter()
                        .map(|inner| block_to_plain_text(inner, formatter))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_to_plain_text(block: &ClipboardBlock, formatter: &dyn ClipboardCustomFormatter) -> String {
    match block {
        ClipboardBlock::Paragraph { content, .. }
        | ClipboardBlock::Heading { content, .. }
        | ClipboardBlock::ListItem { content, .. }
        | ClipboardBlock::CodeBlock { content, .. } => inline_to_plain_text(content, formatter),
        ClipboardBlock::CustomText {
            kind,
            data,
            content,
            ..
        } => {
            let body = inline_to_plain_text(content, formatter);
            if let Some(prefix) = formatter.plain_text_block(BlockShapeKind::Text, kind, data) {
                if body.is_empty() {
                    prefix
                } else {
                    format!("{prefix}\n{body}")
                }
            } else {
                body
            }
        }
        ClipboardBlock::Blockquote(children) => children
            .iter()
            .map(|child| block_to_plain_text(child, formatter))
            .collect::<Vec<_>>()
            .join("\n"),
        ClipboardBlock::CustomContainer {
            kind,
            data,
            children,
        } => {
            let body = children
                .iter()
                .map(|child| block_to_plain_text(child, formatter))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(prefix) = formatter.plain_text_block(BlockShapeKind::Container, kind, data)
            {
                if body.is_empty() {
                    prefix
                } else {
                    format!("{prefix}\n{body}")
                }
            } else {
                body
            }
        }
        ClipboardBlock::Table(table) => table_to_plain_text(table, formatter),
        ClipboardBlock::CustomGrid { kind, data, table } => {
            let body = table_to_plain_text(table, formatter);
            if let Some(prefix) = formatter.plain_text_block(BlockShapeKind::Grid, kind, data) {
                if body.is_empty() {
                    prefix
                } else {
                    format!("{prefix}\n{body}")
                }
            } else {
                body
            }
        }
        ClipboardBlock::HorizontalRule => "---".into(),
        ClipboardBlock::Custom { kind, data } => formatter
            .plain_text_block(BlockShapeKind::Leaf, kind, data)
            .unwrap_or_else(|| format!("[{kind}]")),
    }
}

fn inline_to_plain_text(
    content: &InlineContent,
    formatter: &dyn ClipboardCustomFormatter,
) -> String {
    if content.objects.is_empty() {
        return content.text.clone();
    }
    let mut output = String::with_capacity(content.text.len());
    for (byte_index, character) in content.text.char_indices() {
        if character == OBJECT_REPLACEMENT_CHAR
            && let Some(entry) = content.object_at(byte_index)
        {
            match &entry.object {
                InlineObject::Image { alt, .. } => output.push_str(alt),
                InlineObject::SmartChip { label, .. } => output.push_str(label),
                InlineObject::FootnoteRef { index } => {
                    output.push_str(&format!("[{index}]"));
                }
                InlineObject::Custom { kind, data } => {
                    if let Some(custom_text) = formatter.plain_text_inline(kind, data) {
                        output.push_str(&custom_text);
                    } else {
                        output.push_str(&format!("[{kind}]"));
                    }
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

fn clamp_char_boundary(text: &str, offset: usize) -> usize {
    let mut clamped = offset.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn explicit_css_font_weight_overrides_semantic_tag_defaults() {
        let fragment = ClipboardFragment::from_html(
            "<p><b style=\"font-weight:normal\">regular</b> <span style=\"font-weight:400\">plain</span> <span style=\"font-weight:700\">bold</span></p>",
        );
        let ClipboardBlock::Paragraph { content, .. } = &fragment.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(content.style_at(0).weight, Some(FontWeight(400)));
        assert_eq!(content.style_at(8).weight, Some(FontWeight(400)));
        assert_eq!(content.style_at(15).weight, Some(FontWeight(700)));
    }
    #[test]
    fn html_paragraph_boundaries_and_inline_edges_are_distinct() {
        let inline = ClipboardFragment::from_html("<span>selected</span>");
        assert!(inline.is_open_inline());
        assert_eq!(inline.to_html(), "selected");

        let paragraph = ClipboardFragment::from_html("<p>selected</p>");
        assert!(!paragraph.is_open_inline());
    }

    #[test]
    fn table_copy_uses_visual_occupancy_beside_rowspans() {
        let mut document = Document::new();
        let table_id = document.alloc_block_id();
        let mut make_cell = |text: &str, rowspan: u16| {
            let cell_id = document.alloc_block_id();
            let paragraph_id = document.alloc_block_id();
            let mut cell = TableCell::simple(
                cell_id,
                vec![BlockNode::new(
                    paragraph_id,
                    BlockKind::paragraph(InlineContent::plain(text)),
                    BlockStyle::default(),
                )],
            );
            cell.rowspan = rowspan;
            cell
        };
        let table = TableBlock {
            columns: vec![ColumnSpec::Auto; 2],
            rows: vec![
                TableRow {
                    is_header: false,
                    cells: vec![make_cell("A", 2), make_cell("B", 1)],
                },
                TableRow {
                    is_header: false,
                    cells: vec![make_cell("C", 1)],
                },
            ],
            column_kinds: Default::default(),
            border_style: TableBorderStyle::default(),
        };
        drop(make_cell);
        document.push_block(BlockNode::new(
            table_id,
            BlockKind::table(table),
            BlockStyle::default(),
        ));
        let fragment = document
            .extract_selection_fragment(&DocumentSelection::TableCells {
                table_id,
                anchor_cell: (0, 1),
                focus_cell: (1, 1),
            })
            .unwrap();
        let ClipboardBlock::Table(table) = &fragment.blocks[0] else {
            panic!("expected table");
        };
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            block_to_plain_text(
                &table.rows[0].cells[0].blocks[0],
                &DefaultClipboardFormatter
            ),
            "B"
        );
        assert_eq!(
            block_to_plain_text(
                &table.rows[1].cells[0].blocks[0],
                &DefaultClipboardFormatter
            ),
            "C"
        );
        let expanded = document
            .extract_selection_fragment(&DocumentSelection::TableCells {
                table_id,
                anchor_cell: (1, 0),
                focus_cell: (1, 0),
            })
            .unwrap();
        let ClipboardBlock::Table(expanded_table) = &expanded.blocks[0] else {
            panic!("expected expanded table");
        };
        assert_eq!(expanded_table.rows[0].cells[0].rowspan, 2);
    }
    #[test]
    fn selection_extraction_preserves_structural_nodes_and_atomic_blocks() {
        let mut document = Document::new();
        let before_id =
            document.add_paragraph(InlineContent::plain("before"), BlockStyle::default());
        let container_id = document.alloc_block_id();
        let first_child_id = document.alloc_block_id();
        let second_child_id = document.alloc_block_id();
        document.push_block(BlockNode::new(
            container_id,
            BlockKind::blockquote(vec![
                BlockNode::new(
                    first_child_id,
                    BlockKind::paragraph(InlineContent::plain("inside one")),
                    BlockStyle::default(),
                ),
                BlockNode::new(
                    second_child_id,
                    BlockKind::paragraph(InlineContent::plain("inside two")),
                    BlockStyle::default(),
                ),
            ]),
            BlockStyle::default(),
        ));
        document.add_horizontal_rule();
        let table_id = document.alloc_block_id();
        let cell_id = document.alloc_block_id();
        let cell_paragraph_id = document.alloc_block_id();
        let table = TableBlock {
            columns: vec![ColumnSpec::Auto],
            rows: vec![TableRow {
                is_header: false,
                cells: vec![TableCell::simple(
                    cell_id,
                    vec![BlockNode::new(
                        cell_paragraph_id,
                        BlockKind::paragraph(InlineContent::plain("table cell")),
                        BlockStyle::default(),
                    )],
                )],
            }],
            column_kinds: Default::default(),
            border_style: TableBorderStyle::default(),
        };
        document.push_block(BlockNode::new(
            table_id,
            BlockKind::table(table),
            BlockStyle::default(),
        ));
        let after_id = document.add_paragraph(InlineContent::plain("after"), BlockStyle::default());
        let selection = DocumentSelection::Text {
            anchor: document.doc_pos_for_block(before_id, 0).unwrap(),
            focus: document.doc_pos_for_block(after_id, 5).unwrap(),
            affinity: crate::model::selection::Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = document.extract_selection_fragment(&selection).unwrap();
        assert!(matches!(fragment.blocks[1], ClipboardBlock::Blockquote(_)));
        assert!(matches!(fragment.blocks[2], ClipboardBlock::HorizontalRule));
        assert!(matches!(fragment.blocks[3], ClipboardBlock::Table(_)));
        assert_eq!(
            fragment.to_plain_text(),
            "before\ninside one\ninside two\n---\ntable cell\nafter"
        );
    }
    #[test]
    fn selection_ending_at_next_block_start_keeps_the_boundary_closed() {
        let mut document = Document::new();
        let first_id = document.add_paragraph(InlineContent::plain("first"), BlockStyle::default());
        let second_id =
            document.add_paragraph(InlineContent::plain("second"), BlockStyle::default());
        let selection = DocumentSelection::Text {
            anchor: document.doc_pos_for_block(first_id, 0).unwrap(),
            focus: document.doc_pos_for_block(second_id, 0).unwrap(),
            affinity: crate::model::selection::Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = document.extract_selection_fragment(&selection).unwrap();
        assert!(!fragment.end_open);
        assert!(!fragment.is_open_inline());
    }
    #[test]
    fn selection_inside_both_paragraph_edges_is_open() {
        let mut document = Document::new();
        let first_id =
            document.add_paragraph(InlineContent::plain("first text"), BlockStyle::default());
        let second_id =
            document.add_paragraph(InlineContent::plain("second text"), BlockStyle::default());
        let selection = DocumentSelection::Text {
            anchor: document.doc_pos_for_block(first_id, 2).unwrap(),
            focus: document.doc_pos_for_block(second_id, 4).unwrap(),
            affinity: crate::model::selection::Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = document.extract_selection_fragment(&selection).unwrap();
        assert!(fragment.start_open);
        assert!(fragment.end_open);
        assert!(!fragment.is_open_inline());
    }
    #[test]
    fn partial_container_copy_keeps_tree_and_trims_text_edges() {
        let mut document = Document::new();
        let first_id = document.alloc_block_id();
        let second_id = document.alloc_block_id();
        let container_id = document.alloc_block_id();
        document.push_block(BlockNode::new(
            container_id,
            BlockKind::blockquote(vec![
                BlockNode::new(
                    first_id,
                    BlockKind::paragraph(InlineContent::plain("first")),
                    BlockStyle::default(),
                ),
                BlockNode::new(
                    second_id,
                    BlockKind::paragraph(InlineContent::plain("second")),
                    BlockStyle::default(),
                ),
            ]),
            BlockStyle::default(),
        ));
        let selection = DocumentSelection::Text {
            anchor: document.doc_pos_for_block(first_id, 2).unwrap(),
            focus: document.doc_pos_for_block(second_id, 3).unwrap(),
            affinity: crate::model::selection::Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = document.extract_selection_fragment(&selection).unwrap();
        let ClipboardBlock::Blockquote(children) = &fragment.blocks[0] else {
            panic!("expected a trimmed blockquote");
        };
        assert_eq!(
            block_to_plain_text(&children[0], &DefaultClipboardFormatter),
            "rst"
        );
        assert_eq!(
            block_to_plain_text(&children[1], &DefaultClipboardFormatter),
            "sec"
        );
    }
}
