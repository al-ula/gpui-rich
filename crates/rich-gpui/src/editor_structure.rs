//! Structural editing glue and reversible typed-list input rules.

use super::*;
use rich_model::{ListId, StructureCommand};

/// Pending marker conversion, retained only until immediate Backspace or invalidated by other input.
#[derive(Clone, Debug)]
pub(super) struct InputRuleUndo {
    pub block_id: BlockId,
    pub content: InlineContent,
    pub style: BlockStyle,
    pub list_id: ListId,
    pub list_kind: ListKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TypedListPrefix {
    pub marker_end: usize,
    pub kind: ListKind,
    pub start_number: Option<u32>,
}

pub(super) fn recognize_typed_list_prefix(text: &str) -> Option<TypedListPrefix> {
    if text == "- " {
        return Some(TypedListPrefix {
            marker_end: 2,
            kind: ListKind::Bullet,
            start_number: None,
        });
    }
    let digits = text.strip_suffix(". ")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = digits.parse::<u32>().ok()?;
    if number == 0 {
        return None;
    }
    Some(TypedListPrefix {
        marker_end: text.len(),
        kind: ListKind::Ordered(NumberStyle::Decimal),
        start_number: Some(number),
    })
}

impl RichDocumentView {
    /// Executes a structural command through the normal transaction/history/layout path.
    pub(super) fn execute_structure_command(
        &mut self,
        command: &StructureCommand,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(transaction) = command.transaction(&self.document) else {
            return CommandOutcome::NoOp;
        };
        let selection = self.selection.clone();
        let transaction = transaction.with_selections(selection.clone(), selection);
        self.ime_preedit = None;
        self.history.break_coalescing();
        let changes =
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        if changes.dirty_blocks.is_empty() && !changes.structure_changed {
            CommandOutcome::NoOp
        } else {
            CommandOutcome::Executed
        }
    }

    /// Called before ordinary text insertion; a successful conversion consumes `raw_text`.
    pub(super) fn try_apply_input_rule(&mut self, raw_text: &str, cx: &mut Context<Self>) -> bool {
        self.pending_input_rule_undo = None;
        if !self.policy.editable || raw_text != " " {
            return false;
        }
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return false;
        };
        if anchor != focus {
            return false;
        }
        let Some(block_id) = focus.leaf_block_id() else {
            return false;
        };
        let Some(block) = self.document.find_block(block_id) else {
            return false;
        };
        if !block.kind.is_paragraph() {
            return false;
        }
        let Some(inline) = block.kind.inline_content() else {
            return false;
        };
        if focus.offset != inline.len() {
            return false;
        }
        let mut original_content = inline.clone();
        let mut trigger = inline.text.clone();
        trigger.push_str(raw_text);
        let Some(prefix) = recognize_typed_list_prefix(&trigger) else {
            return false;
        };
        let list_id = self.document.alloc_list_id();
        let original_style = block.style.clone();
        original_content.push_text(raw_text, inline.style_at_caret(focus.offset));
        let before = self.selection.clone();
        let Some(caret) = self.document.doc_pos_for_block(block_id, 0) else {
            return false;
        };
        let mut steps = vec![
            Step::SetBlockKind {
                block_id,
                kind: BlockKind::list_item(list_id, prefix.kind, 0, None, InlineContent::empty()),
            },
            Step::SetBlockStyle {
                block_id,
                style: BlockStyle::default().with_indent(24.0, 0.0),
            },
        ];
        if let Some(start) = prefix.start_number {
            steps.push(Step::SetListNumbering {
                block_id,
                numbering: Some(rich_model::ListNumbering::Restart(start)),
            });
        }
        let transaction = Transaction::new(steps).with_selections(
            before,
            Some(DocumentSelection::caret(caret, Affinity::Downstream)),
        );
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        self.pending_input_rule_undo = Some(InputRuleUndo {
            block_id,
            content: original_content,
            style: original_style,
            list_id,
            list_kind: prefix.kind,
        });
        true
    }

    /// Called before collapsed grapheme Backspace; restores the exact triggering text once.
    pub(super) fn try_undo_input_rule(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_input_rule_undo.take() else {
            return false;
        };
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return false;
        };
        if anchor != focus || focus.leaf_block_id() != Some(pending.block_id) || focus.offset != 0 {
            return false;
        }
        let Some(block) = self.document.find_block(pending.block_id) else {
            return false;
        };
        let Some(list) = block.kind.list_item_data() else {
            return false;
        };
        if list.list_id != pending.list_id || list.kind != pending.list_kind {
            return false;
        }
        let before = self.selection.clone();
        let Some(caret) = self
            .document
            .doc_pos_for_block(pending.block_id, pending.content.len())
        else {
            return false;
        };
        let transaction = Transaction::new(vec![
            Step::SetBlockKind {
                block_id: pending.block_id,
                kind: BlockKind::paragraph(pending.content),
            },
            Step::SetBlockStyle {
                block_id: pending.block_id,
                style: pending.style,
            },
            Step::SetListNumbering {
                block_id: pending.block_id,
                numbering: None,
            },
        ])
        .with_selections(
            before,
            Some(DocumentSelection::caret(caret, Affinity::Downstream)),
        );
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_rules_require_complete_prefix_and_positive_number() {
        assert!(recognize_typed_list_prefix("- ").is_some());
        assert_eq!(
            recognize_typed_list_prefix("12. ").and_then(|prefix| prefix.start_number),
            Some(12)
        );
        assert!(recognize_typed_list_prefix("12.").is_none());
        assert!(recognize_typed_list_prefix("0. ").is_none());
        assert!(recognize_typed_list_prefix("x- ").is_none());
    }
}
