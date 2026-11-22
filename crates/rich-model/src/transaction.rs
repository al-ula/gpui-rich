use crate::block::{BlockId, BlockNode, Document};
use crate::inline::{InlineContent, StyleSpan};
use crate::selection::DocumentSelection;
use crate::style::{BlockStyle, InlineStyle};
use smallvec::SmallVec;
use std::ops::Range;

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
    InsertBlock {
        index: usize,
        block: BlockNode,
    },
    RemoveBlock {
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
                    if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        inline.insert_text(offset, &text, style);
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::DeleteRange {
                            block_id,
                            range: offset..(offset + inserted_len),
                        });
                    }
                }
                Step::InsertContent {
                    block_id,
                    offset,
                    content,
                } => {
                    let inserted_len = content.len();
                    if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        inline.insert_content(offset, &content);
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::DeleteRange {
                            block_id,
                            range: offset..(offset + inserted_len),
                        });
                    }
                }
                Step::DeleteRange { block_id, range } => {
                    if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let removed = inline.delete_range(range.clone());
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::InsertContent {
                            block_id,
                            offset: range.start,
                            content: removed,
                        });
                    }
                }
                Step::ApplyInlineStyle {
                    block_id,
                    range,
                    style,
                } => {
                    if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let previous = inline.apply_style(range, &style);
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::RestoreSpans {
                            block_id,
                            spans: previous,
                        });
                    }
                }
                Step::RestoreSpans { block_id, spans } => {
                    if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let prev = if let (Some(first), Some(last)) = (spans.first(), spans.last())
                        {
                            inline.apply_style(
                                first.range.start..last.range.end,
                                &InlineStyle::default(),
                            )
                        } else {
                            Vec::new()
                        };
                        inline.restore_spans(&spans);
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::RestoreSpans {
                            block_id,
                            spans: prev,
                        });
                    }
                }
                Step::SetBlockStyle { block_id, style } => {
                    if let Some(block) = self.find_block_mut(block_id) {
                        let old_style = std::mem::replace(&mut block.style, style);
                        block.bump_version();
                        change_set.mark_dirty(block_id);
                        inverse_steps.push(Step::SetBlockStyle {
                            block_id,
                            style: old_style,
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
}

#[derive(Default)]
pub struct History {
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,
}

impl History {
    pub fn push_inverse(&mut self, inverse: Transaction) {
        self.undo_stack.push(inverse);
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> Option<(ChangeSet, Option<DocumentSelection>)> {
        let tx = self.undo_stack.pop()?;
        let target_sel = tx.selection_after.clone();
        let applied = doc.apply(tx);
        self.redo_stack.push(applied.inverse);
        Some((applied.change_set, target_sel))
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<(ChangeSet, Option<DocumentSelection>)> {
        let tx = self.redo_stack.pop()?;
        let target_sel = tx.selection_after.clone();
        let applied = doc.apply(tx);
        self.undo_stack.push(applied.inverse);
        Some((applied.change_set, target_sel))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}
