use crate::block::{
    BlockId, BlockKind, BlockNode, ColumnSpec, ContainerTarget, Document, TableCell, TableRow,
};
use crate::inline::{InlineContent, InlineObject, InlineObjectId, StyleSpan};
use crate::selection::{Affinity, DocPos, DocumentSelection, PathSegment};
use crate::style::{BlockStyle, InlineStyle, NamedStyle, StyleId};
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
    ToggleChecklist {
        block_id: BlockId,
    },
    UpdateInlineObject {
        block_id: BlockId,
        object_id: InlineObjectId,
        object: InlineObject,
    },
    UpdateNamedStyle {
        style_id: StyleId,
        style: Option<NamedStyle>,
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

/// Outcome of [`Document::next_table_cell_pos`] when stepping between table cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableCellStep {
    /// Move the caret to the adjacent cell position.
    MoveTo(DocPos),
    /// Reached the start or end boundary of the table.
    AtBoundary,
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
                Step::ToggleChecklist { block_id } => {
                    if self.toggle_checklist_item(block_id).is_some() {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::ToggleChecklist { block_id });
                    }
                }
                Step::UpdateInlineObject {
                    block_id,
                    object_id,
                    object,
                } => {
                    let previous = self
                        .find_block_mut(block_id)
                        .and_then(|block| {
                            let prev = block.kind.inline_content_mut()?.update_object(object_id, object)?;
                            block.bump_version();
                            Some(prev)
                        });
                    if let Some(previous_object) = previous {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::UpdateInlineObject {
                            block_id,
                            object_id,
                            object: previous_object,
                        });
                    }
                }
                Step::UpdateNamedStyle { style_id, style } => {
                    let previous = match style {
                        Some(next_style) => self.stylesheet.styles.insert(style_id, next_style),
                        None => self.stylesheet.styles.remove(&style_id),
                    };
                    let mut affected = Vec::new();
                    bump_blocks_using_named_style(&mut self.blocks, style_id, &mut affected);
                    for affected_id in affected {
                        change_set.mark_dirty(affected_id);
                        self.bump_ancestors_of_block(affected_id, &mut change_set);
                    }
                    change_set.structure_changed = true;
                    inverse_steps.push(Step::UpdateNamedStyle {
                        style_id,
                        style: previous,
                    });
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
        if block.kind.code_block_data().is_some() {
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
        if let Some(item) = block.kind.list_item_data()
            && inline.is_empty()
        {
            let (new_kind, new_style) = if item.indent_level > 0 {
                let next_level = item.indent_level - 1;
                let mut next_style = block.style.clone();
                next_style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
                (
                    BlockKind::list_item(
                        item.list_id,
                        item.kind,
                        next_level,
                        item.checked,
                        InlineContent::empty(),
                    ),
                    next_style,
                )
            } else {
                (
                    BlockKind::paragraph(InlineContent::empty()),
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
        let (trailing_kind, trailing_style) = if block.kind.is_paragraph() {
            (BlockKind::paragraph(trailing), block.style.clone())
        } else if let Some(heading) = block.kind.heading_data() {
            if offset == inline.len() {
                (
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                )
            } else {
                (
                    BlockKind::heading(heading.level, false, trailing),
                    block.style.clone(),
                )
            }
        } else if let Some(item) = block.kind.list_item_data() {
            (
                BlockKind::list_item(
                    item.list_id,
                    item.kind,
                    item.indent_level,
                    item.checked.map(|_| false),
                    trailing,
                ),
                block.style.clone(),
            )
        } else if let BlockKind::Text { kind, data, .. } = &block.kind {
            if offset == inline.len() {
                (
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                )
            } else {
                (
                    BlockKind::text(kind.clone(), data.clone(), trailing),
                    block.style.clone(),
                )
            }
        } else {
            return None;
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
        if let Some(item) = block.kind.list_item_data()
            && let Some(content) = block.kind.inline_content()
        {
            let (new_kind, new_style) = if item.indent_level > 0 {
                let next_level = item.indent_level - 1;
                let mut next_style = block.style.clone();
                next_style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
                (
                    BlockKind::list_item(
                        item.list_id,
                        item.kind,
                        next_level,
                        item.checked,
                        content.clone(),
                    ),
                    next_style,
                )
            } else {
                (
                    BlockKind::paragraph(content.clone()),
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
        if block.kind.heading_data().is_some()
            && let Some(content) = block.kind.inline_content()
        {
            return self.build_convert_block_transaction(
                leaf_id,
                BlockKind::paragraph(content.clone()),
                BlockStyle::default(),
                selection_before,
                Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
            );
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

    /// Builds a transaction updating column width specs for `table_id`.
    pub fn build_set_table_columns_transaction(
        &self,
        table_id: BlockId,
        columns: Vec<ColumnSpec>,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid { kind, data, table } = &self.find_block(table_id)?.kind else {
            return None;
        };
        let mut next_table = table.clone();
        next_table.columns = columns;
        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(kind.clone(), data.clone(), next_table),
            }])
            .with_selections(selection_before.clone(), selection_before),
        )
    }

    /// Builds a span-aware transaction inserting a new row above or below `row_index`.
    pub fn build_insert_table_row_transaction(
        &mut self,
        table_id: BlockId,
        row_index: usize,
        after: bool,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        if table.rows.is_empty() {
            return None;
        }
        let clamped_row = row_index.min(table.rows.len() - 1);
        let insert_at = if after { clamped_row + 1 } else { clamped_row };
        let (col_count, placements, _) = table.resolve_grid();

        let mut next_table = table;
        let mut covered_columns = vec![false; col_count];
        for placement in &placements {
            if placement.row_index < insert_at
                && placement.row_index + placement.rowspan > insert_at
            {
                if let Some(cell) = next_table
                    .rows
                    .get_mut(placement.row_index)
                    .and_then(|row| row.cells.get_mut(placement.cell_index))
                {
                    cell.rowspan = cell.rowspan.saturating_add(1);
                }
                for col in placement.visual_col..(placement.visual_col + placement.colspan) {
                    if let Some(slot) = covered_columns.get_mut(col) {
                        *slot = true;
                    }
                }
            }
        }

        let mut new_cells = Vec::new();
        let mut first_new_para_id = None;
        for is_covered in covered_columns {
            if is_covered {
                continue;
            }
            let cell_id = self.alloc_block_id();
            let para_id = self.alloc_block_id();
            if first_new_para_id.is_none() {
                first_new_para_id = Some(para_id);
            }
            new_cells.push(TableCell::simple(
                cell_id,
                vec![BlockNode::new(
                    para_id,
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                )],
            ));
        }
        next_table.rows.insert(
            insert_at,
            TableRow {
                is_header: false,
                cells: new_cells,
            },
        );

        let selection_after = first_new_para_id
            .and_then(|para_id| {
                self.table_cell_doc_pos(table_id, insert_at, 0, para_id, 0)
            })
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Builds a span-aware transaction deleting rows in `row_range` from `table_id`.
    /// When all rows are deleted, converts the table block into an empty paragraph.
    pub fn build_delete_table_row_transaction(
        &mut self,
        table_id: BlockId,
        row_range: Range<usize>,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let row_count = table.rows.len();
        if row_count == 0 {
            return None;
        }
        let start_row = row_range.start.min(row_count);
        let end_row = row_range.end.min(row_count).max(start_row);
        if start_row >= end_row {
            return None;
        }
        if start_row == 0 && end_row >= row_count {
            let next_pos = self.doc_pos_for_block(table_id, 0);
            return Some(
                Transaction::new(vec![Step::SetBlockKind {
                    block_id: table_id,
                    kind: BlockKind::paragraph(InlineContent::empty()),
                }])
                .with_selections(
                    selection_before,
                    next_pos.map(|pos| DocumentSelection::caret(pos, Affinity::Downstream)),
                ),
            );
        }

        let (_, placements, _) = table.resolve_grid();
        let mut next_table = table;
        // Collect cells starting in a deleted row that continue below `end_row` so they migrate downward.
        let mut migrated: Vec<(usize, TableCell)> = Vec::new();
        for placement in &placements {
            let span_end = placement.row_index + placement.rowspan;
            if placement.row_index < start_row && span_end > start_row {
                let removed_overlap = span_end.min(end_row) - start_row;
                if let Some(cell) = next_table
                    .rows
                    .get_mut(placement.row_index)
                    .and_then(|row| row.cells.get_mut(placement.cell_index))
                {
                    cell.rowspan = cell.rowspan.saturating_sub(removed_overlap as u16).max(1);
                }
            } else if placement.row_index >= start_row
                && placement.row_index < end_row
                && span_end > end_row
                && let Some(cell) = next_table
                    .rows
                    .get(placement.row_index)
                    .and_then(|row| row.cells.get(placement.cell_index))
            {
                let mut cloned = cell.clone();
                cloned.rowspan = (span_end - end_row).max(1) as u16;
                migrated.push((placement.visual_col, cloned));
            }
        }

        if !migrated.is_empty() && end_row < next_table.rows.len() {
            // Reconstruct `end_row` preserving visual column order with migrated spanning cells.
            let mut surviving_in_end: Vec<(usize, TableCell)> = placements
                .iter()
                .filter(|placement| placement.row_index == end_row)
                .filter_map(|placement| {
                    next_table
                        .rows
                        .get(end_row)
                        .and_then(|row| row.cells.get(placement.cell_index))
                        .cloned()
                        .map(|cell| (placement.visual_col, cell))
                })
                .collect();
            surviving_in_end.extend(migrated);
            surviving_in_end.sort_by_key(|(visual_col, _)| *visual_col);
            if let Some(target_row) = next_table.rows.get_mut(end_row) {
                target_row.cells = surviving_in_end.into_iter().map(|(_, cell)| cell).collect();
            }
        }

        next_table.rows.drain(start_row..end_row);
        let focus_row = start_row.min(next_table.rows.len().saturating_sub(1));
        let focus_para_id = next_table
            .rows
            .get(focus_row)
            .and_then(|row| row.cells.first())
            .and_then(|cell| cell.blocks.first())
            .map(|block| block.id);
        let selection_after = focus_para_id
            .and_then(|para_id| self.table_cell_doc_pos(table_id, focus_row, 0, para_id, 0))
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Builds a span-aware transaction inserting a new visual column before or after `visual_col`.
    pub fn build_insert_table_column_transaction(
        &mut self,
        table_id: BlockId,
        visual_col: usize,
        after: bool,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let (col_count, placements, _) = table.resolve_grid();
        let clamped_col = visual_col.min(col_count.saturating_sub(1));
        let insert_at = if after { clamped_col + 1 } else { clamped_col };

        let mut next_table = table;
        while next_table.columns.len() < col_count {
            next_table.columns.push(ColumnSpec::Auto);
        }
        next_table
            .columns
            .insert(insert_at.min(next_table.columns.len()), ColumnSpec::Auto);

        let row_count = next_table.rows.len();
        let mut row_spanning_insert = vec![false; row_count];
        for placement in &placements {
            if placement.visual_col < insert_at
                && placement.visual_col + placement.colspan > insert_at
            {
                if let Some(cell) = next_table
                    .rows
                    .get_mut(placement.row_index)
                    .and_then(|row| row.cells.get_mut(placement.cell_index))
                {
                    cell.colspan = cell.colspan.saturating_add(1);
                }
                for row_idx in placement.row_index..(placement.row_index + placement.rowspan) {
                    if let Some(flag) = row_spanning_insert.get_mut(row_idx) {
                        *flag = true;
                    }
                }
            }
        }

        let mut focus_target: Option<(usize, usize, BlockId)> = None;
        for (row_idx, &is_spanned) in row_spanning_insert.iter().enumerate().take(row_count) {
            if is_spanned {
                continue;
            }
            let insert_cell_index = placements
                .iter()
                .filter(|placement| {
                    placement.row_index == row_idx && placement.visual_col < insert_at
                })
                .count();
            let cell_id = self.alloc_block_id();
            let para_id = self.alloc_block_id();
            if focus_target.is_none() {
                focus_target = Some((row_idx, insert_cell_index, para_id));
            }
            if let Some(row) = next_table.rows.get_mut(row_idx) {
                let clamped_idx = insert_cell_index.min(row.cells.len());
                row.cells.insert(
                    clamped_idx,
                    TableCell::simple(
                        cell_id,
                        vec![BlockNode::new(
                            para_id,
                            BlockKind::paragraph(InlineContent::empty()),
                            BlockStyle::default(),
                        )],
                    ),
                );
            }
        }

        let selection_after = focus_target
            .and_then(|(row_idx, cell_idx, para_id)| {
                self.table_cell_doc_pos(table_id, row_idx, cell_idx, para_id, 0)
            })
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Builds a span-aware transaction deleting visual columns in `col_range` from `table_id`.
    /// When all columns are deleted, converts the table block into an empty paragraph.
    pub fn build_delete_table_column_transaction(
        &mut self,
        table_id: BlockId,
        col_range: Range<usize>,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let (col_count, placements, _) = table.resolve_grid();
        let start_col = col_range.start.min(col_count);
        let end_col = col_range.end.min(col_count).max(start_col);
        if start_col >= end_col {
            return None;
        }
        if start_col == 0 && end_col >= col_count {
            let next_pos = self.doc_pos_for_block(table_id, 0);
            return Some(
                Transaction::new(vec![Step::SetBlockKind {
                    block_id: table_id,
                    kind: BlockKind::paragraph(InlineContent::empty()),
                }])
                .with_selections(
                    selection_before,
                    next_pos.map(|pos| DocumentSelection::caret(pos, Affinity::Downstream)),
                ),
            );
        }

        let mut next_table = table;
        while next_table.columns.len() < col_count {
            next_table.columns.push(ColumnSpec::Auto);
        }
        next_table.columns.drain(start_col..end_col);

        for row_idx in 0..next_table.rows.len() {
            let row_placements: Vec<_> = placements
                .iter()
                .filter(|placement| placement.row_index == row_idx)
                .collect();
            let old_cells = std::mem::take(&mut next_table.rows[row_idx].cells);
            let mut new_cells = Vec::with_capacity(old_cells.len());
            for (placement, mut cell) in row_placements.into_iter().zip(old_cells) {
                let cell_start = placement.visual_col;
                let cell_end = placement.visual_col + placement.colspan;
                let overlap_start = cell_start.max(start_col);
                let overlap_end = cell_end.min(end_col);
                if overlap_start < overlap_end {
                    let removed = overlap_end - overlap_start;
                    if removed >= placement.colspan {
                        continue;
                    }
                    cell.colspan = (placement.colspan - removed).max(1) as u16;
                }
                new_cells.push(cell);
            }
            next_table.rows[row_idx].cells = new_cells;
        }

        let focus_target = next_table.rows.iter().enumerate().find_map(|(r, row)| {
            let cell = row.cells.first()?;
            let para = cell.blocks.first()?;
            Some((r, 0usize, para.id))
        });
        let selection_after = focus_target
            .and_then(|(row_idx, cell_idx, para_id)| {
                self.table_cell_doc_pos(table_id, row_idx, cell_idx, para_id, 0)
            })
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Builds a transaction merging all cells covering the rectangular visual range
    /// defined by `anchor_visual` `(row, col)` and `focus_visual` `(row, col)`.
    pub fn build_merge_table_cells_transaction(
        &mut self,
        table_id: BlockId,
        anchor_visual: (usize, usize),
        focus_visual: (usize, usize),
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let row_count = table.rows.len();
        let (col_count, placements, occupancy) = table.resolve_grid();
        if row_count == 0 || col_count == 0 {
            return None;
        }

        let mut r_min = anchor_visual.0.min(focus_visual.0).min(row_count - 1);
        let mut r_max = anchor_visual.0.max(focus_visual.0).min(row_count - 1);
        let mut c_min = anchor_visual.1.min(focus_visual.1).min(col_count - 1);
        let mut c_max = anchor_visual.1.max(focus_visual.1).min(col_count - 1);

        // Expand selection rectangle until it fully encloses every intersecting spanned cell.
        loop {
            let mut next_r_min = r_min;
            let mut next_r_max = r_max;
            let mut next_c_min = c_min;
            let mut next_c_max = c_max;
            for row_slots in occupancy.iter().take(r_max + 1).skip(r_min) {
                for slot in row_slots.iter().take(c_max + 1).skip(c_min) {
                    if let Some(idx) = *slot {
                        let placement = placements[idx];
                        let p_r_end = (placement.row_index + placement.rowspan).saturating_sub(1);
                        let p_c_end = (placement.visual_col + placement.colspan).saturating_sub(1);
                        next_r_min = next_r_min.min(placement.row_index);
                        next_r_max = next_r_max.max(p_r_end);
                        next_c_min = next_c_min.min(placement.visual_col);
                        next_c_max = next_c_max.max(p_c_end);
                    }
                }
            }
            if (next_r_min, next_r_max, next_c_min, next_c_max) == (r_min, r_max, c_min, c_max) {
                break;
            }
            r_min = next_r_min;
            r_max = next_r_max;
            c_min = next_c_min;
            c_max = next_c_max;
        }

        let mut covered_indices: Vec<usize> = Vec::new();
        for row_slots in occupancy.iter().take(r_max + 1).skip(r_min) {
            for slot in row_slots.iter().take(c_max + 1).skip(c_min) {
                if let Some(idx) = *slot
                    && !covered_indices.contains(&idx)
                {
                    covered_indices.push(idx);
                }
            }
        }
        if covered_indices.len() <= 1 {
            return None;
        }
        covered_indices.sort_by_key(|&idx| (placements[idx].row_index, placements[idx].visual_col));

        let anchor_placement = placements[covered_indices[0]];
        let mut next_table = table;
        let mut absorbed_blocks = Vec::new();
        for &idx in covered_indices.iter().skip(1) {
            let placement = placements[idx];
            if let Some(cell) = next_table
                .rows
                .get(placement.row_index)
                .and_then(|row| row.cells.get(placement.cell_index))
            {
                for block in &cell.blocks {
                    let is_empty_para = block.kind.is_paragraph()
                        && block
                            .kind
                            .inline_content()
                            .is_some_and(InlineContent::is_empty);
                    if !is_empty_para {
                        absorbed_blocks.push(block.clone());
                    }
                }
            }
        }

        let anchor_para_id = if let Some(anchor_cell) = next_table
            .rows
            .get_mut(anchor_placement.row_index)
            .and_then(|row| row.cells.get_mut(anchor_placement.cell_index))
        {
            anchor_cell.colspan = (c_max - c_min + 1) as u16;
            anchor_cell.rowspan = (r_max - r_min + 1) as u16;
            if !absorbed_blocks.is_empty() {
                if anchor_cell.blocks.len() == 1
                    && anchor_cell.blocks[0].kind.is_paragraph()
                    && anchor_cell.blocks[0]
                        .kind
                        .inline_content()
                        .is_some_and(InlineContent::is_empty)
                {
                    anchor_cell.blocks = absorbed_blocks;
                } else {
                    anchor_cell.blocks.extend(absorbed_blocks);
                }
            }
            anchor_cell.blocks.first().map(|block| block.id)
        } else {
            None
        };

        // Remove absorbed secondary cells in reverse `(row_index, cell_index)` order.
        let mut removals: Vec<(usize, usize)> = covered_indices
            .iter()
            .skip(1)
            .map(|&idx| (placements[idx].row_index, placements[idx].cell_index))
            .collect();
        removals.sort_unstable();
        for (row_idx, cell_idx) in removals.into_iter().rev() {
            if let Some(row) = next_table.rows.get_mut(row_idx)
                && cell_idx < row.cells.len()
            {
                row.cells.remove(cell_idx);
            }
        }

        let selection_after = anchor_para_id
            .and_then(|para_id| {
                self.table_cell_doc_pos(
                    table_id,
                    anchor_placement.row_index,
                    anchor_placement.cell_index,
                    para_id,
                    0,
                )
            })
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Builds a transaction splitting a merged cell (`colspan > 1` or `rowspan > 1`) at
    /// stored `(row_index, cell_index)` back into 1×1 cells.
    pub fn build_split_table_cell_transaction(
        &mut self,
        table_id: BlockId,
        row_index: usize,
        cell_index: usize,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let BlockKind::Grid {
            kind: grid_kind,
            data: grid_data,
            table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let (_, placements, _) = table.resolve_grid();
        let target = placements
            .iter()
            .copied()
            .find(|placement| placement.row_index == row_index && placement.cell_index == cell_index)?;
        if target.colspan <= 1 && target.rowspan <= 1 {
            return None;
        }

        let mut next_table = table;
        let anchor_para_id = next_table
            .rows
            .get_mut(target.row_index)
            .and_then(|row| row.cells.get_mut(target.cell_index))
            .and_then(|cell| {
                cell.colspan = 1;
                cell.rowspan = 1;
                cell.blocks.first().map(|block| block.id)
            });

        for r in target.row_index..(target.row_index + target.rowspan) {
            let start_c = if r == target.row_index {
                target.visual_col + 1
            } else {
                target.visual_col
            };
            let end_c = target.visual_col + target.colspan;
            if start_c >= end_c {
                continue;
            }
            let mut insert_idx = placements
                .iter()
                .filter(|placement| placement.row_index == r && placement.visual_col < start_c)
                .count();
            for _ in start_c..end_c {
                let cell_id = self.alloc_block_id();
                let para_id = self.alloc_block_id();
                if let Some(row) = next_table.rows.get_mut(r) {
                    let clamped = insert_idx.min(row.cells.len());
                    row.cells.insert(
                        clamped,
                        TableCell::simple(
                            cell_id,
                            vec![BlockNode::new(
                                para_id,
                                BlockKind::paragraph(InlineContent::empty()),
                                BlockStyle::default(),
                            )],
                        ),
                    );
                    insert_idx += 1;
                }
            }
        }

        let selection_after = anchor_para_id
            .and_then(|para_id| {
                self.table_cell_doc_pos(table_id, target.row_index, target.cell_index, para_id, 0)
            })
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::grid(grid_kind, grid_data, next_table),
            }])
            .with_selections(selection_before, selection_after),
        )
    }

    /// Result of stepping horizontally across cells in a table with Tab / Shift+Tab.
    /// Returns `Some(TableCellStep::MoveTo(pos))` for an adjacent cell, `Some(TableCellStep::AtBoundary)`
    /// when stepping past the first or final cell, or `None` when not inside a valid table cell.
    pub fn next_table_cell_pos(
        &self,
        table_id: BlockId,
        current_row: usize,
        current_cell_index: usize,
        forward: bool,
    ) -> Option<TableCellStep> {
        let table = self.find_block(table_id)?.kind.table_block()?;
        let mut cell_order: Vec<(usize, usize, BlockId)> = Vec::new();
        for (row_idx, row) in table.rows.iter().enumerate() {
            for (cell_idx, cell) in row.cells.iter().enumerate() {
                if let Some(first_block) = cell.blocks.first() {
                    cell_order.push((row_idx, cell_idx, first_block.id));
                }
            }
        }
        let current_pos = cell_order
            .iter()
            .position(|&(r, c, _)| r == current_row && c == current_cell_index)?;

        let target_index = if forward {
            if current_pos + 1 >= cell_order.len() {
                return Some(TableCellStep::AtBoundary);
            }
            current_pos + 1
        } else {
            let Some(prev) = current_pos.checked_sub(1) else {
                return Some(TableCellStep::AtBoundary);
            };
            prev
        };

        let (_, _, block_id) = cell_order[target_index];
        self.doc_pos_for_block(block_id, 0)
            .map(TableCellStep::MoveTo)
    }

    fn table_cell_doc_pos(
        &self,
        table_id: BlockId,
        row_index: usize,
        cell_index: usize,
        para_id: BlockId,
        offset: usize,
    ) -> Option<DocPos> {
        let mut path = self.path_for_block(table_id)?;
        path.pop();
        path.push(PathSegment::TableCell {
            table_id,
            row: row_index,
            col: cell_index,
        });
        path.push(PathSegment::Block(para_id));
        Some(DocPos { path, offset })
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
            ContainerTarget::Container(container_id) => Some(container_id),
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

fn bump_blocks_using_named_style(
    blocks: &mut [BlockNode],
    style_id: StyleId,
    affected: &mut Vec<BlockId>,
) {
    for block in blocks {
        if block.style.named_style == Some(style_id) {
            block.bump_version();
            affected.push(block.id);
        }
        match &mut block.kind {
            BlockKind::Container { children, .. } => {
                bump_blocks_using_named_style(children, style_id, affected);
            }
            BlockKind::Grid { table, .. } => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        bump_blocks_using_named_style(&mut cell.blocks, style_id, affected);
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
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
