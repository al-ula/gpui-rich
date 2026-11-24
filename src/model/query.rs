//! Pure-Rust CQRS query model for inspecting [`Document`] and [`DocumentSelection`] state,
//! extracting derived views, exporting via [`DocumentCodec`], and defining typed
//! [`ExtensionQuery`] contracts.

use crate::model::anchors::{AnchorId, Bookmark, footnote_reference_id_from_payload};
use crate::model::block::{BlockId, BlockKind, BlockNode, Document, ListKind, block_kinds};
use crate::model::clipboard::{
    ClipboardCustomFormatter, ClipboardFragment, DefaultClipboardFormatter,
};
use crate::model::codec::{CodecError, DocumentCodec, DynamicDocumentCodec, EncodedDocument};
use crate::model::command::MarkKind;
use crate::model::components::{ComponentStore, DROPDOWN_KIND, PLACEHOLDER_KIND, VARIABLE_KIND};
use crate::model::inline::OBJECT_REPLACEMENT_CHAR;
use crate::model::navigation::{SearchMatch, SearchOptions, find_matches};
use crate::model::payload::CustomPayload;
use crate::model::selection::{DocPos, DocumentSelection, PathSegment, SelectionValue};
use crate::model::style::{LinkTarget, ResolvedTextStyle, RgbaColor, SharedStr, TextAlign};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

/// Typed downstream read-only query registered on an `ExtensionRegistry` and executed on `&self`.
pub trait ExtensionQuery: Any + Send + Sync + fmt::Debug + 'static {
    /// Stable dot-namespaced query identifier (for example `"acme.callout.count"`).
    const ID: &'static str;
    /// Concrete Rust value returned to callers.
    type Output: Any + 'static;
}

/// Failure returned when executing a registered [`ExtensionQuery`] fails lookup or type checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryError {
    /// No handler is registered for this query identifier.
    UnregisteredQuery,
    /// The query identifier is registered for a different Rust query or output type.
    MismatchedQueryType,
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnregisteredQuery => formatter.write_str("extension query is not registered"),
            Self::MismatchedQueryType => {
                formatter.write_str("extension query type or output type mismatch")
            }
        }
    }
}

impl std::error::Error for QueryError {}

/// Read-only pure-model context supplied when executing a [`ModelQuery`].
pub struct ModelQueryContext<'document> {
    document: &'document Document,
    selection: Option<&'document DocumentSelection>,
    clipboard_formatter: Option<&'document dyn ClipboardCustomFormatter>,
}

impl<'document> ModelQueryContext<'document> {
    /// Creates a model query context for `document` and optional `selection`.
    pub fn new(
        document: &'document Document,
        selection: Option<&'document DocumentSelection>,
    ) -> Self {
        Self {
            document,
            selection,
            clipboard_formatter: None,
        }
    }

    /// Attaches a custom clipboard formatter (such as a view's extension/component formatter).
    pub fn with_clipboard_formatter(
        mut self,
        formatter: &'document dyn ClipboardCustomFormatter,
    ) -> Self {
        self.clipboard_formatter = Some(formatter);
        self
    }

    /// Borrows the queried [`Document`].
    pub fn document(&self) -> &'document Document {
        self.document
    }

    /// Borrows the active [`DocumentSelection`], if any.
    pub fn selection(&self) -> Option<&'document DocumentSelection> {
        self.selection
    }

    /// Borrows the attached [`ClipboardCustomFormatter`], if any.
    pub fn clipboard_formatter(&self) -> Option<&'document dyn ClipboardCustomFormatter> {
        self.clipboard_formatter
    }

    /// Executes a [`ModelQuery`] against this context.
    pub fn query<Q: ModelQuery>(&self, query: Q) -> Q::Output {
        query.execute(self)
    }
}

impl Document {
    /// Executes a pure-model [`ModelQuery`] against this document and optional `selection`.
    pub fn query<Q: ModelQuery>(
        &self,
        selection: Option<&DocumentSelection>,
        query: Q,
    ) -> Q::Output {
        ModelQueryContext::new(self, selection).query(query)
    }
}

/// Pure-Rust read-only query evaluated against [`ModelQueryContext`].
///
/// Every `ModelQuery` can be executed both headlessly on [`Document`] / [`ModelQueryContext`] and
/// through `RichDocumentView::query`.
pub trait ModelQuery {
    /// Stable dot-namespaced identifier for diagnostics and tooling.
    const ID: &'static str;
    /// Typed value produced by this query.
    type Output;

    /// Evaluates this query against `context`.
    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output;
}

/// Describes the active table cell or grid selection context for host toolbars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTableContext {
    pub table_id: BlockId,
    pub row: usize,
    pub visual_col: usize,
    pub cell_index: usize,
    pub can_merge: bool,
    pub can_split: bool,
}

/// High-level classification of the active selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectionKind {
    None,
    Caret,
    TextRange,
    TableCells,
}

/// Snapshot of active inline and block formatting across the current caret or selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormattingSnapshot {
    pub active_inline_style: Option<ResolvedTextStyle>,
    pub active_link: Option<LinkTarget>,
    pub bold: Option<SelectionValue<bool>>,
    pub italic: Option<SelectionValue<bool>>,
    pub underline: Option<SelectionValue<bool>>,
    pub strikethrough: Option<SelectionValue<bool>>,
    pub inline_code: Option<SelectionValue<bool>>,
    pub superscript: Option<SelectionValue<bool>>,
    pub subscript: Option<SelectionValue<bool>>,
    pub font_family: Option<SelectionValue<SharedStr>>,
    pub font_size_pt: Option<SelectionValue<f32>>,
    pub text_color: Option<SelectionValue<RgbaColor>>,
    pub text_align: Option<SelectionValue<TextAlign>>,
    pub heading_level: Option<SelectionValue<Option<u8>>>,
    pub active_list_kind: Option<ListKind>,
    pub in_blockquote: Option<SelectionValue<bool>>,
    pub active_code_block: Option<(BlockId, Option<String>)>,
}

impl FormattingSnapshot {
    /// Returns the tri-state selection status for `mark`.
    pub const fn mark_state(&self, mark: MarkKind) -> Option<SelectionValue<bool>> {
        match mark {
            MarkKind::Bold => self.bold,
            MarkKind::Italic => self.italic,
            MarkKind::Underline => self.underline,
            MarkKind::Strikethrough => self.strikethrough,
            MarkKind::InlineCode => self.inline_code,
            MarkKind::Superscript => self.superscript,
            MarkKind::Subscript => self.subscript,
        }
    }

    /// Returns `true` when `mark` is uniformly active across the caret or selection.
    pub fn is_mark_active(&self, mark: MarkKind) -> bool {
        self.mark_state(mark) == Some(SelectionValue::Uniform(true))
    }
}

/// Query returning [`FormattingSnapshot`] for the active caret or selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormattingState;

impl ModelQuery for FormattingState {
    const ID: &'static str = "rich.query.formatting_state";
    type Output = FormattingSnapshot;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        compute_formatting_snapshot(context.document(), context.selection())
    }
}

/// Snapshot describing the current selection, focused leaf block, and table cell context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionSnapshot {
    pub selection: Option<DocumentSelection>,
    pub kind: SelectionKind,
    pub active_block_id: Option<BlockId>,
    pub table_context: Option<ActiveTableContext>,
}

/// Query returning [`SelectionSnapshot`] for the active selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionState;

impl ModelQuery for SelectionState {
    const ID: &'static str = "rich.query.selection_state";
    type Output = SelectionSnapshot;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let selection = context.selection();
        let (kind, active_block_id) = match selection {
            None => (SelectionKind::None, None),
            Some(DocumentSelection::Text { anchor, focus, .. }) => {
                let kind = if anchor == focus {
                    SelectionKind::Caret
                } else {
                    SelectionKind::TextRange
                };
                (kind, focus.leaf_block_id())
            }
            Some(DocumentSelection::TableCells {
                table_id,
                focus_cell,
                ..
            }) => {
                let block_id = first_pos_in_table_cell(context.document(), *table_id, *focus_cell)
                    .and_then(|pos| pos.leaf_block_id());
                (SelectionKind::TableCells, block_id)
            }
        };
        SelectionSnapshot {
            selection: selection.cloned(),
            kind,
            active_block_id,
            table_context: compute_active_table_context(context.document(), selection),
        }
    }
}

/// Query returning the active selection as a structured [`ClipboardFragment`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedFragment;

impl ModelQuery for SelectedFragment {
    const ID: &'static str = "rich.query.selected_fragment";
    type Output = Option<ClipboardFragment>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let selection = context.selection()?;
        context.document().extract_selection_fragment(selection)
    }
}

/// Query returning the active selection as plain UTF-8 text (TSV for multi-cell tables).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedPlainText;

impl ModelQuery for SelectedPlainText {
    const ID: &'static str = "rich.query.selected_plain_text";
    type Output = Option<String>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let fragment = SelectedFragment.execute(context)?;
        if let Some(formatter) = context.clipboard_formatter() {
            Some(fragment.to_plain_text_with(formatter))
        } else {
            let fallback = ModelClipboardFormatter::new(context.document());
            Some(fragment.to_plain_text_with(&fallback))
        }
    }
}

/// Query returning the active selection as normalized semantic HTML.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedHtml;

impl ModelQuery for SelectedHtml {
    const ID: &'static str = "rich.query.selected_html";
    type Output = Option<String>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let fragment = SelectedFragment.execute(context)?;
        if let Some(formatter) = context.clipboard_formatter() {
            Some(fragment.to_html_with(formatter))
        } else {
            let fallback = ModelClipboardFormatter::new(context.document());
            Some(fragment.to_html_with(&fallback))
        }
    }
}

/// Query returning the entire document as plain UTF-8 text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlainText;

impl ModelQuery for PlainText {
    const ID: &'static str = "rich.query.plain_text";
    type Output = String;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let fragment = ClipboardFragment::from_document(context.document());
        if let Some(formatter) = context.clipboard_formatter() {
            fragment.to_plain_text_with(formatter)
        } else {
            let fallback = ModelClipboardFormatter::new(context.document());
            fragment.to_plain_text_with(&fallback)
        }
    }
}

/// Heading entry in a document outline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub block_id: BlockId,
    pub level: u8,
    pub text: String,
    pub collapsed: bool,
    pub visible: bool,
}

/// Query returning all document headings in reading order along with collapse visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOutline;

impl ModelQuery for DocumentOutline {
    const ID: &'static str = "rich.query.document_outline";
    type Output = Vec<OutlineEntry>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let document = context.document();
        let mut entries = Vec::new();
        for (block, visible) in document.blocks.iter().zip(document.top_level_visibility()) {
            collect_outline_entries(block, visible, &mut entries);
        }
        entries
    }
}

/// Aggregate structural and text counts across a [`Document`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentStatsSnapshot {
    pub word_count: usize,
    pub char_count: usize,
    pub block_count: usize,
    pub leaf_block_count: usize,
    pub heading_count: usize,
    pub table_count: usize,
}

/// Query returning [`DocumentStatsSnapshot`] for the current document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentStats;

impl ModelQuery for DocumentStats {
    const ID: &'static str = "rich.query.document_stats";
    type Output = DocumentStatsSnapshot;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let mut stats = DocumentStatsSnapshot::default();
        collect_document_stats(&context.document().blocks, &mut stats);
        stats
    }
}

/// Numbered footnote summary combining reading-order numbering and footnote body identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentFootnoteEntry {
    pub id: AnchorId,
    pub number: u32,
    pub body_block_id: BlockId,
}

/// Snapshot of durable bookmarks and reading-order numbered footnotes in a [`Document`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentAnchorsSnapshot {
    pub bookmarks: Vec<Bookmark>,
    pub footnotes: Vec<DocumentFootnoteEntry>,
}

/// Query returning bookmarks and numbered footnotes stored on the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentAnchors;

impl ModelQuery for DocumentAnchors {
    const ID: &'static str = "rich.query.document_anchors";
    type Output = DocumentAnchorsSnapshot;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        let document = context.document();
        let bookmarks = document.anchors.bookmarks().cloned().collect();
        let footnotes = document
            .anchors
            .numbered_document_notes(document)
            .into_iter()
            .filter_map(|note| {
                let body_block_id = document.anchors.footnote(note.id)?.block_id;
                Some(DocumentFootnoteEntry {
                    id: note.id,
                    number: note.number,
                    body_block_id,
                })
            })
            .collect();
        DocumentAnchorsSnapshot {
            bookmarks,
            footnotes,
        }
    }
}

/// Query returning the document's dropdown definitions and shared variables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentComponents;

impl ModelQuery for DocumentComponents {
    const ID: &'static str = "rich.query.document_components";
    type Output = ComponentStore;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        context.document().components.clone()
    }
}

/// Query finding all matches for `query` under `options` across the document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindMatches {
    pub query: String,
    pub options: SearchOptions,
}

impl FindMatches {
    /// Creates a search query using [`SearchOptions::default`].
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            options: SearchOptions::default(),
        }
    }

    /// Overrides the [`SearchOptions`] used for matching.
    pub fn with_options(mut self, options: SearchOptions) -> Self {
        self.options = options;
        self
    }
}

impl ModelQuery for FindMatches {
    const ID: &'static str = "rich.query.find_matches";
    type Output = Vec<SearchMatch>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        find_matches(context.document(), &self.query, self.options)
    }
}

/// Format-typed document export query over any [`DocumentCodec`].
#[derive(Clone, Copy, Debug)]
pub struct ExportDocument<'codec, C: DocumentCodec>(pub &'codec C);

impl<C: DocumentCodec> ModelQuery for ExportDocument<'_, C> {
    const ID: &'static str = "rich.query.export_document";
    type Output = Result<EncodedDocument<C::Format>, CodecError>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        self.0.encode(context.document())
    }
}

/// Dynamically dispatched document export query over a [`DynamicDocumentCodec`].
#[derive(Clone, Copy)]
pub struct ExportDocumentDynamic<'codec>(pub &'codec dyn DynamicDocumentCodec);

impl ModelQuery for ExportDocumentDynamic<'_> {
    const ID: &'static str = "rich.query.export_document_dynamic";
    type Output = Result<String, CodecError>;

    fn execute(self, context: &ModelQueryContext<'_>) -> Self::Output {
        self.0.encode_raw(context.document())
    }
}

struct ModelClipboardFormatter<'document> {
    document: &'document Document,
    note_numbers: OnceCell<HashMap<AnchorId, u32>>,
}

impl<'document> ModelClipboardFormatter<'document> {
    fn new(document: &'document Document) -> Self {
        Self {
            document,
            note_numbers: OnceCell::new(),
        }
    }

    fn footnote_label(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        let id = footnote_reference_id_from_payload(kind, data)?;
        let numbers = self
            .note_numbers
            .get_or_init(|| self.document.anchors.footnote_number_map(self.document));
        Some(
            numbers
                .get(&id)
                .map_or_else(|| "[?]".to_owned(), |number| format!("[{number}]")),
        )
    }
}

impl ClipboardCustomFormatter for ModelClipboardFormatter<'_> {
    fn plain_text_block(
        &self,
        shape: crate::model::BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        DefaultClipboardFormatter.plain_text_block(shape, kind, data)
    }

    fn html_block(
        &self,
        shape: crate::model::BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        DefaultClipboardFormatter.html_block(shape, kind, data)
    }

    fn plain_text_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        if matches!(kind, DROPDOWN_KIND | VARIABLE_KIND | PLACEHOLDER_KIND) {
            let json = data.to_json().ok()?;
            return Some(self.document.components.inline_fallback(kind, &json));
        }
        self.footnote_label(kind, data)
    }

    fn html_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.footnote_label(kind, data)
            .map(|label| format!("<sup>{label}</sup>"))
    }
}

fn collect_outline_entries(block: &BlockNode, visible: bool, out: &mut Vec<OutlineEntry>) {
    match &block.kind {
        BlockKind::Text { content, .. } => {
            if let Some(heading) = block.kind.heading_data() {
                let text = content.text.replace(OBJECT_REPLACEMENT_CHAR, "");
                out.push(OutlineEntry {
                    block_id: block.id,
                    level: heading.level,
                    text,
                    collapsed: heading.collapsed,
                    visible,
                });
            }
        }
        BlockKind::Container { children, .. } => {
            for child in children {
                collect_outline_entries(child, visible, out);
            }
        }
        BlockKind::Grid { table, .. } => {
            for row in &table.rows {
                for cell in &row.cells {
                    for child in &cell.blocks {
                        collect_outline_entries(child, visible, out);
                    }
                }
            }
        }
        BlockKind::Leaf { .. } => {}
    }
}

fn collect_document_stats(blocks: &[BlockNode], stats: &mut DocumentStatsSnapshot) {
    for block in blocks {
        stats.block_count += 1;
        match &block.kind {
            BlockKind::Text { content, .. } => {
                stats.leaf_block_count += 1;
                if block.kind.heading_data().is_some() {
                    stats.heading_count += 1;
                }
                count_text_stats(&content.text, stats);
            }
            BlockKind::Container { children, .. } => {
                collect_document_stats(children, stats);
            }
            BlockKind::Grid { table, .. } => {
                stats.table_count += 1;
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_document_stats(&cell.blocks, stats);
                    }
                }
            }
            BlockKind::Leaf { .. } => {
                stats.leaf_block_count += 1;
            }
        }
    }
}

fn count_text_stats(text: &str, stats: &mut DocumentStatsSnapshot) {
    let mut in_word = false;
    for character in text.chars() {
        if character == OBJECT_REPLACEMENT_CHAR {
            in_word = false;
            continue;
        }
        stats.char_count += 1;
        if character.is_whitespace() {
            in_word = false;
        } else if !in_word {
            stats.word_count += 1;
            in_word = true;
        }
    }
}

fn table_cell_of(pos: &DocPos) -> Option<(BlockId, usize, usize)> {
    pos.path.iter().find_map(|segment| match *segment {
        PathSegment::TableCell { table_id, row, col } => Some((table_id, row, col)),
        PathSegment::Block(_) => None,
    })
}

fn first_pos_in_table_cell(
    document: &Document,
    table_id: BlockId,
    cell_coords: (usize, usize),
) -> Option<DocPos> {
    let table = document.find_block(table_id)?.kind.table_block()?;
    let (_, placements, occupancy) = table.resolve_grid();
    let placement_index = occupancy
        .get(cell_coords.0)
        .and_then(|row| row.get(cell_coords.1))
        .and_then(|slot| *slot)?;
    let placement = placements.get(placement_index)?;
    let first_block = table
        .rows
        .get(placement.row_index)?
        .cells
        .get(placement.cell_index)?
        .blocks
        .first()?;
    document.doc_pos_for_block(first_block.id, 0)
}

pub(crate) fn compute_active_table_context(
    document: &Document,
    selection: Option<&DocumentSelection>,
) -> Option<ActiveTableContext> {
    match selection? {
        DocumentSelection::TableCells {
            table_id,
            anchor_cell,
            focus_cell,
        } => {
            let table = document.find_block(*table_id)?.kind.table_block()?;
            let (_, placements, occupancy) = table.resolve_grid();
            let min_row = anchor_cell.0.min(focus_cell.0);
            let min_col = anchor_cell.1.min(focus_cell.1);
            let placement_index = occupancy
                .get(min_row)
                .and_then(|row| row.get(min_col))
                .and_then(|slot| *slot)?;
            let placement = placements.get(placement_index)?;
            Some(ActiveTableContext {
                table_id: *table_id,
                row: placement.row_index,
                visual_col: placement.visual_col,
                cell_index: placement.cell_index,
                can_merge: anchor_cell != focus_cell,
                can_split: placement.colspan > 1 || placement.rowspan > 1,
            })
        }
        DocumentSelection::Text { focus, .. } => {
            let (table_id, row, cell_index) = table_cell_of(focus)?;
            let table = document.find_block(table_id)?.kind.table_block()?;
            let placement = table.placement_for_stored_cell(row, cell_index)?;
            Some(ActiveTableContext {
                table_id,
                row: placement.row_index,
                visual_col: placement.visual_col,
                cell_index: placement.cell_index,
                can_merge: false,
                can_split: placement.colspan > 1 || placement.rowspan > 1,
            })
        }
    }
}

fn active_text_selection(
    document: &Document,
    selection: Option<&DocumentSelection>,
) -> Option<(DocPos, DocPos, Option<crate::model::InlineStyle>)> {
    match selection? {
        DocumentSelection::Text {
            anchor,
            focus,
            stored_marks,
            ..
        } => Some((anchor.clone(), focus.clone(), stored_marks.clone())),
        DocumentSelection::TableCells {
            table_id,
            anchor_cell,
            ..
        } => {
            let pos = first_pos_in_table_cell(document, *table_id, *anchor_cell)?;
            Some((pos.clone(), pos, None))
        }
    }
}

fn ordered_positions(document: &Document, first: &DocPos, second: &DocPos) -> (DocPos, DocPos) {
    let leaves = document.collect_leaf_blocks();
    let first_index = first
        .leaf_block_id()
        .and_then(|id| leaves.iter().position(|block| block.id == id))
        .unwrap_or(0);
    let second_index = second
        .leaf_block_id()
        .and_then(|id| leaves.iter().position(|block| block.id == id))
        .unwrap_or(0);
    if (first_index, first.offset) <= (second_index, second.offset) {
        (first.clone(), second.clone())
    } else {
        (second.clone(), first.clone())
    }
}

fn covered_leaf_ranges(
    document: &Document,
    anchor: &DocPos,
    focus: &DocPos,
) -> Vec<(BlockId, Range<usize>)> {
    let (Some(anchor_leaf_id), Some(focus_leaf_id)) =
        (anchor.leaf_block_id(), focus.leaf_block_id())
    else {
        return Vec::new();
    };
    let leaves = document.collect_leaf_blocks();
    let (Some(anchor_index), Some(focus_index)) = (
        leaves.iter().position(|block| block.id == anchor_leaf_id),
        leaves.iter().position(|block| block.id == focus_leaf_id),
    ) else {
        return Vec::new();
    };
    let ((start_index, start_offset), (end_index, end_offset)) =
        if (anchor_index, anchor.offset) <= (focus_index, focus.offset) {
            ((anchor_index, anchor.offset), (focus_index, focus.offset))
        } else {
            ((focus_index, focus.offset), (anchor_index, anchor.offset))
        };

    let mut covered = Vec::new();
    for index in start_index..=end_index {
        let Some(block) = leaves.get(index) else {
            continue;
        };
        let Some(inline) = block.kind.inline_content() else {
            continue;
        };
        let range_start = if index == start_index {
            start_offset.min(inline.len())
        } else {
            0
        };
        let range_end = if index == end_index {
            end_offset.min(inline.len())
        } else {
            inline.len()
        };
        if range_start <= range_end {
            covered.push((block.id, range_start..range_end));
        }
    }
    covered
}

fn resolve_active_inline_style(
    document: &Document,
    anchor: &DocPos,
    focus: &DocPos,
    stored_marks: Option<&crate::model::InlineStyle>,
) -> Option<ResolvedTextStyle> {
    let is_collapsed = anchor == focus;
    let pos = if is_collapsed {
        focus.clone()
    } else {
        ordered_positions(document, anchor, focus).0
    };
    let block_id = pos.leaf_block_id()?;
    let block = document.find_block(block_id)?;
    let inline = block.kind.inline_content()?;
    let base_style = block
        .style
        .resolved_base_text(&document.stylesheet, &document.config.default_style);
    let local_style = if is_collapsed {
        stored_marks
            .cloned()
            .unwrap_or_else(|| inline.style_at_caret(pos.offset))
    } else {
        inline.style_at(pos.offset)
    };
    Some(local_style.resolve(&base_style))
}

fn collect_resolved_styles(
    document: &Document,
    anchor: &DocPos,
    focus: &DocPos,
    active_inline_style: Option<&ResolvedTextStyle>,
) -> Option<Vec<ResolvedTextStyle>> {
    if anchor == focus {
        return active_inline_style.cloned().map(|style| vec![style]);
    }
    let covered = covered_leaf_ranges(document, anchor, focus);
    let mut styles = Vec::new();
    for (block_id, range) in covered {
        if range.is_empty() {
            continue;
        }
        let Some(block) = document.find_block(block_id) else {
            continue;
        };
        let Some(inline) = block.kind.inline_content() else {
            continue;
        };
        let base_style = block
            .style
            .resolved_base_text(&document.stylesheet, &document.config.default_style);
        for span in &inline.spans {
            if span.range.end <= range.start || span.range.start >= range.end {
                continue;
            }
            styles.push(span.style.resolve(&base_style));
        }
    }
    if styles.is_empty() {
        active_inline_style.cloned().map(|style| vec![style])
    } else {
        Some(styles)
    }
}

fn uniform_or_mixed<T: PartialEq>(
    mut values: impl Iterator<Item = T>,
) -> Option<SelectionValue<T>> {
    let first = values.next()?;
    if values.all(|value| value == first) {
        Some(SelectionValue::Uniform(first))
    } else {
        Some(SelectionValue::Mixed)
    }
}

fn mark_state_from_styles(
    styles: Option<&[ResolvedTextStyle]>,
    mark: MarkKind,
) -> Option<SelectionValue<bool>> {
    let styles = styles?;
    let first = styles.first()?.is_mark_active(mark);
    if styles
        .iter()
        .all(|style| style.is_mark_active(mark) == first)
    {
        Some(SelectionValue::Uniform(first))
    } else {
        Some(SelectionValue::Mixed)
    }
}

pub(crate) fn compute_formatting_snapshot(
    document: &Document,
    selection: Option<&DocumentSelection>,
) -> FormattingSnapshot {
    let Some((anchor, focus, stored_marks)) = active_text_selection(document, selection) else {
        return FormattingSnapshot {
            active_inline_style: None,
            active_link: None,
            bold: None,
            italic: None,
            underline: None,
            strikethrough: None,
            inline_code: None,
            superscript: None,
            subscript: None,
            font_family: None,
            font_size_pt: None,
            text_color: None,
            text_align: None,
            heading_level: None,
            active_list_kind: None,
            in_blockquote: None,
            active_code_block: None,
        };
    };

    let active_inline_style =
        resolve_active_inline_style(document, &anchor, &focus, stored_marks.as_ref());
    let active_link = active_inline_style
        .as_ref()
        .and_then(|style| style.link.clone());
    let styles = collect_resolved_styles(document, &anchor, &focus, active_inline_style.as_ref());
    let styles_slice = styles.as_deref();

    let font_family = styles_slice
        .and_then(|list| uniform_or_mixed(list.iter().map(|style| style.font_family.clone())));
    let font_size_pt = styles_slice.and_then(|list| {
        let first = list.first()?.font_size_pt;
        if list
            .iter()
            .all(|style| (style.font_size_pt - first).abs() < 0.05)
        {
            Some(SelectionValue::Uniform(first))
        } else {
            Some(SelectionValue::Mixed)
        }
    });
    let text_color =
        styles_slice.and_then(|list| uniform_or_mixed(list.iter().map(|style| style.color)));

    let selected_block_ids: Vec<BlockId> = if anchor == focus {
        focus.leaf_block_id().into_iter().collect()
    } else {
        covered_leaf_ranges(document, &anchor, &focus)
            .into_iter()
            .map(|(block_id, _)| block_id)
            .collect()
    };

    let text_align = uniform_or_mixed(
        selected_block_ids
            .iter()
            .filter_map(|id| document.find_block(*id).map(|block| block.style.align)),
    );
    let heading_level = uniform_or_mixed(selected_block_ids.iter().filter_map(|id| {
        document
            .find_block(*id)
            .map(|block| block.kind.heading_data().map(|heading| heading.level))
    }));
    let in_blockquote = uniform_or_mixed(selected_block_ids.iter().map(|id| {
        document.path_for_block(*id).is_some_and(|path| {
            path.iter().rev().skip(1).any(|segment| match *segment {
                PathSegment::Block(ancestor_id) => document
                    .find_block(ancestor_id)
                    .is_some_and(|block| block.kind.semantic_kind() == block_kinds::BLOCKQUOTE),
                PathSegment::TableCell { .. } => false,
            })
        })
    }));

    let focus_block = focus
        .leaf_block_id()
        .and_then(|block_id| document.find_block(block_id));
    let active_list_kind = focus_block
        .and_then(|block| block.kind.list_item_data())
        .map(|item| item.kind);
    let active_code_block = focus_block.and_then(|block| {
        block
            .kind
            .code_block_data()
            .map(|code| (block.id, code.language.clone()))
    });

    FormattingSnapshot {
        active_inline_style,
        active_link,
        bold: mark_state_from_styles(styles_slice, MarkKind::Bold),
        italic: mark_state_from_styles(styles_slice, MarkKind::Italic),
        underline: mark_state_from_styles(styles_slice, MarkKind::Underline),
        strikethrough: mark_state_from_styles(styles_slice, MarkKind::Strikethrough),
        inline_code: mark_state_from_styles(styles_slice, MarkKind::InlineCode),
        superscript: mark_state_from_styles(styles_slice, MarkKind::Superscript),
        subscript: mark_state_from_styles(styles_slice, MarkKind::Subscript),
        font_family,
        font_size_pt,
        text_color,
        text_align,
        heading_level,
        active_list_kind,
        in_blockquote,
        active_code_block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Affinity, AnchorPosition, BlockStyle, BoundaryAffinity, FontWeight, InlineContent,
        InlineStyle, MarkdownCodec, TableBlock, TableBorderStyle, TableCell, TableRow,
        footnote_body_kind, footnote_reference,
    };
    use std::collections::BTreeMap;

    #[test]
    fn formatting_and_selection_queries_reflect_collapsed_and_mixed_ranges() {
        let mut document = Document::new();
        let bold_style = InlineStyle {
            weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let italic_style = InlineStyle {
            italic: Some(true),
            ..Default::default()
        };
        let mut content = InlineContent::empty();
        content.push_text("Hello", bold_style.clone());
        content.push_text(" world", italic_style);
        let paragraph_id = document.add_paragraph(content, BlockStyle::default());

        let start = document.doc_pos_for_block(paragraph_id, 0).unwrap();
        let end = document.doc_pos_for_block(paragraph_id, 11).unwrap();
        let range_selection = DocumentSelection::Text {
            anchor: start.clone(),
            focus: end,
            affinity: Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };

        let formatting = document.query(Some(&range_selection), FormattingState);
        assert_eq!(formatting.bold, Some(SelectionValue::Mixed));
        assert_eq!(formatting.italic, Some(SelectionValue::Mixed));
        assert!(!formatting.is_mark_active(MarkKind::Bold));
        assert_eq!(
            document.query(Some(&range_selection), SelectedPlainText),
            Some("Hello world".to_owned())
        );

        let caret_selection = DocumentSelection::Text {
            anchor: start.clone(),
            focus: start,
            affinity: Affinity::Downstream,
            goal_x: None,
            stored_marks: Some(bold_style),
        };
        let caret_formatting = document.query(Some(&caret_selection), FormattingState);
        assert_eq!(caret_formatting.bold, Some(SelectionValue::Uniform(true)));
        assert!(caret_formatting.is_mark_active(MarkKind::Bold));

        let selection_snapshot = document.query(Some(&caret_selection), SelectionState);
        assert_eq!(selection_snapshot.kind, SelectionKind::Caret);
        assert_eq!(selection_snapshot.active_block_id, Some(paragraph_id));
        assert_eq!(selection_snapshot.table_context, None);
    }

    #[test]
    fn outline_stats_anchors_search_and_export_queries_work_headlessly() {
        let mut document = Document::new();
        let h1_id = document.alloc_block_id();
        document.blocks.push(BlockNode::new(
            h1_id,
            BlockKind::heading(1, true, InlineContent::plain("Intro")),
            BlockStyle::default(),
        ));
        let h2_id = document.add_heading(2, InlineContent::plain("Details"));
        let para_id =
            document.add_paragraph(InlineContent::plain("Alpha beta"), BlockStyle::default());

        let cell_id = document.alloc_block_id();
        let cell_block_id = document.alloc_block_id();
        let table = TableBlock {
            columns: Vec::new(),
            column_kinds: BTreeMap::new(),
            rows: vec![TableRow {
                is_header: false,
                cells: vec![TableCell::simple(
                    cell_id,
                    vec![BlockNode::new(
                        cell_block_id,
                        BlockKind::paragraph(InlineContent::plain("Cell")),
                        BlockStyle::default(),
                    )],
                )],
            }],
            border_style: TableBorderStyle::default(),
        };
        let table_id = document.alloc_block_id();
        document.blocks.push(BlockNode::new(
            table_id,
            BlockKind::table(table),
            BlockStyle::default(),
        ));

        let body_id = document.alloc_block_id();
        let body_para_id = document.alloc_block_id();
        let note_id = document.anchors.add_footnote_block(body_id);
        document.blocks.push(BlockNode::new(
            body_id,
            footnote_body_kind(vec![BlockNode::new(
                body_para_id,
                BlockKind::paragraph(InlineContent::plain("Note body")),
                BlockStyle::default(),
            )]),
            BlockStyle::default(),
        ));
        if let Some(paragraph) = document.find_block_mut(para_id)
            && let Some(inline) = paragraph.kind.inline_content_mut()
        {
            inline.push_object(footnote_reference(note_id), InlineStyle::default());
        }
        document.anchors.add_bookmark(
            "start",
            AnchorPosition {
                block_id: para_id,
                offset: 0,
            },
            BoundaryAffinity::Before,
        );

        let outline = document.query(None, DocumentOutline);
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].block_id, h1_id);
        assert!(outline[0].collapsed);
        assert!(outline[0].visible);
        assert_eq!(outline[1].block_id, h2_id);
        assert!(!outline[1].visible);

        let stats = document.query(None, DocumentStats);
        assert_eq!(stats.heading_count, 2);
        assert_eq!(stats.table_count, 1);
        assert_eq!(stats.word_count, 7);

        let anchors = document.query(None, DocumentAnchors);
        assert_eq!(anchors.bookmarks.len(), 1);
        assert_eq!(anchors.bookmarks[0].name, "start");
        assert_eq!(anchors.footnotes.len(), 1);
        assert_eq!(anchors.footnotes[0].id, note_id);
        assert_eq!(anchors.footnotes[0].number, 1);
        assert_eq!(anchors.footnotes[0].body_block_id, body_id);

        let matches = document.query(
            None,
            FindMatches::new("alpha").with_options(SearchOptions {
                visible_only: false,
                ..SearchOptions::default()
            }),
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].block_id, para_id);

        let exported = document
            .query(None, ExportDocument(&MarkdownCodec))
            .expect("markdown export");
        assert!(exported.as_str().contains("# Intro"));

        let table_selection = DocumentSelection::TableCells {
            table_id,
            anchor_cell: (0, 0),
            focus_cell: (0, 0),
        };
        let selection_state = document.query(Some(&table_selection), SelectionState);
        assert_eq!(selection_state.kind, SelectionKind::TableCells);
        assert_eq!(
            selection_state
                .table_context
                .map(|context| context.table_id),
            Some(table_id)
        );
    }
}
