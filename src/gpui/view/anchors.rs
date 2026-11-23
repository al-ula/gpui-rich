//! GPUI-side durable anchor resolution and command execution.
use super::RichDocumentView;
use gpui_kit::Context;
use crate::model::{
    AnchorCommand, AnchorId, AnchorPosition, BlockId, BlockKind, BlockNode, BlockStyle,
    CommandOutcome, DocPos, Document, InlineContent, InlineStyle, Step, Transaction,
};

/// Resolve a model bookmark to a fresh caret position. The caller passes the result to exact reveal.
pub(super) fn resolve_anchor_position(
    document: &Document,
    target: AnchorPosition,
) -> Option<DocPos> {
    let content = document
        .find_block(target.block_id)?
        .kind
        .inline_content()?;
    if target.offset > content.text.len() || !content.text.is_char_boundary(target.offset) {
        return None;
    }
    document.doc_pos_for_block(target.block_id, target.offset)
}

impl RichDocumentView {
    pub(super) fn execute_anchor_command(
        &mut self,
        command: &AnchorCommand,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        match command {
            AnchorCommand::Reveal { position } => self.reveal_anchor_position(*position, cx),
            AnchorCommand::RevealBookmark { id } => self
                .document
                .anchors
                .bookmark(*id)
                .map(|b| b.position)
                .map_or(CommandOutcome::NoOp, |p| self.reveal_anchor_position(p, cx)),
            AnchorCommand::RevealFootnote { id } => {
                let position = self
                    .document
                    .anchors
                    .footnote(*id)
                    .and_then(|note| self.document.find_block(note.block_id))
                    .and_then(|block| block.kind.children()?.first())
                    .map(|block| AnchorPosition {
                        block_id: block.id,
                        offset: 0,
                    });
                position.map_or(CommandOutcome::NoOp, |position| {
                    self.reveal_anchor_position(position, cx)
                })
            }
            AnchorCommand::RevealFootnoteReference { id } => {
                let position = self
                    .document
                    .collect_leaf_blocks()
                    .into_iter()
                    .find_map(|block| {
                        block.kind.inline_content().and_then(|content| {
                            content.objects.iter().find_map(|entry| {
                                (crate::model::footnote_reference_id(&entry.object) == Some(*id))
                                    .then_some(AnchorPosition {
                                        block_id: block.id,
                                        offset: entry.byte_offset,
                                    })
                            })
                        })
                    });
                position.map_or(CommandOutcome::NoOp, |p| self.reveal_anchor_position(p, cx))
            }
            AnchorCommand::AddBookmark {
                name,
                position,
                affinity,
            } => {
                if resolve_anchor_position(&self.document, *position).is_none() {
                    return CommandOutcome::NoOp;
                }
                let mut anchors = self.document.anchors.clone();
                anchors.add_bookmark(name.clone(), *position, *affinity);
                self.apply_transaction(
                    Transaction::new(vec![Step::SetAnchors { anchors }]),
                    true,
                    cx,
                );
                CommandOutcome::Executed
            }
            AnchorCommand::RemoveBookmark { id } => {
                let mut anchors = self.document.anchors.clone();
                if anchors.remove_bookmark(*id).is_none() {
                    return CommandOutcome::NoOp;
                }
                self.apply_transaction(
                    Transaction::new(vec![Step::SetAnchors { anchors }]),
                    true,
                    cx,
                );
                CommandOutcome::Executed
            }
            AnchorCommand::AddFootnote { body } => {
                let Some((_, focus, _)) = self.active_text_selection() else {
                    return CommandOutcome::NoOp;
                };
                let Some(block_id) = focus.leaf_block_id() else {
                    return CommandOutcome::NoOp;
                };
                self.add_footnote(body, (block_id, focus.offset), cx)
            }
            AnchorCommand::InsertFootnoteReference {
                block_id,
                offset,
                body,
            } => self.add_footnote(body, (*block_id, *offset), cx),
            AnchorCommand::SetFootnoteBody { id, body } => {
                let Some(note) = self.document.anchors.footnote(*id) else {
                    return CommandOutcome::NoOp;
                };
                let Some(block) = self.document.find_block(note.block_id) else {
                    return CommandOutcome::NoOp;
                };
                let Some(first) = block.kind.children().and_then(|children| children.first())
                else {
                    return CommandOutcome::NoOp;
                };
                let mut first = first.clone();
                first.kind = BlockKind::paragraph(InlineContent::plain(body));
                first.bump_version();
                let kind = crate::model::footnote_body_kind(vec![first]);
                self.apply_transaction(
                    Transaction::new(vec![Step::SetBlockKind {
                        block_id: note.block_id,
                        kind,
                    }]),
                    true,
                    cx,
                );
                CommandOutcome::Executed
            }
            AnchorCommand::RemoveFootnote { id } => self.remove_footnote(*id, cx),
        }
    }

    fn reveal_anchor_position(
        &mut self,
        position: AnchorPosition,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(pos) = resolve_anchor_position(&self.document, position) else {
            return CommandOutcome::NoOp;
        };
        self.jump_to_position(&pos, cx)
    }

    fn add_footnote(
        &mut self,
        body: &str,
        (reference, offset): (BlockId, usize),
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(position) = self.document.doc_pos_for_block(reference, offset) else {
            return CommandOutcome::NoOp;
        };
        if crate::model::validate_doc_pos(&self.document, &position).is_none() {
            return CommandOutcome::NoOp;
        }
        let block_id = self.document.alloc_block_id();
        let body_id = self.document.alloc_block_id();
        let paragraph = BlockNode::new(
            body_id,
            BlockKind::paragraph(InlineContent::plain(body)),
            BlockStyle::default(),
        );
        let note = BlockNode::new(
            block_id,
            crate::model::footnote_body_kind(vec![paragraph]),
            BlockStyle::default(),
        );
        let mut anchors = self.document.anchors.clone();
        let note_id = anchors.add_footnote_block(block_id);
        let mut fragment = InlineContent::empty();
        fragment.push_object(
            crate::model::footnote_reference(note_id),
            InlineStyle::default(),
        );
        let steps = vec![
            Step::SetAnchors { anchors },
            Step::InsertContent {
                block_id: reference,
                offset,
                content: fragment,
            },
            Step::InsertBlock {
                index: self.document.blocks.len(),
                block: note,
            },
        ];
        let selection = self.selection.clone();
        self.apply_transaction(
            Transaction::new(steps).with_selections(selection.clone(), selection),
            true,
            cx,
        );
        CommandOutcome::Executed
    }

    fn remove_footnote(&mut self, id: AnchorId, cx: &mut Context<Self>) -> CommandOutcome {
        let Some(note) = self.document.anchors.footnote(id).cloned() else {
            return CommandOutcome::NoOp;
        };
        let Some(index) = self.document.top_level_index(note.block_id) else {
            return CommandOutcome::NoOp;
        };
        let mut anchors = self.document.anchors.clone();
        anchors.remove_footnote(id);
        let mut steps = vec![Step::SetAnchors { anchors }];
        for block in self.document.collect_leaf_blocks() {
            let Some(content) = block.kind.inline_content() else {
                continue;
            };
            for entry in content.objects.iter().rev() {
                if crate::model::footnote_reference_id(&entry.object) == Some(id) {
                    steps.push(Step::DeleteRange {
                        block_id: block.id,
                        range: entry.byte_offset
                            ..entry.byte_offset + crate::model::inline::OBJECT_REPLACEMENT_UTF8_LEN,
                    });
                }
            }
        }
        steps.push(Step::RemoveBlock { index });
        self.apply_transaction(Transaction::new(steps), true, cx);
        CommandOutcome::Executed
    }
}
