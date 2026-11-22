pub mod block;
pub mod command;
pub mod inline;
pub mod selection;
pub mod style;
pub mod transaction;

pub use block::*;
pub use command::*;
pub use inline::*;
pub use selection::*;
pub use style::*;
pub use transaction::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_utf8_utf16_and_object_sentinels() {
        let mut content = InlineContent::empty();
        content.push_text("Hi ", InlineStyle::default());
        content.push_object(
            InlineObject::SmartChip {
                kind: ChipKind::Person,
                label: "@Alex".into(),
                accent: None,
            },
            InlineStyle::default(),
        );
        content.push_text(" 🚀!", InlineStyle::bold());

        assert!(!content.is_ascii);
        // "Hi " (3) + U+FFFC (1) + " " (1) + rocket (2 UTF-16 units) + "!" (1) = 8
        assert_eq!(content.utf16_len, 8);
        assert!(content.object_at(3).is_some());

        let rocket_byte = content.text.find('🚀').unwrap();
        let rocket_u16 = content.utf8_to_utf16(rocket_byte);
        assert_eq!(rocket_u16, 5);
        assert_eq!(content.utf16_to_utf8(rocket_u16), rocket_byte);
        assert_eq!(content.utf16_to_utf8(rocket_u16 + 2), content.len() - 1);
    }

    #[test]
    fn transaction_undo_redo_roundtrip() {
        let mut doc = Document::new();
        let mut inline = InlineContent::plain("Hello world");
        inline.push_object(InlineObject::FootnoteRef { index: 1 }, InlineStyle::default());
        let p_id = doc.add_paragraph(inline, BlockStyle::default());
        let original = doc.clone();

        let mut history = History::default();

        let applied1 = doc.apply(Transaction::new(vec![
            Step::ApplyInlineStyle {
                block_id: p_id,
                range: 0..5,
                style: InlineStyle::bold(),
            },
            Step::InsertText {
                block_id: p_id,
                offset: 5,
                text: " GPUI".into(),
                style: Some(InlineStyle::italic()),
            },
        ]));
        history.push_inverse(applied1.inverse);

        let applied2 = doc.apply(Transaction::new(vec![Step::DeleteRange {
            block_id: p_id,
            range: 6..11,
        }]));
        history.push_inverse(applied2.inverse);

        let text = &doc.find_block(p_id).unwrap().kind.inline_content().unwrap().text;
        assert!(text.starts_with("Hello world"));

        // Undo both transactions and verify full round-trip equality (excluding block version counter).
        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();

        let orig_inline = original.find_block(p_id).unwrap().kind.inline_content().unwrap();
        let cur_inline = doc.find_block(p_id).unwrap().kind.inline_content().unwrap();
        assert_eq!(orig_inline, cur_inline);

        // Redo restores the edits cleanly.
        history.redo(&mut doc).unwrap();
        history.redo(&mut doc).unwrap();
        let cur_spans = &doc.find_block(p_id).unwrap().kind.inline_content().unwrap().spans;
        assert_eq!(cur_spans[0].range, 0..5);
        assert_eq!(cur_spans[0].style.weight, Some(FontWeight::BOLD));
    }

    #[test]
    fn document_json_serialization_roundtrip() {
        let mut doc = Document::new();
        doc.add_heading(1, InlineContent::plain("Architecture"));
        doc.add_paragraph(InlineContent::plain("Non-paginated flow."), BlockStyle::default());

        let json = doc.to_json_pretty().unwrap();
        let parsed = Document::from_json(&json).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn interaction_policy_gates_user_commands_while_allowing_programmatic() {
        let heading_id = BlockId(1);
        let insert = RichCommand::InsertText("hello".into());
        let select_all = RichCommand::SelectAll;
        let open_link = RichCommand::OpenLink("doc://spec".into());
        let toggle_heading = RichCommand::ToggleHeadingCollapsed(heading_id);

        let viewer = InteractionPolicy::viewer();
        assert!(!viewer.should_show_caret());
        assert_eq!(
            viewer.check_command(&insert, CommandSource::User),
            Some(CommandOutcome::RejectedReadOnly)
        );
        assert_eq!(viewer.check_command(&select_all, CommandSource::User), None);
        assert_eq!(viewer.check_command(&open_link, CommandSource::User), None);
        assert_eq!(
            viewer.check_command(&toggle_heading, CommandSource::User),
            None
        );

        let preview = InteractionPolicy::static_preview();
        assert_eq!(
            preview.check_command(&insert, CommandSource::User),
            Some(CommandOutcome::RejectedReadOnly)
        );
        assert_eq!(
            preview.check_command(&select_all, CommandSource::User),
            Some(CommandOutcome::RejectedByPolicy)
        );
        assert_eq!(
            preview.check_command(&open_link, CommandSource::User),
            Some(CommandOutcome::RejectedByPolicy)
        );
        assert_eq!(
            preview.check_command(&toggle_heading, CommandSource::User),
            Some(CommandOutcome::RejectedByPolicy)
        );
        assert_eq!(
            preview.check_command(&RichCommand::ClearSelection, CommandSource::User),
            None
        );

        // Programmatic commands bypass user interaction policy gates.
        for command in [&insert, &select_all, &open_link, &toggle_heading] {
            assert_eq!(
                preview.check_command(command, CommandSource::Programmatic),
                None
            );
        }

        let editable = InteractionPolicy::editable();
        assert!(editable.should_show_caret());
        assert_eq!(editable.check_command(&insert, CommandSource::User), None);
    }

    #[test]
    fn container_aware_split_merge_and_coalesced_history() {
        let mut doc = Document::new();
        let quote_child_id = doc.alloc_block_id();
        let quote_child = BlockNode::new(
            quote_child_id,
            BlockKind::Paragraph(InlineContent::plain("Alpha Beta")),
            BlockStyle::default(),
        );
        let quote_id = doc.add_blockquote(vec![quote_child]);
        let initial_quote_version = doc.find_block(quote_id).unwrap().version;

        let mut history = History::default();
        let split_pos = doc.doc_pos_for_block(quote_child_id, 6).unwrap();
        let split_tx = doc
            .build_split_block_transaction(
                &split_pos,
                Some(DocumentSelection::caret(split_pos.clone(), Affinity::Downstream)),
            )
            .unwrap();
        let applied_split = doc.apply(split_tx);
        history.push_inverse(applied_split.inverse);

        // Parent blockquote version must bump so LayoutCache invalidates container layout.
        assert!(doc.find_block(quote_id).unwrap().version > initial_quote_version);
        assert!(applied_split.change_set.dirty_blocks.contains(&quote_id));

        let children = doc
            .container_blocks(ContainerTarget::Blockquote(quote_id))
            .unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind.inline_content().unwrap().text, "Alpha ");
        assert_eq!(children[1].kind.inline_content().unwrap().text, "Beta");
        let second_child_id = children[1].id;

        // Merge second child back into first child inside the same blockquote.
        let merge_pos = doc.doc_pos_for_block(second_child_id, 0).unwrap();
        let merge_tx = doc
            .build_merge_with_previous_transaction(
                &merge_pos,
                Some(DocumentSelection::caret(merge_pos.clone(), Affinity::Downstream)),
            )
            .unwrap();
        let applied_merge = doc.apply(merge_tx);
        history.push_inverse(applied_merge.inverse);
        assert_eq!(
            doc.container_blocks(ContainerTarget::Blockquote(quote_id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            doc.find_block(quote_child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "Alpha Beta"
        );

        // Undo merge then undo split restores single original paragraph.
        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.container_blocks(ContainerTarget::Blockquote(quote_id))
                .unwrap()
                .len(),
            2
        );
        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.container_blocks(ContainerTarget::Blockquote(quote_id))
                .unwrap()
                .len(),
            1
        );

        // Verify typing coalescing within window groups sequential insertions into one undo entry.
        let base_time = std::time::Instant::now();
        let mut typing_history = History::default();
        for (index, character) in ['!', '?', '.'].into_iter().enumerate() {
            let offset = 10 + index;
            let before_pos = doc.doc_pos_for_block(quote_child_id, offset).unwrap();
            let after_pos = doc.doc_pos_for_block(quote_child_id, offset + 1).unwrap();
            let tx = Transaction::new(vec![Step::InsertText {
                block_id: quote_child_id,
                offset,
                text: character.to_string(),
                style: None,
            }])
            .with_selections(
                Some(DocumentSelection::caret(before_pos, Affinity::Downstream)),
                Some(DocumentSelection::caret(after_pos, Affinity::Downstream)),
            );
            let applied = doc.apply(tx);
            typing_history.push_inverse_grouped_at(
                applied.inverse,
                HistoryMergeGroup::Typing {
                    block_id: quote_child_id,
                },
                base_time + std::time::Duration::from_millis((index as u64) * 80),
            );
        }
        assert_eq!(typing_history.undo_len(), 1);
        assert_eq!(
            doc.find_block(quote_child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "Alpha Beta!?."
        );
        typing_history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.find_block(quote_child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "Alpha Beta"
        );
        typing_history.redo(&mut doc).unwrap();
        assert_eq!(
            doc.find_block(quote_child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "Alpha Beta!?."
        );
    }
}
