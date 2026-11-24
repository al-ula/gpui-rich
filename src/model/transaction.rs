use crate::model::block::{
    BlockId, BlockKind, BlockNode, BlockShape, BlockShapeKind, BlockSpec, CodeBlockData,
    ColumnSpec, ContainerTarget, Document, TableBlock, TableBorderStyle, TableCell, TableRow,
    block_kinds,
};
use crate::model::inline::{InlineContent, InlineObject, InlineObjectId, InlineSpec, StyleSpan};
use crate::model::payload::CustomPayload;
use crate::model::selection::{Affinity, DocPos, DocumentSelection, PathSegment};
use crate::model::style::{BlockStyle, InlineStyle, NamedStyle, SharedStr, StyleId};
use smallvec::SmallVec;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
/// Monotonic timestamp type used for [`History`] coalescing windows.
pub type HistoryInstant = std::time::Instant;
#[cfg(target_family = "wasm")]
/// Monotonic timestamp type used for [`History`] coalescing windows on WebAssembly.
pub type HistoryInstant = web_time::Instant;

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
    RestoreInlineContent {
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
    /// Replaces only the custom payload on `block_id` when its semantic kind and shape match.
    ///
    /// Preserves the block ID, style, width mode, and any inline content, children, or grid table.
    /// Does not mark the change set as structural.
    UpdateBlockData {
        block_id: BlockId,
        expected_kind: SharedStr,
        expected_shape: BlockShapeKind,
        data: CustomPayload,
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
    MoveBlockIn {
        target: ContainerTarget,
        from: usize,
        to: usize,
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
    SetAnchors {
        anchors: crate::model::anchors::AnchorStore,
    },
    MapSplitAnchors {
        block_id: BlockId,
        offset: usize,
        new_block_id: BlockId,
    },
    MapMergeAnchors {
        block_id: BlockId,
        offset: usize,
        source_block_id: BlockId,
        source_offset: usize,
    },
    SetComponents {
        components: crate::model::components::ComponentStore,
    },
    SetListNumbering {
        block_id: BlockId,
        numbering: Option<crate::model::edit::structure::ListNumbering>,
    },
}

impl Step {
    /// Builds a reversible, payload-only block update checked against [`BlockSpec`] `S`.
    pub fn update_block_data<S: BlockSpec>(block_id: BlockId, data: S::Data) -> Self {
        Self::UpdateBlockData {
            block_id,
            expected_kind: Arc::from(S::KIND),
            expected_shape: S::Shape::KIND,
            data: CustomPayload::typed(data),
        }
    }

    /// Builds a reversible, checked inline object payload update using [`Step::UpdateInlineObject`].
    pub fn update_inline_data<S: InlineSpec>(
        block_id: BlockId,
        object_id: InlineObjectId,
        data: S::Data,
    ) -> Self {
        Self::UpdateInlineObject {
            block_id,
            object_id,
            object: InlineObject::custom::<S>(data),
        }
    }
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

/// Result of a collapsed boundary deletion or block-exit request.
///
/// Distinguishes document mutations from non-destructive caret moves (for example pressing
/// Backspace immediately after a table or non-empty code block enters it without modifying text).
#[derive(Clone, Debug, PartialEq)]
pub enum BoundaryDeleteOutcome {
    Edit(Box<Transaction>),
    MoveSelection(Box<DocumentSelection>),
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

        let has_bookmarks = self.anchors.bookmarks().next().is_some();
        let original_block_order = (has_bookmarks
            && tx.steps.iter().any(|step| {
                matches!(
                    step,
                    Step::RemoveBlock { .. }
                        | Step::RemoveBlockIn { .. }
                        | Step::SetBlockKind { .. }
                )
            }))
        .then(|| {
            self.collect_leaf_blocks()
                .into_iter()
                .map(|block| block.id)
                .collect::<Vec<_>>()
        });
        let anchors_before = (has_bookmarks
            || tx
                .steps
                .iter()
                .any(|step| matches!(step, Step::SetAnchors { .. })))
        .then(|| self.anchors.clone());

        for step in tx.steps {
            let restore_inline_identity = matches!(&step, Step::RestoreInlineContent { .. });
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
                        self.anchors.map_insert(block_id, clamped, inserted_len);
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
                }
                | Step::RestoreInlineContent {
                    block_id,
                    offset,
                    content,
                } => {
                    let inserted_len = content.len();
                    let applied = if let Some(block) = self.find_block_mut(block_id)
                        && let Some(inline) = block.kind.inline_content_mut()
                    {
                        let clamped = offset.min(inline.len());
                        if restore_inline_identity {
                            inline.insert_content_preserving_object_ids(clamped, &content);
                        } else {
                            inline.insert_content(clamped, &content);
                        }
                        block.bump_version();
                        Some(clamped)
                    } else {
                        None
                    };
                    if let Some(clamped) = applied {
                        self.anchors.map_insert(block_id, clamped, inserted_len);
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
                        self.anchors
                            .map_delete(block_id, start, start + slice.len());
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::RestoreInlineContent {
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
                Step::UpdateBlockData {
                    block_id,
                    expected_kind,
                    expected_shape,
                    data,
                } => {
                    let previous_data = self.find_block_mut(block_id).and_then(|block| {
                        if block.kind.semantic_kind() != expected_kind.as_ref()
                            || block.kind.shape_kind() != expected_shape
                        {
                            return None;
                        }
                        let previous = std::mem::replace(block.kind.data_mut(), data);
                        block.bump_version();
                        Some(previous)
                    });
                    if let Some(previous_data) = previous_data {
                        change_set.mark_dirty(block_id);
                        self.bump_ancestors_of_block(block_id, &mut change_set);
                        inverse_steps.push(Step::UpdateBlockData {
                            block_id,
                            expected_kind,
                            expected_shape,
                            data: previous_data,
                        });
                    }
                }
                Step::InsertBlock { index, block } => {
                    let end = if crate::model::anchors::is_footnote_body(&block) {
                        self.blocks.len()
                    } else {
                        self.content_end_index()
                    };
                    let clamped = index.min(end);
                    let id = block.id;
                    self.advance_next_block_id_for_node(&block);
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
                    let index = if target == ContainerTarget::Root
                        && !crate::model::anchors::is_footnote_body(&block)
                    {
                        index.min(self.content_end_index())
                    } else {
                        index
                    };
                    let id = block.id;
                    self.advance_next_block_id_for_node(&block);
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
                Step::MoveBlockIn { target, from, to } => {
                    if target == ContainerTarget::Root {
                        let end = self.content_end_index();
                        if (from < end) != (to < end) {
                            continue;
                        }
                    }
                    if let Some(blocks) = self.container_blocks_mut(target)
                        && from != to
                        && from < blocks.len()
                        && to < blocks.len()
                    {
                        let block = blocks.remove(from);
                        let id = block.id;
                        blocks.insert(to, block);
                        change_set.mark_dirty(id);
                        change_set.structure_changed = true;
                        self.bump_ancestors_of_target(target, &mut change_set);
                        inverse_steps.push(Step::MoveBlockIn {
                            target,
                            from: to,
                            to: from,
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
                    let previous = self.find_block_mut(block_id).and_then(|block| {
                        let prev = block
                            .kind
                            .inline_content_mut()?
                            .update_object_checked(object_id, object)?;
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
                Step::SetAnchors { anchors } => {
                    if self.anchors != anchors {
                        self.anchors = anchors;
                        change_set.structure_changed = true;
                    }
                }
                Step::MapSplitAnchors {
                    block_id,
                    offset,
                    new_block_id,
                } => {
                    self.anchors.map_split(block_id, offset, new_block_id);
                }
                Step::MapMergeAnchors {
                    block_id,
                    offset,
                    source_block_id,
                    source_offset,
                } => {
                    self.anchors
                        .map_merge_suffix(block_id, offset, source_block_id, source_offset);
                }
                Step::SetComponents { components } => {
                    if self.components != components {
                        let previous = std::mem::replace(&mut self.components, components);
                        let affected: Vec<_> = self
                            .collect_leaf_blocks()
                            .into_iter()
                            .filter(|block| {
                                block.kind.inline_content().is_some_and(|inline| {
                                    inline.objects.iter().any(|entry| {
                                        let InlineObject::Custom { kind, data } = &entry.object
                                        else {
                                            return false;
                                        };
                                        let Ok(payload) = data.as_json() else {
                                            return false;
                                        };
                                        match kind.as_ref() {
                                            crate::model::components::DROPDOWN_KIND => payload
                                                .get("definition_id")
                                                .and_then(serde_json::Value::as_str)
                                                .is_some_and(|id| {
                                                    previous.dropdowns.get(id)
                                                        != self.components.dropdowns.get(id)
                                                }),
                                            crate::model::components::VARIABLE_KIND => payload
                                                .get("variable_id")
                                                .and_then(serde_json::Value::as_str)
                                                .is_some_and(|id| {
                                                    previous.variables.get(id)
                                                        != self.components.variables.get(id)
                                                }),
                                            _ => false,
                                        }
                                    })
                                })
                            })
                            .map(|block| block.id)
                            .collect();
                        for block_id in affected {
                            if let Some(block) = self.find_block_mut(block_id) {
                                block.bump_version();
                            }
                            change_set.mark_dirty(block_id);
                            self.bump_ancestors_of_block(block_id, &mut change_set);
                        }
                        change_set.structure_changed = true;
                        inverse_steps.push(Step::SetComponents {
                            components: previous,
                        });
                    }
                }
                Step::SetListNumbering {
                    block_id,
                    numbering,
                } => {
                    let previous = self.ordered_lists.set(block_id, numbering);
                    if previous != numbering {
                        change_set.mark_dirty(block_id);
                        change_set.structure_changed = true;
                        inverse_steps.push(Step::SetListNumbering {
                            block_id,
                            numbering: previous,
                        });
                    }
                }
            }
        }

        if let Some(order) = original_block_order {
            self.repair_bookmarks_after_removal(&order);
        }
        inverse_steps.reverse();
        if let Some(anchors) = anchors_before
            && anchors != self.anchors
        {
            inverse_steps.push(Step::SetAnchors { anchors });
        }
        AppliedTransaction {
            change_set,
            inverse: Transaction {
                steps: inverse_steps,
                selection_before: tx.selection_after,
                selection_after: tx.selection_before,
            },
        }
    }

    fn repair_bookmarks_after_removal(&mut self, order: &[BlockId]) {
        use crate::model::anchors::AnchorPosition;

        let repairs: Vec<_> = self
            .anchors
            .bookmarks()
            .filter_map(|bookmark| {
                let position = bookmark.position;
                let text = |id| self.find_block(id)?.kind.inline_content();
                if let Some(inline) = text(position.block_id) {
                    let offset = inline
                        .text
                        .floor_char_boundary(position.offset.min(inline.len()));
                    return (offset != position.offset).then_some((
                        bookmark.id,
                        Some(AnchorPosition {
                            block_id: position.block_id,
                            offset,
                        }),
                    ));
                }
                let fallback = order
                    .iter()
                    .position(|id| *id == position.block_id)
                    .and_then(|index| {
                        order[index + 1..]
                            .iter()
                            .find_map(|&id| {
                                text(id).map(|_| AnchorPosition {
                                    block_id: id,
                                    offset: 0,
                                })
                            })
                            .or_else(|| {
                                order[..index].iter().rev().find_map(|&id| {
                                    text(id).map(|inline| AnchorPosition {
                                        block_id: id,
                                        offset: inline.len(),
                                    })
                                })
                            })
                    })
                    .or_else(|| {
                        self.collect_leaf_blocks()
                            .first()
                            .map(|leaf| AnchorPosition {
                                block_id: leaf.id,
                                offset: 0,
                            })
                    });
                Some((bookmark.id, fallback))
            })
            .collect();
        for (id, fallback) in repairs {
            if let Some(position) = fallback {
                self.anchors.set_bookmark_position(id, position);
            } else {
                self.anchors.remove_bookmark(id);
            }
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
            if self.anchors.bookmarks().next().is_some() {
                steps.push(Step::MapMergeAnchors {
                    block_id: start_id,
                    offset: start_offset,
                    source_block_id: end_id,
                    source_offset: end_offset,
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

        // Pressing Enter on an empty trailing child of a non-footnote container exits the container.
        if inline.is_empty()
            && let ContainerTarget::Container(container_id) = target
            && let Some(container_block) = self.find_block(container_id)
            && !crate::model::anchors::is_footnote_body(container_block)
            && let Some(siblings) = self.container_blocks(target)
            && block_index + 1 == siblings.len()
            && let Some((parent_target, container_index)) = self.locate_block(container_id)
        {
            let lifted_block = BlockNode::new(
                leaf_id,
                BlockKind::paragraph(InlineContent::empty()),
                BlockStyle::default(),
            );
            let mut steps = Vec::new();
            let insert_index = if siblings.len() == 1 {
                steps.push(Step::RemoveBlockIn {
                    target: parent_target,
                    index: container_index,
                });
                container_index
            } else {
                steps.push(Step::RemoveBlockIn {
                    target,
                    index: block_index,
                });
                container_index + 1
            };
            steps.push(Step::InsertBlockIn {
                target: parent_target,
                index: insert_index,
                block: lifted_block,
            });
            let next_pos = self.doc_pos_for_container_child(parent_target, leaf_id, 0)?;
            return Some(Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            ));
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
                    BlockKind::raw_text(kind.clone(), data.clone(), trailing),
                    block.style.clone(),
                )
            }
        } else {
            return None;
        };

        let mut steps = Vec::new();
        if self.anchors.bookmarks().next().is_some() {
            steps.push(Step::MapSplitAnchors {
                block_id: leaf_id,
                offset,
                new_block_id,
            });
        }
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

        Some(Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        ))
    }

    /// Plans Backspace at offset 0 of `pos`, distinguishing destructive edits from safe
    /// non-destructive caret entry into preceding tables, containers, or code blocks.
    pub fn plan_backward_boundary_delete(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<BoundaryDeleteOutcome> {
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
                (BlockKind::paragraph(content.clone()), BlockStyle::default())
            };
            return self
                .build_convert_block_transaction(
                    leaf_id,
                    new_kind,
                    new_style,
                    selection_before,
                    Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                )
                .map(|transaction| BoundaryDeleteOutcome::Edit(Box::new(transaction)));
        }
        if block.kind.heading_data().is_some()
            && let Some(content) = block.kind.inline_content()
        {
            return self
                .build_convert_block_transaction(
                    leaf_id,
                    BlockKind::paragraph(content.clone()),
                    BlockStyle::default(),
                    selection_before,
                    Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                )
                .map(|transaction| BoundaryDeleteOutcome::Edit(Box::new(transaction)));
        }
        if block.kind.code_block_data().is_some()
            && let Some(content) = block.kind.inline_content()
        {
            if content.is_empty() {
                return self
                    .build_convert_block_transaction(
                        leaf_id,
                        BlockKind::paragraph(InlineContent::empty()),
                        BlockStyle::default(),
                        selection_before,
                        Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                    )
                    .map(|transaction| BoundaryDeleteOutcome::Edit(Box::new(transaction)));
            }
            return None;
        }

        if block_index == 0 {
            if let ContainerTarget::Container(container_id) = target
                && let Some(container_block) = self.find_block(container_id)
                && !crate::model::anchors::is_footnote_body(container_block)
                && let Some(siblings) = self.container_blocks(target)
                && let Some((parent_target, container_index)) = self.locate_block(container_id)
            {
                let lifted_block = block.clone();
                let mut steps = Vec::new();
                if siblings.len() == 1 {
                    steps.push(Step::RemoveBlockIn {
                        target: parent_target,
                        index: container_index,
                    });
                } else {
                    steps.push(Step::RemoveBlockIn { target, index: 0 });
                }
                steps.push(Step::InsertBlockIn {
                    target: parent_target,
                    index: container_index,
                    block: lifted_block,
                });
                let next_pos = self.doc_pos_for_container_child(parent_target, leaf_id, 0)?;
                return Some(BoundaryDeleteOutcome::Edit(Box::new(
                    Transaction::new(steps).with_selections(
                        selection_before,
                        Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
                    ),
                )));
            }
            return None;
        }

        if target == ContainerTarget::Root && block_index >= self.content_end_index() {
            return None;
        }

        let siblings = self.container_blocks(target)?;
        let prev_block = siblings.get(block_index - 1)?;
        match &prev_block.kind {
            BlockKind::Leaf { .. } => {
                let steps = vec![Step::RemoveBlockIn {
                    target,
                    index: block_index - 1,
                }];
                return Some(BoundaryDeleteOutcome::Edit(Box::new(
                    Transaction::new(steps).with_selections(
                        selection_before,
                        Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                    ),
                )));
            }
            BlockKind::Grid { .. } => {
                let target_pos = self.last_text_pos_in_node(prev_block)?;
                return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                    DocumentSelection::caret(target_pos, Affinity::Downstream),
                )));
            }
            BlockKind::Container { .. } => {
                if let Some(target_pos) = self.last_text_pos_in_node(prev_block) {
                    return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                        DocumentSelection::caret(target_pos, Affinity::Downstream),
                    )));
                }
                let steps = vec![Step::RemoveBlockIn {
                    target,
                    index: block_index - 1,
                }];
                return Some(BoundaryDeleteOutcome::Edit(Box::new(
                    Transaction::new(steps).with_selections(
                        selection_before,
                        Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                    ),
                )));
            }
            BlockKind::Text { .. } if prev_block.kind.code_block_data().is_some() => {
                let prev_inline = prev_block.kind.inline_content()?;
                if prev_inline.is_empty() {
                    let steps = vec![Step::RemoveBlockIn {
                        target,
                        index: block_index - 1,
                    }];
                    return Some(BoundaryDeleteOutcome::Edit(Box::new(
                        Transaction::new(steps).with_selections(
                            selection_before,
                            Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                        ),
                    )));
                }
                let target_pos = self.doc_pos_for_block(prev_block.id, prev_inline.len())?;
                return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                    DocumentSelection::caret(target_pos, Affinity::Downstream),
                )));
            }
            BlockKind::Text { .. } => {}
        }

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
        if self.anchors.bookmarks().next().is_some() {
            steps.push(Step::MapMergeAnchors {
                block_id: prev_block.id,
                offset: join_offset,
                source_block_id: leaf_id,
                source_offset: 0,
            });
        }
        steps.push(Step::RemoveBlockIn {
            target,
            index: block_index,
        });

        let next_pos = self.doc_pos_for_block(prev_block.id, join_offset)?;
        Some(BoundaryDeleteOutcome::Edit(Box::new(
            Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            ),
        )))
    }

    /// Builds a container-aware transaction merging the block at `pos` into its preceding sibling
    /// (used for Backspace at offset 0).
    pub fn build_merge_with_previous_transaction(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        match self.plan_backward_boundary_delete(pos, selection_before)? {
            BoundaryDeleteOutcome::Edit(transaction) => Some(*transaction),
            BoundaryDeleteOutcome::MoveSelection(_) => None,
        }
    }

    /// Plans forward Delete at the end of `pos`, distinguishing destructive edits from safe
    /// non-destructive caret entry into following tables, containers, or code blocks.
    pub fn plan_forward_boundary_delete(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<BoundaryDeleteOutcome> {
        let leaf_id = pos.leaf_block_id()?;
        let (target, block_index) = self.locate_block(leaf_id)?;
        let siblings = self.container_blocks(target)?;
        let current_block = siblings.get(block_index)?;
        if current_block.kind.code_block_data().is_some() {
            return None;
        }
        if target == ContainerTarget::Root && block_index + 1 >= self.content_end_index() {
            return None;
        }
        let next_block = siblings.get(block_index + 1)?;
        match &next_block.kind {
            BlockKind::Leaf { .. } => {
                let steps = vec![Step::RemoveBlockIn {
                    target,
                    index: block_index + 1,
                }];
                return Some(BoundaryDeleteOutcome::Edit(Box::new(
                    Transaction::new(steps).with_selections(
                        selection_before,
                        Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                    ),
                )));
            }
            BlockKind::Grid { .. } => {
                let target_pos = self.first_text_pos_in_node(next_block)?;
                return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                    DocumentSelection::caret(target_pos, Affinity::Downstream),
                )));
            }
            BlockKind::Container { .. } => {
                if let Some(target_pos) = self.first_text_pos_in_node(next_block) {
                    return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                        DocumentSelection::caret(target_pos, Affinity::Downstream),
                    )));
                }
                let steps = vec![Step::RemoveBlockIn {
                    target,
                    index: block_index + 1,
                }];
                return Some(BoundaryDeleteOutcome::Edit(Box::new(
                    Transaction::new(steps).with_selections(
                        selection_before,
                        Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                    ),
                )));
            }
            BlockKind::Text { .. } if next_block.kind.code_block_data().is_some() => {
                let next_inline = next_block.kind.inline_content()?;
                if next_inline.is_empty() {
                    let steps = vec![Step::RemoveBlockIn {
                        target,
                        index: block_index + 1,
                    }];
                    return Some(BoundaryDeleteOutcome::Edit(Box::new(
                        Transaction::new(steps).with_selections(
                            selection_before,
                            Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
                        ),
                    )));
                }
                let target_pos = self.doc_pos_for_block(next_block.id, 0)?;
                return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                    DocumentSelection::caret(target_pos, Affinity::Downstream),
                )));
            }
            BlockKind::Text { .. } => {}
        }

        let current_inline = current_block.kind.inline_content()?;
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
        if self.anchors.bookmarks().next().is_some() {
            steps.push(Step::MapMergeAnchors {
                block_id: leaf_id,
                offset: join_offset,
                source_block_id: next_block.id,
                source_offset: 0,
            });
        }
        steps.push(Step::RemoveBlockIn {
            target,
            index: block_index + 1,
        });

        Some(BoundaryDeleteOutcome::Edit(Box::new(
            Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(pos.clone(), Affinity::Downstream)),
            ),
        )))
    }

    /// Builds a container-aware transaction merging the next sibling into the block at `pos`
    /// (used for forward Delete at the end of a block).
    pub fn build_merge_with_next_transaction(
        &self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        match self.plan_forward_boundary_delete(pos, selection_before)? {
            BoundaryDeleteOutcome::Edit(transaction) => Some(*transaction),
            BoundaryDeleteOutcome::MoveSelection(_) => None,
        }
    }

    /// Exits a code block, non-footnote container, or table by moving to the next sibling text
    /// block or inserting a fresh paragraph immediately after it.
    pub fn plan_exit_block(
        &mut self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<BoundaryDeleteOutcome> {
        let leaf_id = pos.leaf_block_id()?;
        let exit_block_id = if self
            .find_block(leaf_id)
            .is_some_and(|block| block.kind.code_block_data().is_some())
        {
            leaf_id
        } else {
            pos.path
                .iter()
                .rev()
                .skip(1)
                .find_map(|segment| match *segment {
                    PathSegment::Block(id) => self
                        .find_block(id)
                        .filter(|block| {
                            matches!(block.kind, BlockKind::Container { .. })
                                && !crate::model::anchors::is_footnote_body(block)
                        })
                        .map(|_| id),
                    PathSegment::TableCell { table_id, .. } => Some(table_id),
                })?
        };

        let (parent_target, block_index) = self.locate_block(exit_block_id)?;
        let siblings = self.container_blocks(parent_target)?;
        let next_index = block_index + 1;
        let within_content =
            parent_target != ContainerTarget::Root || next_index < self.content_end_index();
        if within_content
            && let Some(next_sibling) = siblings.get(next_index)
            && next_sibling.kind.inline_content().is_some()
        {
            let target_pos = self.doc_pos_for_block(next_sibling.id, 0)?;
            return Some(BoundaryDeleteOutcome::MoveSelection(Box::new(
                DocumentSelection::caret(target_pos, Affinity::Downstream),
            )));
        }

        let new_paragraph_id = self.alloc_block_id();
        let new_paragraph = BlockNode::new(
            new_paragraph_id,
            BlockKind::paragraph(InlineContent::empty()),
            BlockStyle::default(),
        );
        let next_pos = self.doc_pos_for_container_child(parent_target, new_paragraph_id, 0)?;
        let transaction = Transaction::new(vec![Step::InsertBlockIn {
            target: parent_target,
            index: next_index,
            block: new_paragraph,
        }])
        .with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        );
        Some(BoundaryDeleteOutcome::Edit(Box::new(transaction)))
    }

    /// Toggles blockquote wrapping for `block_ids`: unwraps if all selected blocks are already
    /// inside blockquotes, or wraps the contiguous sibling span in a new blockquote otherwise.
    pub fn build_toggle_blockquote_transaction(
        &mut self,
        block_ids: &[BlockId],
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        if block_ids.is_empty() {
            return None;
        }

        let enclosing_quotes: Vec<Option<BlockId>> = block_ids
            .iter()
            .map(|&block_id| self.enclosing_blockquote_id(block_id))
            .collect();

        if enclosing_quotes.iter().all(Option::is_some) {
            let mut quote_id_order: Vec<BlockId> = Vec::new();
            for quote_id in enclosing_quotes.into_iter().flatten() {
                if !quote_id_order.contains(&quote_id) {
                    quote_id_order.push(quote_id);
                }
            }
            if quote_id_order.len() != 1 {
                return None;
            }
            let quote_id = quote_id_order[0];
            let quote_block = self.find_block(quote_id)?.clone();
            let quote_children = quote_block.kind.children()?;
            let (parent_target, quote_index) = self.locate_block(quote_id)?;

            let mut min_child_index = usize::MAX;
            let mut max_child_index = 0usize;
            for &block_id in block_ids {
                let child_index = quote_children
                    .iter()
                    .position(|child| child.id == block_id || contains_block_id(child, block_id))?;
                min_child_index = min_child_index.min(child_index);
                max_child_index = max_child_index.max(child_index);
            }
            if min_child_index > max_child_index || max_child_index >= quote_children.len() {
                return None;
            }

            let before = quote_children[..min_child_index].to_vec();
            let lifted = quote_children[min_child_index..=max_child_index].to_vec();
            let after = quote_children[(max_child_index + 1)..].to_vec();

            let mut steps = vec![Step::RemoveBlockIn {
                target: parent_target,
                index: quote_index,
            }];
            let mut insert_at = quote_index;
            if !before.is_empty() {
                steps.push(Step::InsertBlockIn {
                    target: parent_target,
                    index: insert_at,
                    block: BlockNode::new(
                        quote_id,
                        BlockKind::blockquote(before),
                        quote_block.style.clone(),
                    ),
                });
                insert_at += 1;
            }
            for child in lifted {
                steps.push(Step::InsertBlockIn {
                    target: parent_target,
                    index: insert_at,
                    block: child,
                });
                insert_at += 1;
            }
            if !after.is_empty() {
                let trailing_quote_id = self.alloc_block_id();
                steps.push(Step::InsertBlockIn {
                    target: parent_target,
                    index: insert_at,
                    block: BlockNode::new(
                        trailing_quote_id,
                        BlockKind::blockquote(after),
                        quote_block.style,
                    ),
                });
            }

            let selection_after = selection_before
                .clone()
                .map(|selection| remove_path_segment_from_selection(selection, quote_id));
            return Some(
                Transaction::new(steps).with_selections(selection_before, selection_after),
            );
        }

        let first_id = *block_ids.first()?;
        let (target, first_index) = self.locate_block(first_id)?;
        let siblings = self.container_blocks(target)?;
        let mut min_index = first_index;
        let mut max_index = first_index;
        for &block_id in block_ids.iter().skip(1) {
            let (other_target, other_index) = self.locate_block(block_id)?;
            if other_target != target {
                return None;
            }
            min_index = min_index.min(other_index);
            max_index = max_index.max(other_index);
        }
        if target == ContainerTarget::Root && max_index >= self.content_end_index() {
            return None;
        }
        let wrapped_slice = siblings.get(min_index..=max_index)?;
        if wrapped_slice
            .iter()
            .any(crate::model::anchors::is_footnote_body)
        {
            return None;
        }
        let wrapped_ids: Vec<BlockId> = wrapped_slice.iter().map(|block| block.id).collect();
        let wrapped_children = wrapped_slice.to_vec();
        let quote_id = self.alloc_block_id();
        let quote_block = BlockNode::new(
            quote_id,
            BlockKind::blockquote(wrapped_children),
            default_blockquote_style(),
        );

        let mut steps = Vec::with_capacity(wrapped_ids.len() + 1);
        for index in (min_index..=max_index).rev() {
            steps.push(Step::RemoveBlockIn { target, index });
        }
        steps.push(Step::InsertBlockIn {
            target,
            index: min_index,
            block: quote_block,
        });

        let selection_after = selection_before
            .clone()
            .map(|selection| insert_path_ancestor_in_selection(selection, &wrapped_ids, quote_id));
        Some(Transaction::new(steps).with_selections(selection_before, selection_after))
    }

    /// Toggles code-block formatting for `block_ids`: converts a code block into sibling
    /// paragraphs split by `\n`, or merges contiguous text siblings into one code block.
    pub fn build_toggle_code_block_transaction(
        &mut self,
        block_ids: &[BlockId],
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        if block_ids.is_empty() {
            return None;
        }

        if block_ids.len() == 1 {
            let block_id = block_ids[0];
            let block = self.find_block(block_id)?;
            if block.kind.code_block_data().is_some() {
                let text = block.kind.inline_content()?.text.clone();
                let focus_offset = match selection_before.as_ref() {
                    Some(DocumentSelection::Text { focus, .. })
                        if focus.leaf_block_id() == Some(block_id) =>
                    {
                        focus.offset.min(text.len())
                    }
                    _ => text.len(),
                };
                let lines: Vec<&str> = text.split('\n').collect();
                let (target, block_index) = self.locate_block(block_id)?;
                let mut steps = vec![
                    Step::SetBlockKind {
                        block_id,
                        kind: BlockKind::paragraph(InlineContent::plain(
                            lines.first().copied().unwrap_or(""),
                        )),
                    },
                    Step::SetBlockStyle {
                        block_id,
                        style: BlockStyle::default(),
                    },
                ];

                let mut line_block_ids = Vec::with_capacity(lines.len());
                line_block_ids.push(block_id);
                for (line_offset, line) in lines.iter().enumerate().skip(1) {
                    let new_id = self.alloc_block_id();
                    line_block_ids.push(new_id);
                    steps.push(Step::InsertBlockIn {
                        target,
                        index: block_index + line_offset,
                        block: BlockNode::new(
                            new_id,
                            BlockKind::paragraph(InlineContent::plain(*line)),
                            BlockStyle::default(),
                        ),
                    });
                }

                let mut remaining = focus_offset;
                let mut target_line_index = 0usize;
                for (line_index, line) in lines.iter().enumerate() {
                    if remaining <= line.len() || line_index + 1 == lines.len() {
                        target_line_index = line_index;
                        break;
                    }
                    remaining = remaining.saturating_sub(line.len() + 1);
                }
                let caret_block_id = line_block_ids[target_line_index];
                let caret_offset = remaining.min(lines[target_line_index].len());
                let next_pos =
                    self.doc_pos_for_container_child(target, caret_block_id, caret_offset)?;
                return Some(Transaction::new(steps).with_selections(
                    selection_before,
                    Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
                ));
            }
        }

        let first_id = *block_ids.first()?;
        let (target, first_index) = self.locate_block(first_id)?;
        let siblings = self.container_blocks(target)?;
        let mut min_index = first_index;
        let mut max_index = first_index;
        for &block_id in block_ids.iter().skip(1) {
            let (other_target, other_index) = self.locate_block(block_id)?;
            if other_target != target {
                return None;
            }
            min_index = min_index.min(other_index);
            max_index = max_index.max(other_index);
        }
        let slice = siblings.get(min_index..=max_index)?;
        if !slice
            .iter()
            .all(|block| block.kind.inline_content().is_some())
        {
            return None;
        }

        let focus_target = match selection_before.as_ref() {
            Some(DocumentSelection::Text { focus, .. }) => {
                focus.leaf_block_id().map(|id| (id, focus.offset))
            }
            _ => None,
        };
        let mut joined = String::new();
        let mut mapped_focus_offset = None;
        for (offset_index, block) in slice.iter().enumerate() {
            if offset_index > 0 {
                joined.push('\n');
            }
            let text = &block.kind.inline_content()?.text;
            if let Some((focus_id, focus_offset)) = focus_target
                && focus_id == block.id
            {
                mapped_focus_offset = Some(joined.len() + focus_offset.min(text.len()));
            }
            joined.push_str(text);
        }

        let kept_id = slice.first()?.id;
        let mut steps = vec![
            Step::SetBlockKind {
                block_id: kept_id,
                kind: BlockKind::code_block(None, InlineContent::plain(joined.clone())),
            },
            Step::SetBlockStyle {
                block_id: kept_id,
                style: BlockStyle::named(StyleId::CODE_BLOCK),
            },
        ];
        for index in ((min_index + 1)..=max_index).rev() {
            steps.push(Step::RemoveBlockIn { target, index });
        }
        let caret_offset = mapped_focus_offset.unwrap_or(joined.len());
        let next_pos = self.doc_pos_for_container_child(target, kept_id, caret_offset)?;
        Some(Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        ))
    }

    /// Updates the language tag on a code block without altering its content or structure.
    pub fn build_set_code_block_language_transaction(
        &self,
        block_id: BlockId,
        language: Option<String>,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let block = self.find_block(block_id)?;
        let current = block.kind.code_block_data()?;
        let normalized = language
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if current.language == normalized {
            return None;
        }
        let step = Step::UpdateBlockData {
            block_id,
            expected_kind: Arc::from(block_kinds::CODE_BLOCK),
            expected_shape: BlockShapeKind::Text,
            data: CustomPayload::typed(CodeBlockData {
                language: normalized,
            }),
        };
        Some(
            Transaction::new(vec![step])
                .with_selections(selection_before.clone(), selection_before),
        )
    }

    /// Inserts a horizontal rule at collapsed position `pos`, ensuring the caret lands in an
    /// editable paragraph immediately after the rule.
    pub fn build_insert_horizontal_rule_transaction(
        &mut self,
        pos: &DocPos,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let rule_id = self.alloc_block_id();
        let rule_block = BlockNode::new(
            rule_id,
            BlockKind::horizontal_rule(),
            default_horizontal_rule_style(),
        );
        self.build_insert_structural_block_transaction(pos, rule_block, None, selection_before)
    }

    /// Inserts a `rows × columns` table at collapsed position `pos`, ensuring a trailing paragraph
    /// exists when inserted at the end of a container and placing the caret in cell `(0, 0)`.
    pub fn build_insert_table_transaction(
        &mut self,
        pos: &DocPos,
        rows: usize,
        columns: usize,
        with_header: bool,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let row_count = rows.max(1);
        let col_count = columns.max(1);
        let table_id = self.alloc_block_id();
        let mut table_rows = Vec::with_capacity(row_count);
        let mut first_cell_para_id = None;
        for row_index in 0..row_count {
            let mut cells = Vec::with_capacity(col_count);
            for col_index in 0..col_count {
                let cell_id = self.alloc_block_id();
                let para_id = self.alloc_block_id();
                if row_index == 0 && col_index == 0 {
                    first_cell_para_id = Some(para_id);
                }
                cells.push(TableCell::simple(
                    cell_id,
                    vec![BlockNode::new(
                        para_id,
                        BlockKind::paragraph(InlineContent::empty()),
                        BlockStyle::default(),
                    )],
                ));
            }
            table_rows.push(TableRow {
                is_header: with_header && row_index == 0,
                cells,
            });
        }
        let table_block = BlockNode::new(
            table_id,
            BlockKind::table(TableBlock {
                columns: vec![ColumnSpec::Auto; col_count],
                column_kinds: Default::default(),
                rows: table_rows,
                border_style: TableBorderStyle::default(),
            }),
            BlockStyle::default(),
        );
        let caret_in_table = first_cell_para_id.map(|para_id| (table_id, para_id));
        self.build_insert_structural_block_transaction(
            pos,
            table_block,
            caret_in_table,
            selection_before,
        )
    }

    fn build_insert_structural_block_transaction(
        &mut self,
        pos: &DocPos,
        inserted_block: BlockNode,
        caret_in_table: Option<(BlockId, BlockId)>,
        selection_before: Option<DocumentSelection>,
    ) -> Option<Transaction> {
        let leaf_id = pos.leaf_block_id()?;
        let (target, block_index) = self.locate_block(leaf_id)?;
        let block = self.find_block(leaf_id)?.clone();
        let inline = block.kind.inline_content()?;
        let offset = pos.offset.min(inline.len());
        let siblings = self.container_blocks(target)?;

        let mut steps = Vec::new();
        let following_pos: DocPos;

        if block.kind.code_block_data().is_some() || offset == inline.len() {
            if inline.is_empty() && block.kind.is_paragraph() {
                steps.push(Step::InsertBlockIn {
                    target,
                    index: block_index,
                    block: inserted_block,
                });
                following_pos = self.doc_pos_for_container_child(target, leaf_id, 0)?;
            } else {
                steps.push(Step::InsertBlockIn {
                    target,
                    index: block_index + 1,
                    block: inserted_block,
                });
                let next_sibling_index = block_index + 1;
                let has_following_text = (target != ContainerTarget::Root
                    || next_sibling_index < self.content_end_index())
                    && siblings
                        .get(next_sibling_index)
                        .is_some_and(|sibling| sibling.kind.inline_content().is_some());
                if has_following_text {
                    let next_id = siblings.get(next_sibling_index)?.id;
                    following_pos = self.doc_pos_for_container_child(target, next_id, 0)?;
                } else {
                    let trailing_id = self.alloc_block_id();
                    steps.push(Step::InsertBlockIn {
                        target,
                        index: block_index + 2,
                        block: BlockNode::new(
                            trailing_id,
                            BlockKind::paragraph(InlineContent::empty()),
                            BlockStyle::default(),
                        ),
                    });
                    following_pos = self.doc_pos_for_container_child(target, trailing_id, 0)?;
                }
            }
        } else if offset == 0 {
            steps.push(Step::InsertBlockIn {
                target,
                index: block_index,
                block: inserted_block,
            });
            following_pos = self.doc_pos_for_container_child(target, leaf_id, 0)?;
        } else {
            let trailing_content = inline.slice(offset..inline.len());
            let trailing_id = self.alloc_block_id();
            if self.anchors.bookmarks().next().is_some() {
                steps.push(Step::MapSplitAnchors {
                    block_id: leaf_id,
                    offset,
                    new_block_id: trailing_id,
                });
            }
            steps.push(Step::DeleteRange {
                block_id: leaf_id,
                range: offset..inline.len(),
            });
            steps.push(Step::InsertBlockIn {
                target,
                index: block_index + 1,
                block: inserted_block,
            });
            steps.push(Step::InsertBlockIn {
                target,
                index: block_index + 2,
                block: BlockNode::new(
                    trailing_id,
                    BlockKind::paragraph(trailing_content),
                    BlockStyle::default(),
                ),
            });
            following_pos = self.doc_pos_for_container_child(target, trailing_id, 0)?;
        }

        let caret_pos = if let Some((table_id, para_id)) = caret_in_table {
            let mut path = self.path_for_container_target(target)?;
            path.push(PathSegment::TableCell {
                table_id,
                row: 0,
                col: 0,
            });
            path.push(PathSegment::Block(para_id));
            DocPos { path, offset: 0 }
        } else {
            following_pos
        };

        Some(Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(caret_pos, Affinity::Downstream)),
        ))
    }

    fn path_for_container_target(
        &self,
        target: ContainerTarget,
    ) -> Option<SmallVec<[PathSegment; 4]>> {
        match target {
            ContainerTarget::Root => Some(SmallVec::new()),
            ContainerTarget::Container(container_id) => self.path_for_block(container_id),
            ContainerTarget::TableCell { table_id, row, col } => {
                let mut path = self.path_for_block(table_id)?;
                path.pop();
                path.push(PathSegment::TableCell { table_id, row, col });
                Some(path)
            }
        }
    }

    fn doc_pos_for_container_child(
        &self,
        target: ContainerTarget,
        block_id: BlockId,
        offset: usize,
    ) -> Option<DocPos> {
        let mut path = self.path_for_container_target(target)?;
        path.push(PathSegment::Block(block_id));
        Some(DocPos { path, offset })
    }

    fn enclosing_blockquote_id(&self, block_id: BlockId) -> Option<BlockId> {
        let path = self.path_for_block(block_id)?;
        path.iter()
            .rev()
            .skip(1)
            .find_map(|segment| match *segment {
                PathSegment::Block(id) => self
                    .find_block(id)
                    .filter(|block| block.kind.semantic_kind() == block_kinds::BLOCKQUOTE)
                    .map(|_| id),
                PathSegment::TableCell { .. } => None,
            })
    }

    fn first_text_pos_in_node(&self, node: &BlockNode) -> Option<DocPos> {
        match &node.kind {
            BlockKind::Text { .. } => self.doc_pos_for_block(node.id, 0),
            BlockKind::Container { children, .. } => children
                .iter()
                .find_map(|child| self.first_text_pos_in_node(child)),
            BlockKind::Grid { table, .. } => table.rows.iter().find_map(|row| {
                row.cells.iter().find_map(|cell| {
                    cell.blocks
                        .iter()
                        .find_map(|child| self.first_text_pos_in_node(child))
                })
            }),
            BlockKind::Leaf { .. } => None,
        }
    }

    fn last_text_pos_in_node(&self, node: &BlockNode) -> Option<DocPos> {
        match &node.kind {
            BlockKind::Text { content, .. } => self.doc_pos_for_block(node.id, content.len()),
            BlockKind::Container { children, .. } => children
                .iter()
                .rev()
                .find_map(|child| self.last_text_pos_in_node(child)),
            BlockKind::Grid { table, .. } => table.rows.iter().rev().find_map(|row| {
                row.cells.iter().rev().find_map(|cell| {
                    cell.blocks
                        .iter()
                        .rev()
                        .find_map(|child| self.last_text_pos_in_node(child))
                })
            }),
            BlockKind::Leaf { .. } => None,
        }
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
                kind: BlockKind::raw_grid(kind.clone(), data.clone(), next_table),
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
            .and_then(|para_id| self.table_cell_doc_pos(table_id, insert_at, 0, para_id, 0))
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        Some(
            Transaction::new(vec![Step::SetBlockKind {
                block_id: table_id,
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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
        next_table.column_kinds = std::mem::take(&mut next_table.column_kinds)
            .into_iter()
            .map(|(column, kind)| (column + usize::from(column >= insert_at), kind))
            .collect();

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
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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
        next_table.column_kinds = std::mem::take(&mut next_table.column_kinds)
            .into_iter()
            .filter_map(|(column, kind)| {
                if column < start_col {
                    Some((column, kind))
                } else if column >= end_col {
                    Some((column - (end_col - start_col), kind))
                } else {
                    None
                }
            })
            .collect();

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
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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
        let target = placements.iter().copied().find(|placement| {
            placement.row_index == row_index && placement.cell_index == cell_index
        })?;
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
                kind: BlockKind::raw_grid(grid_kind, grid_data, next_table),
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

    pub(crate) fn table_cell_doc_pos(
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

fn default_blockquote_style() -> BlockStyle {
    BlockStyle {
        indent_start_pt: 16.0,
        space_before_pt: Some(6.0),
        space_after_pt: Some(10.0),
        ..BlockStyle::default()
    }
}

fn default_horizontal_rule_style() -> BlockStyle {
    BlockStyle {
        space_before_pt: Some(10.0),
        space_after_pt: Some(10.0),
        ..BlockStyle::default()
    }
}

fn contains_block_id(node: &BlockNode, target_id: BlockId) -> bool {
    if node.id == target_id {
        return true;
    }
    match &node.kind {
        BlockKind::Container { children, .. } => children
            .iter()
            .any(|child| contains_block_id(child, target_id)),
        BlockKind::Grid { table, .. } => table.rows.iter().any(|row| {
            row.cells.iter().any(|cell| {
                cell.blocks
                    .iter()
                    .any(|child| contains_block_id(child, target_id))
            })
        }),
        BlockKind::Text { .. } | BlockKind::Leaf { .. } => false,
    }
}

fn remove_path_segment_from_pos(mut pos: DocPos, removed_id: BlockId) -> DocPos {
    pos.path
        .retain(|segment| *segment != PathSegment::Block(removed_id));
    pos
}

fn remove_path_segment_from_selection(
    selection: DocumentSelection,
    removed_id: BlockId,
) -> DocumentSelection {
    match selection {
        DocumentSelection::Text {
            anchor,
            focus,
            affinity,
            goal_x,
            stored_marks,
        } => DocumentSelection::Text {
            anchor: remove_path_segment_from_pos(anchor, removed_id),
            focus: remove_path_segment_from_pos(focus, removed_id),
            affinity,
            goal_x,
            stored_marks,
        },
        other => other,
    }
}

fn insert_path_ancestor_in_pos(
    mut pos: DocPos,
    wrapped_ids: &[BlockId],
    ancestor_id: BlockId,
) -> DocPos {
    if let Some(insert_at) = pos.path.iter().position(|segment| match *segment {
        PathSegment::Block(id) | PathSegment::TableCell { table_id: id, .. } => {
            wrapped_ids.contains(&id)
        }
    }) {
        pos.path.insert(insert_at, PathSegment::Block(ancestor_id));
    }
    pos
}

fn insert_path_ancestor_in_selection(
    selection: DocumentSelection,
    wrapped_ids: &[BlockId],
    ancestor_id: BlockId,
) -> DocumentSelection {
    match selection {
        DocumentSelection::Text {
            anchor,
            focus,
            affinity,
            goal_x,
            stored_marks,
        } => DocumentSelection::Text {
            anchor: insert_path_ancestor_in_pos(anchor, wrapped_ids, ancestor_id),
            focus: insert_path_ancestor_in_pos(focus, wrapped_ids, ancestor_id),
            affinity,
            goal_x,
            stored_marks,
        },
        other => other,
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
    timestamp: HistoryInstant,
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
        self.push_inverse_grouped_at(inverse, HistoryMergeGroup::Atomic, HistoryInstant::now());
    }

    /// Pushes an inverse transaction with coalescing metadata using the current clock time.
    pub fn push_inverse_grouped(&mut self, inverse: Transaction, group: HistoryMergeGroup) {
        self.push_inverse_grouped_at(inverse, group, HistoryInstant::now());
    }

    /// Pushes an inverse transaction at an explicit `timestamp` (deterministic for unit tests).
    pub fn push_inverse_grouped_at(
        &mut self,
        inverse: Transaction,
        group: HistoryMergeGroup,
        timestamp: HistoryInstant,
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
            combined_steps.append(&mut previous.inverse.steps);
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
            timestamp: HistoryInstant::now(),
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

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }
}
