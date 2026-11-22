use crate::block::{BlockId, BlockKind, BlockNode, ContainerTarget, Document};
use crate::inline::{InlineContent, StyleSpan};
use crate::selection::{Affinity, DocPos, DocumentSelection, PathSegment};
use crate::style::{BlockStyle, InlineStyle};
use smallvec::SmallVec;
use std::ops::Range;
use std::time::{Duration, Instant};

/// Atomic, reversible document mutation step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    InsertText {
        block_id: BlockId,
        offset: usize,
        text: String,
        style: Option<InlineStyle>,
    },
    InsertContent {
        block_id: BlockId,
        offset: usize,
        content: InlineContent,
    },
    DeleteRange {
        block_id: BlockId,
        range: Range<usize>,
    },
    ApplyInlineStyle {
        block_id: BlockId,
        range: Range<usize>,
        style: InlineStyle,
    },
    RestoreSpans {
        block_id: BlockId,
        spans: Vec<StyleSpan>,
    },
    SetBlockStyle {
        block_id: BlockId,
        style: BlockStyle,
    },
    SetBlockKind {
        block_id: BlockId,
        kind: BlockKind,
    },
    InsertBlock {
        index: usize,
        block: BlockNode,
    },
    RemoveBlock {
        index: usize,
    },
    InsertBlockIn {
        target: ContainerTarget,
        index: usize,
        block: BlockNode,
    },
    RemoveBlockIn {
        target: ContainerTarget,
        index: usize,
    },
    ToggleHeadingCollapsed {
        block_id: BlockId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub steps: Vec<Step>,
    pub selection_before: Option<DocumentSelection>,
    pub selection_after: Option<DocumentSelection>,
}

impl Transaction {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            selection_before: None,
            selection_after: None,
        }
    }

    pub fn with_selections(
        mut self,
        before: Option<DocumentSelection>,
        after: Option<DocumentSelection>,
    ) -> Self {
        self.selection_before = before;
        self.selection_after = after;
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub dirty_blocks: SmallVec<[BlockId; 4]>,
    pub structure_changed: bool,
}

impl ChangeSet {
    fn mark_dirty(&mut self, id: BlockId) {
        if !self.dirty_blocks.contains(&id) {
            self.dirty_blocks.push(id);
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppliedTransaction {
    pub change_set: ChangeSet,
    pub inverse: Transaction,
}

impl Document {
    pub fn apply(&mut self, tx: Transaction) -> AppliedTransaction {
        let mut change_set = ChangeSet::default();
        let mut inverse_steps = Vec::with_capacity(tx.steps.len());

        for step in tx.steps {
            match step {
                Step::InsertText {
                    block_id,
                    offset,
                    text,
                    style,
                } => {
                    let inserted_len = text.len();
                    let applied = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let clamped = offset.min(inline.len());
                        inline.insert_text(clamped, &text, style);
                        block.bump_version();
                        Some(clamped)
                    } else {
                        None
                    };
                    if let Some(clamped) = applied {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::DeleteRange {
                            block_id,
                            range: clamped..(clamped + inserted_len),
                        });
                    }
                }
                Step::InsertContent {
                    block_id,
                    offset,
                    content,
                } => {
                    let inserted_len = content.len();
                    let applied = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let clamped = offset.min(inline.len());
                        inline.insert_content(clamped, &content);
                        block.bump_version();
                        Some(clamped)
                    } else {
                        None
                    };
                    if let Some(clamped) = applied {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::DeleteRange {
                            block_id,
                            range: clamped..(clamped + inserted_len),
                        });
                    }
                }
                Step::DeleteRange { block_id, range } => {
                    let removed = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let start = range.start.min(inline.len());
                        let end = range.end.min(inline.len()).max(start);
                        let slice = inline.delete_range(start..end);
                        block.bump_version();
                        Some((start, slice))
                    } else {
                        None
                    };
                    if let Some((start, slice)) = removed {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::InsertContent {
                            block_id,
                            offset: start,
                            content: slice,
                        });
                    }
                }
                Step::ApplyInlineStyle {
                    block_id,
                    range,
                    style,
                } => {
                    let previous = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let start = range.start.min(inline.len());
                        let end = range.end.min(inline.len()).max(start);
                        let prev = inline.apply_style(start..end, &style);
                        block.bump_version();
                        Some(prev)
                    } else {
                        None
                    };
                    if let Some(previous) = previous {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::RestoreSpans {
                            block_id,
                            spans: previous,
                        });
                    }
                }
                Step::RestoreSpans { block_id, spans } => {
                    let prev = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let previous =
                            if let (Some(first), Some(last)) = (spans.first(), spans.last()) {
                                inline.apply_style(
                                    first.range.start..last.range.end,
                                    &InlineStyle::default(),
                                )
                            } else {
                                Vec::new()
                            };
                        inline.restore_spans(&spans);
                        block.bump_version();
                        Some(previous)
                    } else {
                        None
                    };
                    if let Some(previous) = prev {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::RestoreSpans {
                            block_id,
                            spans: previous,
                        });
                    }
                }
                Step::SetBlockStyle { block_id, style } => {
                    let old_style = self.find_block_mut(block_id).map(|block| {
                        let old = std::mem::replace(&mut block.style, style);
                        block.bump_version();
                        old
                    });
                    if let Some(old_style) = old_style {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::SetBlockStyle {
                            block_id,
                            style: old_style,
                        });
                    }
                }
                Step::SetBlockKind { block_id, kind } => {
                    let old_kind = self.find_block_mut(block_id).map(|block| {
                        let old = std::mem::replace(&mut block.kind, kind);
                        block.bump_version();
                        old
                    });
                    if let Some(old_kind) = old_kind {
                        change_set.mark_dirty(block_id);
                        change_set.structure_changed = true;
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::SetBlockKind {
                            block_id,
                            kind: old_kind,
                        });
                    }
                }
                Step::InsertBlock { index, block } => {
                    let clamped = index.min(self.blocks.len());
                    let id = block.id;
                    self.blocks.insert(clamped, block);
                    change_set.mark_dirty(id);
                    change_set.structure_changed = true;
                    inverse_steps.push(Step::RemoveBlock { index: clamped });
                }
                Step::RemoveBlock { index } => {
                    if index < self.blocks.len() {
                        let removed = self.blocks.remove(index);
                        change_set.mark_dirty(removed.id);
                        change_set.structure_changed = true;
                        inverse_steps.push(Step::InsertBlock {
                            index,
                            block: removed,
                        });
                    }
                }
                Step::InsertBlockIn {
                    target,
                    index,
                    block,
                } => {
                    let id = block.id;
                    let inserted_index = self.container_blocks_mut(target).map(|blocks| {
                        let clamped = index.min(blocks.len());
                        blocks.insert(clamped, block);
                        clamped
                    });
                    if let Some(clamped) = inserted_index {
                        change_set.mark_dirty(id);
                        change_set.structure_changed = true;
                        self.bump_ancestors_of_target(target, &mut change_set);
                        inverse_steps.push(Step::RemoveBlockIn {
                            target,
                            index: clamped,
                        });
                    }
                }
                Step::RemoveBlockIn { target, index } => {
                    let removed = self.container_blocks_mut(target).and_then(|blocks| {
                        if index < blocks.len() {
                            Some(blocks.remove(index))
                        } else {
                            None
                        }
                    });
                    if let Some(removed) = removed {
                        change_set.mark_dirty(removed.id);
                        change_set.structure_changed = true;
                        self.bump_ancestors_of_target(target, &mut change_set);
                        inverse_steps.push(Step::InsertBlockIn {
                            target,
                            index,
                            block: removed,
                        });
                    }
                }
                Step::ToggleHeadingCollapsed { block_id } => {
                    if self.toggle_heading_collapsed(block_id).is_some() {
                        change_set.mark_dirty(block_id);
                        change_set.structure_changed = true;
                        inverse_steps.push(Step::ToggleHeadingCollapsed { block_id });
                    }
                }
            }
        }

        inverse_steps.reverse();
        AppliedTransaction {
            change_set,
            inverse: Transaction {
                steps: inverse_steps,
                selection_before: tx.selection_after,
                selection_after: tx.selection_before,
            },
        }
    }

    /// Builds steps deleting the text range between `anchor` and `focus` (same-block or cross-block)
    /// and returns `(steps, collapsed_caret_pos)`.
    pub fn delete_selection_steps(
        &self,
        anchor: &DocPos,
        focus: &DocPos,
    ) -> Option<(Vec<Step>, DocPos)> {
        if anchor == focus {
            return Some((Vec::new(), anchor.clone()));
        }

        let anchor_leaf_id = anchor.leaf_block_id()?;
        let focus_leaf_id = focus.leaf_block_id()?;
        let leaves = self.collect_leaf_blocks();
        let anchor_order = leaves.iter().position(|block| block.id == anchor_leaf_id)?;
        let focus_order = leaves.iter().position(|block| block.id == focus_leaf_id)?;

        let ((start_order, start_pos), (end_order, end_pos)) =
            if (anchor_order, anchor.offset) <= (focus_order, focus.offset) {
                ((anchor_order, anchor.clone()), (focus_order, focus.clone()))
            } else {
                ((focus_order, focus.clone()), (anchor_order, anchor.clone()))
            };

        let start_id = start_pos.leaf_block_id()?;
        let end_id = end_pos.leaf_block_id()?;

        if start_id == end_id {
            let inline = self.find_block(start_id)?.kind.inline_content()?;
            let start_offset = start_pos.offset.min(inline.len());
            let end_offset = end_pos.offset.min(inline.len()).max(start_offset);
            let mut steps = Vec::new();
            if start_offset < end_offset {
                steps.push(Step::DeleteRange {
                    block_id: start_id,
                    range: start_offset..end_offset,
                });
            }
            let mut caret = start_pos;
            caret.offset = start_offset;
            return Some((steps, caret));
        }

        let start_inline = self.find_block(start_id)?.kind.inline_content()?;
        let end_inline = self.find_block(end_id)?.kind.inline_content()?;
        let start_offset = start_pos.offset.min(start_inline.len());
        let end_offset = end_pos.offset.min(end_inline.len());

        let (start_target, start_container_index) = self.locate_block(start_id)?;
        let (end_target, end_container_index) = self.locate_block(end_id)?;

        let mut steps = Vec::new();

        if start_target == end_target && start_container_index < end_container_index {
            // Merge trailing content of `end_id` onto `start_id` and remove all covered blocks in reverse order.
            if start_offset < start_inline.len() {
                steps.push(Step::DeleteRange {
                    block_id: start_id,
                    range: start_offset..start_inline.len(),
                });
            }
            if end_offset < end_inline.len() {
                let trailing = end_inline.slice(end_offset..end_inline.len());
                steps.push(Step::InsertContent {
                    block_id: start_id,
                    offset: start_offset,
                    content: trailing,
                });
            }
            for remove_index in ((start_container_index + 1)..=end_container_index).rev() {
                steps.push(Step::RemoveBlockIn {
                    target: start_target,
                    index: remove_index,
                });
            }
        } else {
            // Cross-container selection (e.g. prose into table cell): clear covered text without breaking containers.
            if start_offset < start_inline.len() {
                steps.push(Step::DeleteRange {
                    block_id: start_id,
                    range: start_offset..start_inline.len(),
                });
            }
            for leaf in leaves
                .iter()
                .take(end_order)
                .skip(start_order.saturating_add(1))
            {
                if let Some(inline) = leaf.kind.inline_content()
                    && !inline.is_empty()
                {
                    steps.push(Step::DeleteRange {
                        block_id: leaf.id,
                        range: 0..inline.len(),
                    });
                }
            }
            if end_offset > 0 {
                steps.push(Step::DeleteRange {
                    block_id: end_id,
                    range: 0..end_offset,
                });
            }
        }

        let mut caret = start_pos;
        caret.offset = start_offset;
        Some((steps, caret))
    }

    /// Builds a container-aware transaction splitting the block at `pos` (handling Enter in paragraphs,
    /// headings, list items, code blocks, blockquotes, and table cells).
    pub fn build_split_block_transaction(
        &mut self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let leaf_id = pos.leaf_block_id()?;
        let (target, block_index) = self.locate_block(leaf_id)?;
        let block = self.find_block(leaf_id)?.clone();
        let inline = block.kind.inline_content()?;
        let offset = pos.offset.min(inline.len());

        // Inside a code block, Enter inserts a newline rather than splitting into a second frame.
        if matches!(block.kind, BlockKind::CodeBlock { .. }) {
            let mut next_pos = pos.clone();
            next_pos.offset = offset + 1;
            return Some(
                Transaction::new(vec![Step::InsertText {
                    block_id: leaf_id,
                    offset,
                    text: "\n".into(),
                    style: None,
                }])
                .with_selections(
                    selection_before,
                    Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
                ),
            );
        }

        // Pressing Enter on an empty list item outdents or exits the list to a normal paragraph.
        if let BlockKind::ListItem {
            list_id,
            kind,
            indent_level,
            checked,
            content,
        } = &block.kind
            && content.is_empty()
        {
            let (new_kind, new_style) = if *indent_level > 0 {
                let next_level = indent_level - 1;
                let mut next_style = block.style.clone();
                next_style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
                (
                    BlockKind::ListItem {
                        list_id: *list_id,
                        kind: *kind,
                        indent_level: next_level,
                        checked: *checked,
                        content: InlineContent::empty(),
                    },
                    next_style,
                )
            } else {
                (
                    BlockKind::Paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                )
            };
            let mut next_pos = pos.clone();
            next_pos.offset = 0;
            return self.build_convert_block_transaction(
                leaf_id,
                new_kind,
                new_style,
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            );
        }

        let trailing = inline.slice(offset..inline.len());
        let new_block_id = self.alloc_block_id();
        let (trailing_kind, trailing_style) = match &block.kind {
            BlockKind::Paragraph(_) => (BlockKind::Paragraph(trailing), block.style.clone()),
            BlockKind::Heading { level, .. } => {
                if offset == inline.len() {
                    (
                        BlockKind::Paragraph(InlineContent::empty()),
                        BlockStyle::default(),
                    )
                } else {
                    (
                        BlockKind::Heading {
                            level: *level,
                            collapsed: false,
                            content: trailing,
                        },
                        block.style.clone(),
                    )
                }
            }
            BlockKind::ListItem {
                list_id,
                kind,
                indent_level,
                checked,
                ..
            } => (
                BlockKind::ListItem {
                    list_id: *list_id,
                    kind: *kind,
                    indent_level: *indent_level,
                    checked: checked.map(|_| false),
                    content: trailing,
                },
                block.style.clone(),
            ),
            _ => return None,
        };

        let mut steps = Vec::new();
        if offset < inline.len() {
            steps.push(Step::DeleteRange {
                block_id: leaf_id,
                range: offset..inline.len(),
            });
        }
        steps.push(Step::InsertBlockIn {
            target,
            index: block_index + 1,
            block: BlockNode::new(new_block_id, trailing_kind, trailing_style),
        });

        let mut next_path = pos.path.clone();
        if let Some(last_segment) = next_path.last_mut() {
            *last_segment = PathSegment::Block(new_block_id);
        }
        let next_pos = DocPos {
            path: next_path,
            offset: 0,
        };

        Some(
            Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            ),
        )
    }

    /// Builds a container-aware transaction merging the block at `pos` into its preceding sibling
    /// (used for Backspace at offset 0).
    pub fn build_merge_with_previous_transaction(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let leaf_id = pos.leaf_block_id()?;
        let (target, block_index) = self.locate_block(leaf_id)?;
        let block = self.find_block(leaf_id)?;

        // Backspace at offset 0 of a list item or heading converts/outdents before merging upward.
        match &block.kind {
            BlockKind::ListItem {
                list_id,
                kind,
                indent_level,
                checked,
                content,
            } => {
                let (new_kind, new_style) = if *indent_level > 0 {
                    let next_level = indent_level - 1;
                    let mut next_style = block.style.clone();
                    next_style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
                    (
                        BlockKind::ListItem {
                            list_id: *list_id,
                            kind: *kind,
                            indent_level: next_level,
                            checked: *checked,
                            content: content.clone(),
                        },
                        next_style,
                    )
                } else {
                    (
                        BlockKind::Paragraph(content.clone()),
                        BlockStyle::default(),
                    )
                };
                return self.build_convert_block_transaction(
                    leaf_id,
                    new_kind,
                    new_style,
                    selection_before,
                    Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                );
            }
            BlockKind::Heading { content, .. } => {
                return self.build_convert_block_transaction(
                    leaf_id,
                    BlockKind::Paragraph(content.clone()),
                    BlockStyle::default(),
                    selection_before,
                    Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                );
            }
            _ => {}
        }

        if block_index == 0 {
            return None;
        }
        let siblings = self.container_blocks(target)?;
        let prev_block = siblings.get(block_index - 1)?;
        let prev_inline = prev_block.kind.inline_content()?;
        let current_inline = block.kind.inline_content()?;
        let join_offset = prev_inline.len();

        let mut steps = Vec::new();
        if !current_inline.is_empty() {
            steps.push(Step::InsertContent {
                block_id: prev_block.id,
                offset: join_offset,
                content: current_inline.clone(),
            });
        }
        steps.push(Step::RemoveBlockIn {
            target,
            index: block_index,
        });

        let next_pos = self.doc_pos_for_block(prev_block.id, join_offset)?;
        Some(
            Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            ),
        )
    }

    /// Builds a container-aware transaction merging the next sibling into the block at `pos`
    /// (used for forward Delete at the end of a block).
    pub fn build_merge_with_next_transaction(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let leaf_id = pos.leaf_block_id()?;
        let (target, block_index) = self.locate_block(leaf_id)?;
        let siblings = self.container_blocks(target)?;
        let next_block = siblings.get(block_index + 1)?;
        let current_inline = siblings.get(block_index)?.kind.inline_content()?;
        let next_inline = next_block.kind.inline_content()?;
        let join_offset = current_inline.len();

        let mut steps = Vec::new();
        if !next_inline.is_empty() {
            steps.push(Step::InsertContent {
                block_id: leaf_id,
                offset: join_offset,
                content: next_inline.clone(),
            });
        }
        steps.push(Step::RemoveBlockIn {
            target,
            index: block_index + 1,
        });

        Some(
            Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
            ),
        )
    }

    /// Builds a transaction converting `block_id` to `new_kind` and `new_style`.
    pub fn build_convert_block_transaction(
        &self,
        block_id: BlockId,
        new_kind: BlockKind,
        new_style: BlockStyle,
        selection_before: Option<DocumentSelection>,
        selection_after: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let _ = self.find_block(block_id)?;
        Some(
            Transaction::new(vec![
                Step::SetBlockKind {
                    block_id,
                    kind: new_kind,
                },
                Step::SetBlockStyle {
                    block_id,
                    style: new_style,
                },
            ])
            .with_selections(selection_before, selection_after),
        )
    }

    fn bump_ancestors_of_block(&mut self, block_id: BlockId, change_set: &mut ChangeSet) {
        if let Some((target, _)) = self.locate_block(block_id) {
            self.bump_ancestors_of_target(target, change_set);
        }
    }

    fn bump_ancestors_of_target(&mut self, target: ContainerTarget, change_set: &mut ChangeSet) {
        let mut current = target;
        while let Some(parent_id) = match current {
            ContainerTarget::Root => None,
            ContainerTarget::Blockquote(quote_id) => Some(quote_id),
            ContainerTarget::TableCell { table_id, .. } => Some(table_id),
        } {
            if let Some(parent_block) = self.find_block_mut(parent_id) {
                parent_block.bump_version();
            }
            change_set.mark_dirty(parent_id);
            match self.locate_block(parent_id) {
                Some((parent_target, _)) => current = parent_target,
                None => break,
            }
        }
    }
}

/// Classifies an edit transaction for undo-history coalescing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryMergeGroup {
    /// Never merge with neighboring history entries.
    Atomic,
    /// Sequential character insertions inside a single block.
    Typing { block_id: BlockId },
    /// Sequential deletions inside a single block.
    Deleting { block_id: BlockId },
}

struct HistoryEntry {
    inverse: Transaction,
    group: HistoryMergeGroup,
    timestamp: Instant,
}

const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_millis(500);

pub struct History {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<Transaction>,
    coalesce_window: Duration,
    coalescing_broken: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            coalesce_window: DEFAULT_COALESCE_WINDOW,
            coalescing_broken: false,
        }
    }
}

impl History {
    /// Overrides the maximum interval between sequential edits in the same merge group.
    pub fn with_coalesce_window(mut self, window: Duration) -> Self {
        self.coalesce_window = window;
        self
    }

    /// Prevents the next recorded transaction from coalescing with the previous undo entry.
    pub fn break_coalescing(&mut self) {
        self.coalescing_broken = true;
    }

    /// Pushes an atomic (non-coalescing) inverse transaction onto the undo stack.
    pub fn push_inverse(&mut self, inverse: Transaction) {
        self.push_inverse_grouped_at(inverse, HistoryMergeGroup::Atomic, Instant::now());
    }

    /// Pushes an inverse transaction with coalescing metadata using the current clock time.
    pub fn push_inverse_grouped(&mut self, inverse: Transaction, group: HistoryMergeGroup) {
        self.push_inverse_grouped_at(inverse, group, Instant::now());
    }

    /// Pushes an inverse transaction at an explicit `timestamp` (deterministic for unit tests).
    pub fn push_inverse_grouped_at(
        &mut self,
        inverse: Transaction,
        group: HistoryMergeGroup,
        timestamp: Instant,
    ) {
        self.redo_stack.clear();

        let can_coalesce = !self.coalescing_broken
            && group != HistoryMergeGroup::Atomic
            && self.undo_stack.last().is_some_and(|previous| {
                previous.group == group
                    && timestamp.saturating_duration_since(previous.timestamp)
                        <= self.coalesce_window
            });

        if can_coalesce && let Some(previous) = self.undo_stack.last_mut() {
            // Execute the newer inverse first so undoing reverses edits in strict LIFO order.
            let mut combined_steps = inverse.steps;
            combined_steps.extend(previous.inverse.steps.drain(..));
            previous.inverse.steps = combined_steps;
            if inverse.selection_before.is_some() {
                previous.inverse.selection_before = inverse.selection_before;
            }
            previous.timestamp = timestamp;
        } else {
            self.undo_stack.push(HistoryEntry {
                inverse,
                group,
                timestamp,
            });
        }
        self.coalescing_broken = false;
    }

    pub fn undo(&mut self, doc: &mut Document) -> Option<(ChangeSet, Option<DocumentSelection>)> {
        let entry = self.undo_stack.pop()?;
        self.coalescing_broken = true;
        let target_sel = entry.inverse.selection_after.clone();
        let applied = doc.apply(entry.inverse);
        self.redo_stack.push(applied.inverse);
        Some((applied.change_set, target_sel))
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<(ChangeSet, Option<DocumentSelection>)> {
        let tx = self.redo_stack.pop()?;
        self.coalescing_broken = true;
        let target_sel = tx.selection_after.clone();
        let applied = doc.apply(tx);
        self.undo_stack.push(HistoryEntry {
            inverse: applied.inverse,
            group: HistoryMergeGroup::Atomic,
            timestamp: Instant::now(),
        });
        Some((applied.change_set, target_sel))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }
}
