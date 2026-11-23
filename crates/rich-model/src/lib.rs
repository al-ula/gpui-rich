pub mod anchors;
pub mod block;
pub mod clipboard;
pub mod components;
pub mod codec;
pub mod command;
pub mod decoration;
pub mod grid_edit;
pub mod inline;
pub mod navigation;
pub mod payload;
pub mod selection;
pub mod style;
pub mod structure_edit;
pub mod sync;
pub mod transaction;

pub use anchors::*;
pub use block::*;
pub use clipboard::*;
pub use components::*;
pub use codec::*;
pub use command::*;
pub use decoration::*;
pub use grid_edit::*;
pub use inline::*;
pub use navigation::*;
pub use payload::*;
pub use selection::*;
pub use style::*;
pub use structure_edit::*;
pub use sync::*;
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
    fn interaction_policy_masks_children_without_overwriting_siblings_or_child_choices() {
        let mut policy = InteractionPolicy::editable();
        policy.editing.formatting.rich_styles = false;
        policy.editing.tables.column_resize = false;
        policy.reading.links = false;

        let bold = RichCommand::ToggleMark(MarkKind::Bold);
        let underline = RichCommand::ToggleMark(MarkKind::Underline);
        let resize_columns = RichCommand::SetTableColumns {
            table_id: BlockId(1),
            columns: vec![ColumnSpec::Auto],
        };
        let add_row = RichCommand::InsertTableRow { after: true };
        let copy_code = RichCommand::CopyCodeBlock(BlockId(2));
        let open_link = RichCommand::OpenLink("doc://spec".into());
        let insert = RichCommand::InsertText("hello".into());
        let rejected = Some(CommandOutcome::RejectedByPolicy);

        policy.editing.formatting.enabled = false;
        policy.editing.tables.enabled = false;
        policy.reading.enabled = false;
        for command in [&bold, &add_row, &copy_code] {
            assert_eq!(policy.check_command(command, CommandSource::User), rejected);
            assert_eq!(policy.check_command(command, CommandSource::Programmatic), None);
        }
        assert_eq!(policy.check_command(&insert, CommandSource::User), None);

        policy.editing.formatting.enabled = true;
        policy.editing.tables.enabled = true;
        policy.reading.enabled = true;
        for command in [&bold, &add_row, &copy_code] {
            assert_eq!(policy.check_command(command, CommandSource::User), None);
        }
        for command in [&underline, &resize_columns, &open_link] {
            assert_eq!(policy.check_command(command, CommandSource::User), rejected);
        }

        policy.editing.enabled = false;
        assert_eq!(
            policy.check_command(&bold, CommandSource::User),
            Some(CommandOutcome::RejectedReadOnly)
        );
        assert_eq!(policy.check_command(&copy_code, CommandSource::User), None);
    }

    #[test]
    fn container_aware_split_merge_and_coalesced_history() {
        let mut doc = Document::new();
        let quote_child_id = doc.alloc_block_id();
        let quote_child = BlockNode::new(
            quote_child_id,
            BlockKind::paragraph(InlineContent::plain("Alpha Beta")),
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
            .container_blocks(ContainerTarget::Container(quote_id))
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
            doc.container_blocks(ContainerTarget::Container(quote_id))
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
            doc.container_blocks(ContainerTarget::Container(quote_id))
                .unwrap()
                .len(),
            2
        );
        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.container_blocks(ContainerTarget::Container(quote_id))
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

    #[test]
    fn mark_toggling_and_rich_html_clipboard_roundtrip() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct BadgeData {
            text: String,
        }
        struct BadgeSpec;
        impl InlineSpec for BadgeSpec {
            const KIND: &'static str = "badge";
            type Data = BadgeData;
        }

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct CalloutLeafData {
            tone: String,
        }
        struct CalloutLeafSpec;
        impl BlockSpec for CalloutLeafSpec {
            const KIND: &'static str = "callout";
            type Shape = block_shape::Leaf;
            type Data = CalloutLeafData;
        }

        #[derive(Debug)]
        struct InsertCalloutCommand;
        impl ExtensionCommand for InsertCalloutCommand {
            const ID: &'static str = "app.insert_callout";
            type Effect = Edit;
        }

        #[derive(Debug)]
        struct InspectCalloutCommand;
        impl ExtensionCommand for InspectCalloutCommand {
            const ID: &'static str = "app.inspect_callout";
            type Effect = ReadOnly;
        }

        let mut document = Document::new();
        let heading_id = document.add_heading(2, InlineContent::plain("Overview"));
        let mut body = InlineContent::empty();
        body.push_text("Hello ", InlineStyle::default());
        body.push_text("bold", InlineStyle::bold().with_underline(UnderlineKind::Solid));
        body.push_text(" ", InlineStyle::default());
        body.push_text("link", InlineStyle::default().with_link("https://gpui.rs"));
        body.push_object(
            InlineObject::custom::<BadgeSpec>(BadgeData { text: "v2".into() }),
            InlineStyle::default(),
        );
        let paragraph_id = document.add_paragraph(body, BlockStyle::default().with_align(TextAlign::Center));
        let custom_block_id = document.add_leaf_block::<CalloutLeafSpec>(CalloutLeafData {
            tone: "info".into(),
        });

        // Verify clearing underline via mapped_spans_in_range + RestoreSpans is reversible.
        let base_style = document.config.default_style.clone();
        let updated_spans = document
            .find_block(paragraph_id)
            .unwrap()
            .kind
            .inline_content()
            .unwrap()
            .mapped_spans_in_range(6..10, |style| {
                style.set_mark(MarkKind::Underline, false, &base_style);
            });
        let applied = document.apply(Transaction::new(vec![Step::RestoreSpans {
            block_id: paragraph_id,
            spans: updated_spans,
        }]));
        assert!(
            !document
                .find_block(paragraph_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .style_at(7)
                .is_mark_active(MarkKind::Underline, &base_style)
        );
        document.apply(applied.inverse);
        assert!(
            document
                .find_block(paragraph_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .style_at(7)
                .is_mark_active(MarkKind::Underline, &base_style)
        );

        // Extract cross-block rich selection and round-trip through normalized HTML.
        let start = document.doc_pos_for_block(heading_id, 0).unwrap();
        let end_len = document
            .find_block(paragraph_id)
            .unwrap()
            .kind
            .inline_content()
            .unwrap()
            .len();
        let end = document.doc_pos_for_block(paragraph_id, end_len).unwrap();
        let selection = DocumentSelection::Text {
            anchor: start,
            focus: end,
            affinity: Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = document.extract_selection_fragment(&selection).unwrap();
        let html = fragment.to_html();
        let reparsed = ClipboardFragment::from_html(&html);
        assert_eq!(reparsed.blocks.len(), 2);
        assert!(matches!(reparsed.blocks[0], ClipboardBlock::Heading { level: 2, .. }));
        if let ClipboardBlock::Paragraph { content, align } = &reparsed.blocks[1] {
            assert_eq!(*align, TextAlign::Center);
            assert!(content.style_at(7).is_mark_active(MarkKind::Bold, &base_style));
            assert!(content.style_at(7).is_mark_active(MarkKind::Underline, &base_style));
            assert_eq!(
                content.style_at(12).link.as_ref().map(|l| l.href.as_str()),
                Some("https://gpui.rs")
            );
            assert_eq!(
                content
                    .objects
                    .first()
                    .unwrap()
                    .object
                    .read_data::<BadgeSpec>()
                    .unwrap()
                    .text,
                "v2"
            );
        } else {
            panic!("expected paragraph block after HTML roundtrip");
        }

        // Verify unsafe script tags and javascript: links are stripped during HTML normalization.
        let hostile = ClipboardFragment::from_html(
            "<p>Safe<script>alert(1)</script><a href=\"javascript:evil()\">click</a></p>",
        );
        assert_eq!(hostile.to_plain_text(), "Safeclick");
        if let ClipboardBlock::Paragraph { content, .. } = &hostile.blocks[0] {
            assert!(content.style_at(5).link.is_none());
        } else {
            panic!("expected sanitized paragraph");
        }

        // Verify custom block serialization and typed extension command policy gating.
        assert!(document.find_block(custom_block_id).is_some());
        let json = document.to_json_pretty().unwrap();
        let parsed = Document::from_json(&json).unwrap();
        assert_eq!(document, parsed);

        let mutating_custom = RichCommand::extension(InsertCalloutCommand);
        let readonly_custom = RichCommand::extension(InspectCalloutCommand);
        assert_eq!(mutating_custom.command_id(), "app.insert_callout");
        let viewer = InteractionPolicy::viewer();
        assert_eq!(
            viewer.check_command(&mutating_custom, CommandSource::User),
            Some(CommandOutcome::RejectedReadOnly)
        );
        assert_eq!(viewer.check_command(&readonly_custom, CommandSource::User), None);
    }

    #[test]
    fn font_family_html_roundtrip_and_named_style_update_undo() {
        let mut document = Document::new();
        let heading_id = document.add_heading(1, InlineContent::plain("Styled Heading"));
        let mut body = InlineContent::empty();
        body.push_text(
            "Serif quote",
            InlineStyle::default()
                .with_font_family("Georgia")
                .with_font_size(14.0)
                .with_italic(true),
        );
        let para_id = document.add_paragraph(body, BlockStyle::default());

        // Verify CSS font-family survives HTML clipboard round-trip.
        let start = document.doc_pos_for_block(para_id, 0).unwrap();
        let end = document.doc_pos_for_block(para_id, 11).unwrap();
        let selection = DocumentSelection::Text {
            anchor: start,
            focus: end,
            affinity: Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let html = document
            .extract_selection_fragment(&selection)
            .unwrap()
            .to_html();
        assert!(html.contains("font-family:'Georgia'"));
        let reparsed = ClipboardFragment::from_html(&html);
        if let ClipboardBlock::Paragraph { content, .. } = &reparsed.blocks[0] {
            assert_eq!(content.style_at(0).font_family.as_deref(), Some("Georgia"));
            assert_eq!(content.style_at(0).font_size_pt, Some(14.0));
        } else {
            panic!("expected paragraph block");
        }

        // Verify UpdateNamedStyle bumps dependent block versions and reverses cleanly on Undo.
        let initial_version = document.find_block(heading_id).unwrap().version;
        let mut updated_heading = document.stylesheet.get(StyleId::HEADING_1).unwrap().clone();
        updated_heading.text.font_family = Some(std::sync::Arc::from("serif"));
        updated_heading.text.font_size_pt = Some(24.0);

        let mut history = History::default();
        let applied = document.apply(Transaction::new(vec![Step::UpdateNamedStyle {
            style_id: StyleId::HEADING_1,
            style: Some(updated_heading),
        }]));
        history.push_inverse(applied.inverse);

        assert!(applied.change_set.dirty_blocks.contains(&heading_id));
        assert!(document.find_block(heading_id).unwrap().version > initial_version);
        assert_eq!(
            document
                .stylesheet
                .get(StyleId::HEADING_1)
                .unwrap()
                .text
                .font_family
                .as_deref(),
            Some("serif")
        );

        history.undo(&mut document).unwrap();
        assert_eq!(
            document
                .stylesheet
                .get(StyleId::HEADING_1)
                .unwrap()
                .text
                .font_family,
            None
        );
        assert_eq!(
            document
                .stylesheet
                .get(StyleId::HEADING_1)
                .unwrap()
                .text
                .font_size_pt,
            Some(20.0)
        );
    }

    #[test]
    fn table_editing_merge_split_row_col_and_undo_roundtrip() {
        let mut doc = Document::new();
        let table_id = doc.alloc_block_id();
        let mut rows = Vec::new();
        for r in 0..2 {
            let mut cells = Vec::new();
            for c in 0..2 {
                let cid = doc.alloc_block_id();
                let pid = doc.alloc_block_id();
                cells.push(TableCell::simple(
                    cid,
                    vec![BlockNode::new(
                        pid,
                        BlockKind::paragraph(InlineContent::plain(format!("R{r}C{c}"))),
                        BlockStyle::default(),
                    )],
                ));
            }
            rows.push(TableRow {
                is_header: r == 0,
                cells,
            });
        }
        doc.push_block(BlockNode::new(
            table_id,
            BlockKind::table(TableBlock {
                columns: vec![ColumnSpec::Auto; 2],
                rows,
                column_kinds: Default::default(),
                border_style: TableBorderStyle::default(),
            }),
            BlockStyle::default(),
        ));

        let mut history = History::default();

        // Merge top row cells (0,0)..(0,1).
        let merge_tx = doc
            .build_merge_table_cells_transaction(table_id, (0, 0), (0, 1), None)
            .unwrap();
        let applied = doc.apply(merge_tx);
        history.push_inverse(applied.inverse);
        let merged = doc.find_block(table_id).unwrap().kind.table_block().unwrap();
        assert_eq!(merged.rows[0].cells.len(), 1);
        assert_eq!(merged.rows[0].cells[0].colspan, 2);
        assert_eq!(merged.rows[0].cells[0].blocks.len(), 2);

        // Insert a column after visual col 0 (splits across the colspan=2 header cell!).
        let insert_col_tx = doc
            .build_insert_table_column_transaction(table_id, 0, true, None)
            .unwrap();
        let applied = doc.apply(insert_col_tx);
        history.push_inverse(applied.inverse);
        let after_col = doc.find_block(table_id).unwrap().kind.table_block().unwrap();
        assert_eq!(after_col.grid_col_count(), 3);
        assert_eq!(after_col.rows[0].cells[0].colspan, 3);
        assert_eq!(after_col.rows[1].cells.len(), 3);

        // Split the merged header cell back into 3 1x1 cells.
        let split_tx = doc
            .build_split_table_cell_transaction(table_id, 0, 0, None)
            .unwrap();
        let applied = doc.apply(split_tx);
        history.push_inverse(applied.inverse);
        let after_split = doc.find_block(table_id).unwrap().kind.table_block().unwrap();
        assert_eq!(after_split.rows[0].cells.len(), 3);
        assert_eq!(after_split.rows[0].cells[0].colspan, 1);

        // Insert row below and delete it.
        let insert_row_tx = doc
            .build_insert_table_row_transaction(table_id, 1, true, None)
            .unwrap();
        let applied = doc.apply(insert_row_tx);
        history.push_inverse(applied.inverse);
        let after_row = doc.find_block(table_id).unwrap().kind.table_block().unwrap();
        assert_eq!(after_row.rows.len(), 3);

        // Undo all 4 operations restores exact 2x2 table.
        for _ in 0..4 {
            history.undo(&mut doc).unwrap();
        }
        let restored = doc.find_block(table_id).unwrap().kind.table_block().unwrap();
        assert_eq!(restored.rows.len(), 2);
        assert_eq!(restored.rows[0].cells.len(), 2);
        assert_eq!(restored.rows[0].cells[0].colspan, 1);
    }

    #[test]
    fn checklist_toggle_and_inline_image_resize_undo_roundtrip() {
        let mut doc = Document::new();
        let task_id = doc.add_list_item(
            ListId(1),
            ListKind::Checklist,
            0,
            Some(false),
            InlineContent::plain("Ship Phase 3"),
        );
        let mut para = InlineContent::empty();
        let img_id = para.push_object(
            InlineObject::Image {
                asset_id: "hero.png".into(),
                alt: "Hero".into(),
                width_pt: 200.0,
                height_pt: 120.0,
                valign: InlineVAlign::Baseline,
            },
            InlineStyle::default(),
        );
        let para_id = doc.add_paragraph(para, BlockStyle::default());

        let mut history = History::default();
        let toggle = doc.apply(Transaction::new(vec![Step::ToggleChecklist {
            block_id: task_id,
        }]));
        history.push_inverse(toggle.inverse);
        assert_eq!(
            doc.find_block(task_id)
                .unwrap()
                .kind
                .list_item_data()
                .and_then(|item| item.checked),
            Some(true)
        );

        let resize = doc.apply(Transaction::new(vec![Step::UpdateInlineObject {
            block_id: para_id,
            object_id: img_id,
            object: InlineObject::Image {
                asset_id: "hero.png".into(),
                alt: "Hero".into(),
                width_pt: 320.0,
                height_pt: 192.0,
                valign: InlineVAlign::Baseline,
            },
        }]));
        history.push_inverse(resize.inverse);
        assert!(matches!(
            doc.find_block(para_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .object_by_id(img_id)
                .unwrap()
                .object,
            InlineObject::Image {
                width_pt,
                height_pt,
                ..
            } if (width_pt - 320.0).abs() < 0.01 && (height_pt - 192.0).abs() < 0.01
        ));

        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.find_block(task_id)
                .unwrap()
                .kind
                .list_item_data()
                .and_then(|item| item.checked),
            Some(false)
        );
        assert!(matches!(
            doc.find_block(para_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .object_by_id(img_id)
                .unwrap()
                .object,
            InlineObject::Image { width_pt, .. } if (width_pt - 200.0).abs() < 0.01
        ));
    }

    #[test]
    fn document_codec_markdown_and_html_roundtrip() {
        let registry = CodecRegistry::with_builtin_codecs();
        assert!(registry.get_by_extension(&FileExtension::new("md")).is_ok());
        assert!(registry.get_by_mime(&MimeType::new("text/html")).is_ok());

        let source_md = "\
# Phase 3 Spec

Hello **bold**, *italic*, `code`, and [docs](https://gpui.rs).

- [x] Checklist done
- [ ] Checklist open

> Quoted insight

```rust
fn main() {}
```

| Metric | Value |
| --- | ---: |
| FPS | 120 |

---";
        let markdown_id = FormatId::new("markdown");
        let html_id = FormatId::new("html");
        let decoded = registry.decode_as(&markdown_id, source_md).unwrap();
        assert_eq!(decoded.blocks.len(), 8);
        assert_eq!(decoded.blocks[0].kind.heading_data().map(|h| h.level), Some(1));
        assert!(matches!(
            decoded.blocks[2].kind.list_item_data(),
            Some(ListItemData {
                kind: ListKind::Checklist,
                checked: Some(true),
                ..
            })
        ));
        assert!(decoded.blocks[4].kind.is_blockquote());
        assert_eq!(
            decoded.blocks[5]
                .kind
                .code_block_data()
                .and_then(|code| code.language.clone())
                .as_deref(),
            Some("rust")
        );
        assert!(decoded.blocks[6].kind.is_builtin_table());
        assert!(decoded.blocks[7].kind.is_horizontal_rule());

        let reencoded_md = registry.encode_as(&markdown_id, &decoded).unwrap();
        let reparsed_md = registry.decode_as(&markdown_id, &reencoded_md).unwrap();
        assert_eq!(reparsed_md.blocks.len(), decoded.blocks.len());

        let html = registry.encode_as(&html_id, &decoded).unwrap();
        let reparsed_html = registry.decode_as(&html_id, &html).unwrap();
        assert_eq!(reparsed_html.blocks.len(), decoded.blocks.len());
    }

    #[test]
    fn crdt_sync_bridge_converges_out_of_order_edits() {
        let mut base = Document::new();
        let p1 = base.add_paragraph(InlineContent::plain("Shared"), BlockStyle::default());

        let mut doc_a = base.clone();
        let mut doc_b = base;
        let mut bridge_a = CrdtSyncBridge::new(ReplicaId(1), &doc_a);
        let mut bridge_b = CrdtSyncBridge::new(ReplicaId(2), &doc_b);

        // Replica A edits p1 and appends block 100.
        let new_a = BlockNode::new(
            BlockId(100),
            BlockKind::paragraph(InlineContent::plain("From A")),
            BlockStyle::default(),
        );
        let applied_a = doc_a.apply(Transaction::new(vec![
            Step::InsertText {
                block_id: p1,
                offset: 6,
                text: " A".into(),
                style: None,
            },
            Step::InsertBlock {
                index: 1,
                block: new_a,
            },
        ]));
        bridge_a.after_transaction(&applied_a.change_set, &doc_a, None);
        let deltas_from_a = bridge_a.drain_outbox();

        // Replica B concurrently edits p1 and appends block 200.
        let new_b = BlockNode::new(
            BlockId(200),
            BlockKind::paragraph(InlineContent::plain("From B")),
            BlockStyle::default(),
        );
        let applied_b = doc_b.apply(Transaction::new(vec![
            Step::InsertText {
                block_id: p1,
                offset: 6,
                text: " B".into(),
                style: None,
            },
            Step::InsertBlock {
                index: 1,
                block: new_b,
            },
        ]));
        bridge_b.after_transaction(&applied_b.change_set, &doc_b, None);
        let deltas_from_b = bridge_b.drain_outbox();

        // Exchange deltas in opposite orders; both replicas must converge identically.
        for delta in &deltas_from_b {
            let serialized = delta.to_json().unwrap();
            let parsed = SyncDelta::from_json(&serialized).unwrap();
            if let Some(tx) = bridge_a.apply_remote_delta(&parsed, &doc_a) {
                bridge_a.set_applying_remote(true);
                let applied = doc_a.apply(tx);
                bridge_a.after_transaction(&applied.change_set, &doc_a, None);
                bridge_a.set_applying_remote(false);
            }
        }
        for delta in &deltas_from_a {
            if let Some(tx) = bridge_b.apply_remote_delta(delta, &doc_b) {
                bridge_b.set_applying_remote(true);
                let applied = doc_b.apply(tx);
                bridge_b.after_transaction(&applied.change_set, &doc_b, None);
                bridge_b.set_applying_remote(false);
            }
        }

        // Neither replica re-broadcasts remote changes (echo suppression).
        assert!(bridge_a.drain_outbox().is_empty());
        assert!(bridge_b.drain_outbox().is_empty());

        let texts_a: Vec<&str> = doc_a
            .blocks
            .iter()
            .filter_map(|block| block.kind.inline_content().map(|c| c.text.as_str()))
            .collect();
        let texts_b: Vec<&str> = doc_b
            .blocks
            .iter()
            .filter_map(|block| block.kind.inline_content().map(|c| c.text.as_str()))
            .collect();
        assert_eq!(texts_a, texts_b);
        assert_eq!(texts_a.len(), 3);
    }

    #[test]
    fn custom_payload_typed_raw_roundtrip_and_custom_container_editing() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct CalloutMeta {
            title: String,
            tone: String,
        }
        struct CalloutSpec;
        impl BlockSpec for CalloutSpec {
            const KIND: &'static str = "callout";
            type Shape = block_shape::Container;
            type Data = CalloutMeta;
        }

        let meta = CalloutMeta {
            title: "Note".into(),
            tone: "info".into(),
        };
        let mut doc = Document::new();
        let child_id = doc.alloc_block_id();
        let child = BlockNode::new(
            child_id,
            BlockKind::paragraph(InlineContent::plain("First Second")),
            BlockStyle::default(),
        );
        let container_id = doc.add_container_block::<CalloutSpec>(meta.clone(), vec![child]);
        assert_eq!(
            doc.find_block(container_id)
                .unwrap()
                .kind
                .data()
                .downcast_ref::<CalloutMeta>(),
            Some(&meta)
        );

        // JSON round-trip turns Typed payload into Raw while preserving Document equality and lazy decoding.
        let json = doc.to_json_pretty().unwrap();
        let parsed = Document::from_json(&json).unwrap();
        assert_eq!(doc, parsed);
        let raw_payload = parsed.find_block(container_id).unwrap().kind.data();
        assert!(!raw_payload.is_typed());
        assert_eq!(
            parsed
                .find_block(container_id)
                .unwrap()
                .read_data::<CalloutSpec>()
                .unwrap()
                .as_ref(),
            &meta
        );

        // Splitting a paragraph inside a custom Container updates container children and bumps ancestor version.
        let before_version = doc.find_block(container_id).unwrap().version;
        let split_pos = doc.doc_pos_for_block(child_id, 6).unwrap();
        let split_tx = doc
            .build_split_block_transaction(&split_pos, None)
            .unwrap();
        let applied = doc.apply(split_tx);
        assert!(doc.find_block(container_id).unwrap().version > before_version);
        assert!(applied.change_set.dirty_blocks.contains(&container_id));
        let children = doc
            .container_blocks(ContainerTarget::Container(container_id))
            .unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind.inline_content().unwrap().text, "First ");
        assert_eq!(children[1].kind.inline_content().unwrap().text, "Second");
    }

    #[test]
    fn custom_node_json_fidelity_error_classification_and_fallible_serialization() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct CalloutData {
            title: String,
        }
        struct CalloutSpec;
        impl BlockSpec for CalloutSpec {
            const KIND: &'static str = "callout";
            type Shape = block_shape::Container;
            type Data = CalloutData;
        }
        struct CalloutLeafSpec;
        impl BlockSpec for CalloutLeafSpec {
            const KIND: &'static str = "callout";
            type Shape = block_shape::Leaf;
            type Data = CalloutData;
        }

        // Unknown custom kind + unknown JSON fields + image ID string round-trip faithfully.
        let mut original = Document::new();
        let mut inline = InlineContent::empty();
        inline.push_object(
            InlineObject::Image {
                asset_id: ImageAssetId::new("asset://hero/1"),
                alt: "Hero".into(),
                width_pt: 80.0,
                height_pt: 40.0,
                valign: InlineVAlign::Baseline,
            },
            InlineStyle::default(),
        );
        original.add_paragraph(inline, BlockStyle::default());
        let unknown_id = original.alloc_block_id();
        original.push_block(BlockNode::new(
            unknown_id,
            BlockKind::Leaf {
                kind: std::sync::Arc::from("vendor.unknown_widget"),
                data: CustomPayload::from_json(serde_json::json!({ "extra": [1, 2, 3] })),
            },
            BlockStyle::default(),
        ));
        let json = original.to_json_pretty().unwrap();
        assert!(json.contains("\"asset_id\": \"asset://hero/1\""));
        let reparsed = Document::from_json(&json).unwrap();
        assert_eq!(reparsed, original);
        assert_eq!(
            reparsed
                .find_block(unknown_id)
                .unwrap()
                .kind
                .data()
                .to_json()
                .unwrap(),
            serde_json::json!({ "extra": [1, 2, 3] })
        );

        // Distinguish wrong kind, wrong shape, and malformed payload on checked reads.
        let corrupt_callout = BlockNode::new(
            BlockId(99),
            BlockKind::Container {
                kind: std::sync::Arc::from("callout"),
                data: CustomPayload::from_json(serde_json::json!({ "title": 123 })),
                children: Vec::new(),
            },
            BlockStyle::default(),
        );
        assert!(matches!(
            corrupt_callout.read_data::<CalloutSpec>(),
            Err(NodeDataError::InvalidData { kind: "callout", .. })
        ));
        assert!(matches!(
            corrupt_callout.read_data::<CalloutLeafSpec>(),
            Err(NodeDataError::ShapeMismatch {
                kind: "callout",
                expected: BlockShapeKind::Leaf,
                actual: BlockShapeKind::Container,
            })
        ));
        assert!(matches!(
            reparsed.find_block(unknown_id).unwrap().read_data::<CalloutSpec>(),
            Err(NodeDataError::KindMismatch { expected: "callout", .. })
        ));

        // Typed payload serialization failure propagates an error instead of writing null.
        #[derive(Clone, Debug, PartialEq)]
        struct FailingPayload;
        impl serde::Serialize for FailingPayload {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("cannot serialize"))
            }
        }
        impl<'de> serde::Deserialize<'de> for FailingPayload {
            fn deserialize<D: serde::Deserializer<'de>>(
                _deserializer: D,
            ) -> Result<Self, D::Error> {
                Ok(Self)
            }
        }
        struct FailingSpec;
        impl BlockSpec for FailingSpec {
            const KIND: &'static str = "failing";
            type Shape = block_shape::Leaf;
            type Data = FailingPayload;
        }
        let mut failing_doc = Document::new();
        failing_doc.add_leaf_block::<FailingSpec>(FailingPayload);
        assert!(failing_doc.to_json_pretty().is_err());
        assert!(matches!(
            JsonCodec.encode(&failing_doc),
            Err(CodecError::Serialize { .. })
        ));
        assert!(matches!(
            MarkdownCodec.encode(&failing_doc),
            Err(CodecError::Serialize { .. })
        ));
    }

    #[test]
    fn nested_payload_edits_preserve_identity_update_ancestors_reject_stale_and_undo() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct CardData {
            title: String,
        }
        struct CardSpec;
        impl BlockSpec for CardSpec {
            const KIND: &'static str = "card";
            type Shape = block_shape::Container;
            type Data = CardData;
        }
        struct OtherLeafSpec;
        impl BlockSpec for OtherLeafSpec {
            const KIND: &'static str = "other";
            type Shape = block_shape::Leaf;
            type Data = CardData;
        }

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct StatusData {
            label: String,
        }
        struct StatusSpec;
        impl InlineSpec for StatusSpec {
            const KIND: &'static str = "status";
            type Data = StatusData;
        }
        struct OtherInlineSpec;
        impl InlineSpec for OtherInlineSpec {
            const KIND: &'static str = "other_inline";
            type Data = StatusData;
        }

        let mut doc = Document::new();
        let mut leaf_inline = InlineContent::plain("Status: ");
        let badge_id = leaf_inline.push_object(
            InlineObject::custom::<StatusSpec>(StatusData {
                label: "Draft".into(),
            }),
            InlineStyle::default(),
        );
        let child_id = doc.alloc_block_id();
        let child = BlockNode::new(child_id, BlockKind::paragraph(leaf_inline), BlockStyle::default());
        let inner_card_id = doc.alloc_block_id();
        let inner_card = BlockNode::new(
            inner_card_id,
            BlockKind::container::<CardSpec>(CardData { title: "Inner v1".into() }, vec![child]),
            BlockStyle::default(),
        );
        let outer_quote_id = doc.add_blockquote(vec![inner_card]);

        let outer_v0 = doc.find_block(outer_quote_id).unwrap().version;
        let inner_v0 = doc.find_block(inner_card_id).unwrap().version;
        let mut history = History::default();

        // Payload-only block edit preserves children and block ID, bumps ancestors, is non-structural.
        let applied = doc.apply(Transaction::new(vec![Step::update_block_data::<CardSpec>(
            inner_card_id,
            CardData { title: "Inner v2".into() },
        )]));
        history.push_inverse(applied.inverse);
        assert!(!applied.change_set.structure_changed);
        assert!(applied.change_set.dirty_blocks.contains(&inner_card_id));
        assert!(applied.change_set.dirty_blocks.contains(&outer_quote_id));
        assert!(doc.find_block(inner_card_id).unwrap().version > inner_v0);
        assert!(doc.find_block(outer_quote_id).unwrap().version > outer_v0);
        assert_eq!(
            doc.find_block(inner_card_id).unwrap().kind.children().unwrap()[0].id,
            child_id
        );
        assert_eq!(
            doc.find_block(inner_card_id).unwrap().read_data::<CardSpec>().unwrap().title,
            "Inner v2"
        );

        // Stale wrong-kind block update is rejected without dirtying or changing data.
        let stale_block = doc.apply(Transaction::new(vec![Step::update_block_data::<OtherLeafSpec>(
            inner_card_id,
            CardData { title: "Corrupted".into() },
        )]));
        assert!(stale_block.change_set.dirty_blocks.is_empty());
        assert_eq!(
            doc.find_block(inner_card_id).unwrap().read_data::<CardSpec>().unwrap().title,
            "Inner v2"
        );

        // Checked inline payload update preserves object ID and bumps ancestors.
        let inline_edit = doc.apply(Transaction::new(vec![Step::update_inline_data::<StatusSpec>(
            child_id,
            badge_id,
            StatusData { label: "Done".into() },
        )]));
        history.push_inverse(inline_edit.inverse);
        assert!(!inline_edit.change_set.structure_changed);
        assert!(inline_edit.change_set.dirty_blocks.contains(&child_id));
        assert!(inline_edit.change_set.dirty_blocks.contains(&inner_card_id));
        assert!(inline_edit.change_set.dirty_blocks.contains(&outer_quote_id));

        // Stale wrong-kind inline update does not overwrite the badge.
        let stale_inline = doc.apply(Transaction::new(vec![Step::update_inline_data::<OtherInlineSpec>(
            child_id,
            badge_id,
            StatusData { label: "Wrong".into() },
        )]));
        assert!(stale_inline.change_set.dirty_blocks.is_empty());
        assert_eq!(
            doc.find_block(child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .object_by_id(badge_id)
                .unwrap()
                .object
                .read_data::<StatusSpec>()
                .unwrap()
                .label,
            "Done"
        );

        // Undo restores both inline and nested block payloads cleanly.
        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.find_block(inner_card_id).unwrap().read_data::<CardSpec>().unwrap().title,
            "Inner v1"
        );
        assert_eq!(
            doc.find_block(child_id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .object_by_id(badge_id)
                .unwrap()
                .object
                .read_data::<StatusSpec>()
                .unwrap()
                .label,
            "Draft"
        );
    }

    #[test]
    fn versioned_clipboard_preserves_custom_grid_open_edges_and_rejects_malformed() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct SheetMeta {
            sheet_name: String,
        }
        struct SheetGridSpec;
        impl BlockSpec for SheetGridSpec {
            const KIND: &'static str = "acme.sheet";
            type Shape = block_shape::Grid;
            type Data = SheetMeta;
        }

        let mut doc = Document::new();
        let before_id = doc.add_paragraph(InlineContent::plain("alpha beta"), BlockStyle::default());
        let cell_id = doc.alloc_block_id();
        let cell_para_id = doc.alloc_block_id();
        let table = TableBlock {
            columns: vec![ColumnSpec::Auto],
            column_kinds: Default::default(),
            rows: vec![TableRow {
                is_header: false,
                cells: vec![TableCell::simple(
                    cell_id,
                    vec![BlockNode::new(
                        cell_para_id,
                        BlockKind::paragraph(InlineContent::plain("42")),
                        BlockStyle::default(),
                    )],
                )],
            }],
            border_style: TableBorderStyle::default(),
        };
        doc.add_grid_block::<SheetGridSpec>(SheetMeta { sheet_name: "Q4".into() }, table);
        let after_id = doc.add_paragraph(InlineContent::plain("gamma delta"), BlockStyle::default());

        let selection = DocumentSelection::Text {
            anchor: doc.doc_pos_for_block(before_id, 6).unwrap(),
            focus: doc.doc_pos_for_block(after_id, 5).unwrap(),
            affinity: Affinity::Downstream,
            goal_x: None,
            stored_marks: None,
        };
        let fragment = doc.extract_selection_fragment(&selection).unwrap();
        assert!(fragment.start_open);
        assert!(fragment.end_open);
        assert!(matches!(
            &fragment.blocks[1],
            ClipboardBlock::CustomGrid { kind, .. } if kind.as_ref() == "acme.sheet"
        ));

        let encoded = fragment.to_versioned_json().unwrap();
        let decoded = ClipboardFragment::from_versioned_json(&encoded).unwrap();
        assert_eq!(decoded, fragment);

        // Destination materialization allocates fresh IDs and preserves custom grid data.
        let mut destination = Document::new();
        let nodes = decoded.to_block_nodes(&mut destination);
        assert_eq!(nodes[1].read_data::<SheetGridSpec>().unwrap().sheet_name, "Q4");

        // Unsupported version and broken UTF-8 span boundaries are rejected.
        let bad_version = encoded.replace("\"version\":1", "\"version\":99");
        assert!(matches!(
            ClipboardFragment::from_versioned_json(&bad_version),
            Err(ClipboardFragmentError::UnsupportedVersion { found: 99, expected: 1 })
        ));
        let valid_unicode = ClipboardFragment::from_blocks(vec![ClipboardBlock::Paragraph {
            content: InlineContent::plain("é"),
            align: TextAlign::Left,
        }])
        .to_versioned_json()
        .unwrap();
        let bad_utf8 = valid_unicode.replace("\"end\":2", "\"end\":1");
        assert!(matches!(
            ClipboardFragment::from_versioned_json(&bad_utf8),
            Err(ClipboardFragmentError::Malformed(_))
        ));
    }

    #[test]
    fn extension_command_wire_rejection_checked_decorations_and_ambiguous_codecs() {
        // Built-in commands round-trip; extension commands reject serialization; legacy Custom rejects deserialization.
        let builtin = RichCommand::InsertText("hi".into());
        let builtin_json = serde_json::to_string(&builtin).unwrap();
        assert_eq!(serde_json::from_str::<RichCommand>(&builtin_json).unwrap(), builtin);

        #[derive(Debug)]
        struct LocalCmd(#[allow(dead_code)] u32);
        impl ExtensionCommand for LocalCmd {
            const ID: &'static str = "acme.local";
            type Effect = ReadOnly;
        }
        assert!(serde_json::to_string(&RichCommand::extension(LocalCmd(7))).is_err());
        let legacy_custom = r#"{"Custom":{"id":"acme.old","payload":null,"mutates_document":true}}"#;
        assert!(serde_json::from_str::<RichCommand>(legacy_custom).is_err());

        // DecorationWriter rejects invalid UTF-8 boundaries, reversed ranges, and out-of-bounds offsets.
        let block = BlockNode::new(
            BlockId(1),
            BlockKind::paragraph(InlineContent::plain("aé")),
            BlockStyle::default(),
        );
        let mut writer = DecorationWriter::new(&block);
        assert_eq!(
            writer.push_inline(
                BlockByteRange::new(BlockByteOffset(0), BlockByteOffset(2)),
                InlineDecorationStyle::new(),
            ),
            Err(DecorationError::InvalidUtf8Boundary { offset: 2 })
        );
        assert_eq!(
            writer.push_inline(
                BlockByteRange::new(BlockByteOffset(3), BlockByteOffset(1)),
                InlineDecorationStyle::new(),
            ),
            Err(DecorationError::InvalidOrder { start: 3, end: 1 })
        );
        assert_eq!(
            writer.push_ghost_text(BlockByteOffset(99), Affinity::Downstream, "x", None),
            Err(DecorationError::OutOfBounds { offset: 99, len: 3 })
        );
        writer
            .push_inline(
                BlockByteRange::new(BlockByteOffset(1), BlockByteOffset(3)),
                InlineDecorationStyle::new().with_priority(5),
            )
            .unwrap();
        let decorations = writer.finish();
        assert_eq!(decorations.inline.len(), 1);
        assert_eq!(decorations.inline[0].range(), 1..3);

        // CodecRegistry rejects duplicate IDs and reports ambiguous MIME/suffix lookups.
        struct AltMarkdownFormat;
        struct AltMarkdownCodec;
        impl DocumentCodec for AltMarkdownCodec {
            type Format = AltMarkdownFormat;
            fn format_id(&self) -> &str { "alt-markdown" }
            fn mime_types(&self) -> &[&str] { &["text/markdown"] }
            fn file_extensions(&self) -> &[&str] { &["md"] }
            fn encode(&self, _doc: &Document) -> Result<EncodedDocument<Self::Format>, CodecError> {
                Ok(EncodedDocument::from_raw(""))
            }
            fn decode(&self, _input: &EncodedDocument<Self::Format>) -> Result<Document, CodecError> {
                Ok(Document::new())
            }
        }
        let mut registry = CodecRegistry::with_builtin_codecs();
        assert!(matches!(
            registry.register(MarkdownCodec),
            Err(CodecError::DuplicateFormatId { .. })
        ));
        registry.register(AltMarkdownCodec).unwrap();
        assert!(matches!(
            registry.get_by_mime(&MimeType::new("text/markdown")),
            Err(CodecError::AmbiguousMimeType { candidates, .. }) if candidates.len() == 2
        ));
        assert!(matches!(
            registry.get_by_extension(&FileExtension::new(".md")),
            Err(CodecError::AmbiguousFileExtension { candidates, .. }) if candidates.len() == 2
        ));
    }
}
