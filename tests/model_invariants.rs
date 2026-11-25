use gpui_rich::model::*;

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
    inline.push_object(
        InlineObject::FootnoteRef { index: 1 },
        InlineStyle::default(),
    );
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

    let text = &doc
        .find_block(p_id)
        .unwrap()
        .kind
        .inline_content()
        .unwrap()
        .text;
    assert!(text.starts_with("Hello world"));

    // Undo both transactions and verify full round-trip equality (excluding block version counter).
    history.undo(&mut doc).unwrap();
    history.undo(&mut doc).unwrap();

    let orig_inline = original
        .find_block(p_id)
        .unwrap()
        .kind
        .inline_content()
        .unwrap();
    let cur_inline = doc.find_block(p_id).unwrap().kind.inline_content().unwrap();
    assert_eq!(orig_inline, cur_inline);

    // Redo restores the edits cleanly.
    history.redo(&mut doc).unwrap();
    history.redo(&mut doc).unwrap();
    let cur_spans = &doc
        .find_block(p_id)
        .unwrap()
        .kind
        .inline_content()
        .unwrap()
        .spans;
    assert_eq!(cur_spans[0].range, 0..5);
    assert_eq!(cur_spans[0].style.weight, Some(FontWeight::BOLD));
}

#[test]
fn document_json_serialization_roundtrip() {
    let mut doc = Document::new();
    doc.add_heading(1, InlineContent::plain("Architecture"));
    doc.add_paragraph(
        InlineContent::plain("Non-paginated flow."),
        BlockStyle::default(),
    );

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
        assert_eq!(
            policy.check_command(command, CommandSource::Programmatic),
            None
        );
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
            Some(DocumentSelection::caret(
                split_pos.clone(),
                Affinity::Downstream,
            )),
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
            Some(DocumentSelection::caret(
                merge_pos.clone(),
                Affinity::Downstream,
            )),
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
    body.push_text(
        "bold",
        InlineStyle::bold().with_underline(UnderlineKind::Solid),
    );
    body.push_text(" ", InlineStyle::default());
    body.push_text("link", InlineStyle::default().with_link("https://gpui.rs"));
    body.push_object(
        InlineObject::custom::<BadgeSpec>(BadgeData { text: "v2".into() }),
        InlineStyle::default(),
    );
    let paragraph_id =
        document.add_paragraph(body, BlockStyle::default().with_align(TextAlign::Center));
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
    assert!(matches!(
        reparsed.blocks[0],
        ClipboardBlock::Heading { level: 2, .. }
    ));
    if let ClipboardBlock::Paragraph { content, align } = &reparsed.blocks[1] {
        assert_eq!(*align, TextAlign::Center);
        assert!(
            content
                .style_at(7)
                .is_mark_active(MarkKind::Bold, &base_style)
        );
        assert!(
            content
                .style_at(7)
                .is_mark_active(MarkKind::Underline, &base_style)
        );
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
    assert_eq!(
        viewer.check_command(&readonly_custom, CommandSource::User),
        None
    );
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
    let merged = doc
        .find_block(table_id)
        .unwrap()
        .kind
        .table_block()
        .unwrap();
    assert_eq!(merged.rows[0].cells.len(), 1);
    assert_eq!(merged.rows[0].cells[0].colspan, 2);
    assert_eq!(merged.rows[0].cells[0].blocks.len(), 2);

    // Insert a column after visual col 0 (splits across the colspan=2 header cell!).
    let insert_col_tx = doc
        .build_insert_table_column_transaction(table_id, 0, true, None)
        .unwrap();
    let applied = doc.apply(insert_col_tx);
    history.push_inverse(applied.inverse);
    let after_col = doc
        .find_block(table_id)
        .unwrap()
        .kind
        .table_block()
        .unwrap();
    assert_eq!(after_col.grid_col_count(), 3);
    assert_eq!(after_col.rows[0].cells[0].colspan, 3);
    assert_eq!(after_col.rows[1].cells.len(), 3);

    // Split the merged header cell back into 3 1x1 cells.
    let split_tx = doc
        .build_split_table_cell_transaction(table_id, 0, 0, None)
        .unwrap();
    let applied = doc.apply(split_tx);
    history.push_inverse(applied.inverse);
    let after_split = doc
        .find_block(table_id)
        .unwrap()
        .kind
        .table_block()
        .unwrap();
    assert_eq!(after_split.rows[0].cells.len(), 3);
    assert_eq!(after_split.rows[0].cells[0].colspan, 1);

    // Insert row below and delete it.
    let insert_row_tx = doc
        .build_insert_table_row_transaction(table_id, 1, true, None)
        .unwrap();
    let applied = doc.apply(insert_row_tx);
    history.push_inverse(applied.inverse);
    let after_row = doc
        .find_block(table_id)
        .unwrap()
        .kind
        .table_block()
        .unwrap();
    assert_eq!(after_row.rows.len(), 3);

    // Undo all 4 operations restores exact 2x2 table.
    for _ in 0..4 {
        history.undo(&mut doc).unwrap();
    }
    let restored = doc
        .find_block(table_id)
        .unwrap()
        .kind
        .table_block()
        .unwrap();
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
    assert_eq!(
        decoded.blocks[0].kind.heading_data().map(|h| h.level),
        Some(1)
    );
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
    let split_tx = doc.build_split_block_transaction(&split_pos, None).unwrap();
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
        Err(NodeDataError::InvalidData {
            kind: "callout",
            ..
        })
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
        reparsed
            .find_block(unknown_id)
            .unwrap()
            .read_data::<CalloutSpec>(),
        Err(NodeDataError::KindMismatch {
            expected: "callout",
            ..
        })
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
        fn deserialize<D: serde::Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
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
    let child = BlockNode::new(
        child_id,
        BlockKind::paragraph(leaf_inline),
        BlockStyle::default(),
    );
    let inner_card_id = doc.alloc_block_id();
    let inner_card = BlockNode::new(
        inner_card_id,
        BlockKind::container::<CardSpec>(
            CardData {
                title: "Inner v1".into(),
            },
            vec![child],
        ),
        BlockStyle::default(),
    );
    let outer_quote_id = doc.add_blockquote(vec![inner_card]);

    let outer_v0 = doc.find_block(outer_quote_id).unwrap().version;
    let inner_v0 = doc.find_block(inner_card_id).unwrap().version;
    let mut history = History::default();

    // Payload-only block edit preserves children and block ID, bumps ancestors, is non-structural.
    let applied = doc.apply(Transaction::new(vec![Step::update_block_data::<CardSpec>(
        inner_card_id,
        CardData {
            title: "Inner v2".into(),
        },
    )]));
    history.push_inverse(applied.inverse);
    assert!(!applied.change_set.structure_changed);
    assert!(applied.change_set.dirty_blocks.contains(&inner_card_id));
    assert!(applied.change_set.dirty_blocks.contains(&outer_quote_id));
    assert!(doc.find_block(inner_card_id).unwrap().version > inner_v0);
    assert!(doc.find_block(outer_quote_id).unwrap().version > outer_v0);
    assert_eq!(
        doc.find_block(inner_card_id)
            .unwrap()
            .kind
            .children()
            .unwrap()[0]
            .id,
        child_id
    );
    assert_eq!(
        doc.find_block(inner_card_id)
            .unwrap()
            .read_data::<CardSpec>()
            .unwrap()
            .title,
        "Inner v2"
    );

    // Stale wrong-kind block update is rejected without dirtying or changing data.
    let stale_block = doc.apply(Transaction::new(vec![Step::update_block_data::<
        OtherLeafSpec,
    >(
        inner_card_id,
        CardData {
            title: "Corrupted".into(),
        },
    )]));
    assert!(stale_block.change_set.dirty_blocks.is_empty());
    assert_eq!(
        doc.find_block(inner_card_id)
            .unwrap()
            .read_data::<CardSpec>()
            .unwrap()
            .title,
        "Inner v2"
    );

    // Checked inline payload update preserves object ID and bumps ancestors.
    let inline_edit = doc.apply(Transaction::new(vec![
        Step::update_inline_data::<StatusSpec>(
            child_id,
            badge_id,
            StatusData {
                label: "Done".into(),
            },
        ),
    ]));
    history.push_inverse(inline_edit.inverse);
    assert!(!inline_edit.change_set.structure_changed);
    assert!(inline_edit.change_set.dirty_blocks.contains(&child_id));
    assert!(inline_edit.change_set.dirty_blocks.contains(&inner_card_id));
    assert!(
        inline_edit
            .change_set
            .dirty_blocks
            .contains(&outer_quote_id)
    );

    // Stale wrong-kind inline update does not overwrite the badge.
    let stale_inline = doc.apply(Transaction::new(vec![Step::update_inline_data::<
        OtherInlineSpec,
    >(
        child_id,
        badge_id,
        StatusData {
            label: "Wrong".into(),
        },
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
        doc.find_block(inner_card_id)
            .unwrap()
            .read_data::<CardSpec>()
            .unwrap()
            .title,
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
    doc.add_grid_block::<SheetGridSpec>(
        SheetMeta {
            sheet_name: "Q4".into(),
        },
        table,
    );
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
    assert_eq!(
        nodes[1].read_data::<SheetGridSpec>().unwrap().sheet_name,
        "Q4"
    );

    // Unsupported version and broken UTF-8 span boundaries are rejected.
    let bad_version = encoded.replace("\"version\":1", "\"version\":99");
    assert!(matches!(
        ClipboardFragment::from_versioned_json(&bad_version),
        Err(ClipboardFragmentError::UnsupportedVersion {
            found: 99,
            expected: 1
        })
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
    assert_eq!(
        serde_json::from_str::<RichCommand>(&builtin_json).unwrap(),
        builtin
    );

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
        fn format_id(&self) -> &str {
            "alt-markdown"
        }
        fn mime_types(&self) -> &[&str] {
            &["text/markdown"]
        }
        fn file_extensions(&self) -> &[&str] {
            &["md"]
        }
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

#[test]
fn boundary_ergonomics_exit_and_builtin_block_commands_roundtrip() {
    // 1. Blockquote exit on empty trailing Enter, unwrap on first-child Backspace, and footnote isolation.
    let mut doc = Document::new();
    let first_quote_child = doc.alloc_block_id();
    let second_quote_child = doc.alloc_block_id();
    let quote_id = doc.add_blockquote(vec![
        BlockNode::new(
            first_quote_child,
            BlockKind::paragraph(InlineContent::plain("Quoted")),
            BlockStyle::default(),
        ),
        BlockNode::new(
            second_quote_child,
            BlockKind::paragraph(InlineContent::empty()),
            BlockStyle::default(),
        ),
    ]);
    let bookmark_id = doc.anchors.add_bookmark(
        "q".to_owned(),
        AnchorPosition {
            block_id: first_quote_child,
            offset: 2,
        },
        BoundaryAffinity::After,
    );
    let mut history = History::default();

    let empty_pos = doc.doc_pos_for_block(second_quote_child, 0).unwrap();
    let exit_tx = doc.build_split_block_transaction(&empty_pos, None).unwrap();
    let applied_exit = doc.apply(exit_tx);
    history.push_inverse(applied_exit.inverse);
    assert_eq!(doc.blocks.len(), 2);
    assert_eq!(doc.blocks[0].id, quote_id);
    assert_eq!(doc.blocks[1].id, second_quote_child);
    assert!(doc.blocks[1].kind.is_paragraph());

    let first_pos = doc.doc_pos_for_block(first_quote_child, 0).unwrap();
    let BoundaryDeleteOutcome::Edit(unwrap_tx) =
        doc.plan_backward_boundary_delete(&first_pos, None).unwrap()
    else {
        panic!("expected unwrap edit");
    };
    let applied_unwrap = doc.apply(*unwrap_tx);
    history.push_inverse(applied_unwrap.inverse);
    assert!(doc.find_block(quote_id).is_none());
    assert_eq!(doc.blocks[0].id, first_quote_child);
    assert_eq!(
        doc.anchors.bookmark(bookmark_id).unwrap().position,
        AnchorPosition {
            block_id: first_quote_child,
            offset: 2,
        }
    );

    // Undo restores the original 2-child blockquote cleanly.
    history.undo(&mut doc).unwrap();
    history.undo(&mut doc).unwrap();
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(
        doc.container_blocks(ContainerTarget::Container(quote_id))
            .unwrap()
            .len(),
        2
    );

    // Footnote bodies are never unwrapped or exited on Enter/Backspace.
    let footnote_para = doc.alloc_block_id();
    let footnote_body = doc.alloc_block_id();
    let footnote_id = doc.anchors.add_footnote_block(footnote_body);
    doc.push_block(BlockNode::new(
        footnote_body,
        footnote_body_kind(vec![BlockNode::new(
            footnote_para,
            BlockKind::paragraph(InlineContent::empty()),
            BlockStyle::default(),
        )]),
        BlockStyle::default(),
    ));
    let footnote_pos = doc.doc_pos_for_block(footnote_para, 0).unwrap();
    assert!(
        doc.plan_backward_boundary_delete(&footnote_pos, None)
            .is_none()
    );
    assert!(doc.plan_exit_block(&footnote_pos, None).is_none());
    assert_eq!(
        doc.anchors.footnote(footnote_id).unwrap().block_id,
        footnote_body
    );

    // 2. Horizontal rule deletion, non-destructive table/code entry, empty code conversion, and ExitBlock.
    let mut doc = Document::new();
    let before_id = doc.add_paragraph(InlineContent::plain("Before"), BlockStyle::default());
    let rule_id = doc.add_horizontal_rule();
    let middle_id = doc.add_paragraph(InlineContent::plain("Middle"), BlockStyle::default());
    let code_id = doc.add_code_block(Some("rs".to_owned()), InlineContent::plain("let x = 1;"));
    let after_code_id = doc.add_paragraph(InlineContent::plain("After"), BlockStyle::default());

    let middle_start = doc.doc_pos_for_block(middle_id, 0).unwrap();
    let BoundaryDeleteOutcome::Edit(del_rule_tx) = doc
        .plan_backward_boundary_delete(&middle_start, None)
        .unwrap()
    else {
        panic!("expected horizontal rule deletion");
    };
    let applied_rule = doc.apply(*del_rule_tx);
    assert!(doc.find_block(rule_id).is_none());
    doc.apply(applied_rule.inverse);
    assert!(doc.find_block(rule_id).is_some());

    // Forward Delete before horizontal rule removes it, and Undo restores it.
    let before_end = doc.doc_pos_for_block(before_id, 6).unwrap();
    let BoundaryDeleteOutcome::Edit(fwd_del_rule_tx) =
        doc.plan_forward_boundary_delete(&before_end, None).unwrap()
    else {
        panic!("expected forward horizontal rule deletion");
    };
    let applied_fwd_rule = doc.apply(*fwd_del_rule_tx);
    assert!(doc.find_block(rule_id).is_none());
    doc.apply(applied_fwd_rule.inverse);
    assert!(doc.find_block(rule_id).is_some());

    // Backspace after non-empty code block moves caret into code block without mutating.
    let after_code_start = doc.doc_pos_for_block(after_code_id, 0).unwrap();
    let BoundaryDeleteOutcome::MoveSelection(moved) = doc
        .plan_backward_boundary_delete(&after_code_start, None)
        .unwrap()
    else {
        panic!("expected non-destructive move into code block");
    };
    assert_eq!(
        *moved,
        DocumentSelection::caret(
            doc.doc_pos_for_block(code_id, 10).unwrap(),
            Affinity::Downstream
        )
    );

    // Non-empty code block Backspace at offset 0 is a no-op; empty code block converts to paragraph.
    let code_start = doc.doc_pos_for_block(code_id, 0).unwrap();
    assert!(
        doc.plan_backward_boundary_delete(&code_start, None)
            .is_none()
    );
    let BoundaryDeleteOutcome::MoveSelection(exited) =
        doc.plan_exit_block(&code_start, None).unwrap()
    else {
        panic!("expected ExitBlock to move to existing following paragraph");
    };
    assert_eq!(
        *exited,
        DocumentSelection::caret(after_code_start.clone(), Affinity::Downstream)
    );

    let empty_code_id = doc.add_code_block(None, InlineContent::empty());
    let empty_code_pos = doc.doc_pos_for_block(empty_code_id, 0).unwrap();
    let BoundaryDeleteOutcome::Edit(insert_after_tx) =
        doc.plan_exit_block(&empty_code_pos, None).unwrap()
    else {
        panic!("expected ExitBlock at document end to insert trailing paragraph");
    };
    doc.apply(*insert_after_tx);
    assert!(doc.blocks.last().unwrap().kind.is_paragraph());

    let BoundaryDeleteOutcome::Edit(convert_empty_code_tx) = doc
        .plan_backward_boundary_delete(&empty_code_pos, None)
        .unwrap()
    else {
        panic!("expected empty code block to convert to paragraph");
    };
    doc.apply(*convert_empty_code_tx);
    assert!(doc.find_block(empty_code_id).unwrap().kind.is_paragraph());

    // 3. Built-in block commands: ToggleBlockquote, ToggleCodeBlock, SetCodeBlockLanguage,
    // InsertHorizontalRule, InsertTable, and InteractionPolicy gating.
    let mut doc = Document::new();
    let p1 = doc.add_paragraph(InlineContent::plain("Line 1"), BlockStyle::default());
    let p2 = doc.add_paragraph(InlineContent::plain("Line 2"), BlockStyle::default());
    let sel = Some(DocumentSelection::Text {
        anchor: doc.doc_pos_for_block(p1, 0).unwrap(),
        focus: doc.doc_pos_for_block(p2, 6).unwrap(),
        affinity: Affinity::Downstream,
        goal_x: None,
        stored_marks: None,
    });
    let wrap_tx = doc
        .build_toggle_blockquote_transaction(&[p1, p2], sel)
        .unwrap();
    doc.apply(wrap_tx);
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(doc.blocks[0].kind.semantic_kind(), block_kinds::BLOCKQUOTE);

    let unwrap_tx = doc
        .build_toggle_blockquote_transaction(&[p1, p2], None)
        .unwrap();
    doc.apply(unwrap_tx);
    assert_eq!(doc.blocks.len(), 2);
    assert_eq!(doc.blocks[0].id, p1);
    assert_eq!(doc.blocks[1].id, p2);

    let code_tx = doc
        .build_toggle_code_block_transaction(&[p1, p2], None)
        .unwrap();
    doc.apply(code_tx);
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(
        doc.find_block(p1)
            .unwrap()
            .kind
            .inline_content()
            .unwrap()
            .text,
        "Line 1\nLine 2"
    );

    let lang_tx = doc
        .build_set_code_block_language_transaction(p1, Some("rust".to_owned()), None)
        .unwrap();
    doc.apply(lang_tx);
    assert_eq!(
        doc.find_block(p1)
            .unwrap()
            .kind
            .code_block_data()
            .unwrap()
            .language
            .as_deref(),
        Some("rust")
    );

    let uncode_tx = doc
        .build_toggle_code_block_transaction(&[p1], None)
        .unwrap();
    doc.apply(uncode_tx);
    assert_eq!(doc.blocks.len(), 2);
    assert_eq!(doc.blocks[0].kind.inline_content().unwrap().text, "Line 1");
    assert_eq!(doc.blocks[1].kind.inline_content().unwrap().text, "Line 2");

    let split_hr_pos = doc.doc_pos_for_block(p1, 4).unwrap();
    let hr_tx = doc
        .build_insert_horizontal_rule_transaction(&split_hr_pos, None)
        .unwrap();
    doc.apply(hr_tx);
    assert!(doc.blocks[1].kind.is_horizontal_rule());
    assert_eq!(doc.blocks[0].kind.inline_content().unwrap().text, "Line");
    assert_eq!(doc.blocks[2].kind.inline_content().unwrap().text, " 1");

    let table_pos = doc
        .doc_pos_for_block(doc.blocks.last().unwrap().id, 6)
        .unwrap();
    let table_tx = doc
        .build_insert_table_transaction(&table_pos, 2, 2, true, None)
        .unwrap();
    doc.apply(table_tx);
    let table_index = doc
        .blocks
        .iter()
        .position(|block| block.kind.table_block().is_some())
        .unwrap();
    let table_id = doc.blocks[table_index].id;
    let after_table_id = doc.blocks[table_index + 1].id;
    assert!(doc.blocks[table_index + 1].kind.is_paragraph());
    let after_table_pos = doc.doc_pos_for_block(after_table_id, 0).unwrap();
    let BoundaryDeleteOutcome::MoveSelection(entered_table) = doc
        .plan_backward_boundary_delete(&after_table_pos, None)
        .unwrap()
    else {
        panic!("expected Backspace after table to enter last cell");
    };
    assert!(matches!(
        *entered_table,
        DocumentSelection::Text { focus, .. }
            if focus.path.iter().any(|seg| matches!(seg, PathSegment::TableCell { table_id: id, row: 1, col: 1 } if *id == table_id))
    ));

    let mut policy = InteractionPolicy::editable();
    policy.editing.formatting.blocks = false;
    policy.editing.tables.structure = false;
    assert_eq!(
        policy.check_command(&RichCommand::ToggleBlockquote, CommandSource::User),
        Some(CommandOutcome::RejectedByPolicy)
    );
    assert_eq!(
        policy.check_command(&RichCommand::ToggleCodeBlock, CommandSource::User),
        Some(CommandOutcome::RejectedByPolicy)
    );
    assert_eq!(
        policy.check_command(
            &RichCommand::SetCodeBlockLanguage(Some("rs".to_owned())),
            CommandSource::User
        ),
        Some(CommandOutcome::RejectedByPolicy)
    );
    assert_eq!(
        policy.check_command(&RichCommand::InsertHorizontalRule, CommandSource::User),
        Some(CommandOutcome::RejectedByPolicy)
    );
    assert_eq!(
        policy.check_command(
            &RichCommand::InsertTable {
                rows: 2,
                columns: 2,
                with_header: true
            },
            CommandSource::User
        ),
        Some(CommandOutcome::RejectedByPolicy)
    );
    assert_eq!(
        policy.check_command(&RichCommand::ExitBlock, CommandSource::User),
        None
    );
}

#[test]
fn mermaid_spec_markdown_roundtrip_and_svg_dimensions() {
    use gpui_rich::model::{
        MarkdownCodecConfig, MermaidData, MermaidSpec, parse_svg_intrinsic_size,
    };

    let markdown = "# Architecture\n\n```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```\n\n```rust\nfn main() {}\n```";
    let decoded = MarkdownCodec.decode_raw(markdown).expect("decode markdown");
    assert_eq!(decoded.blocks.len(), 3);
    let mermaid = decoded.blocks[1]
        .kind
        .read_data::<MermaidSpec>()
        .expect("fenced mermaid block must decode into MermaidSpec");
    assert_eq!(mermaid.source, "flowchart LR\n    A[Start] --> B[End]");
    assert!(decoded.blocks[2].kind.code_block_data().is_some());

    let encoded = MarkdownCodec.encode(&decoded).expect("encode markdown");
    assert!(
        encoded
            .as_str()
            .contains("```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```")
    );
    let roundtripped = MarkdownCodec.decode(&encoded).expect("re-decode markdown");
    assert_eq!(
        roundtripped.blocks[1]
            .kind
            .read_data::<MermaidSpec>()
            .expect("roundtripped MermaidSpec")
            .source,
        "flowchart LR\n    A[Start] --> B[End]"
    );

    // Opt-out config keeps fenced mermaid blocks as plain CodeBlock nodes.
    let plain_codec = MarkdownCodec::with_config(MarkdownCodecConfig {
        mermaid_fences: false,
    });
    let plain_doc = plain_codec
        .decode_raw(markdown)
        .expect("decode markdown without mermaid fences");
    assert_eq!(
        plain_doc.blocks[1]
            .kind
            .code_block_data()
            .and_then(|code| code.language.clone())
            .as_deref(),
        Some("mermaid")
    );

    // JSON round-trip preserves MermaidSpec payload including optional metadata.
    let mut doc = Document::new();
    doc.add_leaf_block::<MermaidSpec>(
        MermaidData::new("sequenceDiagram\nA->>B: Ping")
            .with_caption("Ping flow")
            .with_max_height_pt(320.0),
    );
    let json = JsonCodec.encode(&doc).expect("encode json");
    let json_doc = JsonCodec.decode(&json).expect("decode json");
    let restored = json_doc.blocks[0]
        .kind
        .read_data::<MermaidSpec>()
        .expect("restored MermaidSpec");
    assert_eq!(restored.caption.as_deref(), Some("Ping flow"));
    assert_eq!(restored.max_height_pt, Some(320.0));

    // SVG dimension parsing prefers viewBox over percentage width and falls back to px/pt sizes.
    assert_eq!(
        parse_svg_intrinsic_size(r#"<svg width="100%" viewBox="0 0 320.5 180.25"></svg>"#),
        Some((320.5, 180.25))
    );
    assert_eq!(
        parse_svg_intrinsic_size(r#"<svg width="240px" height="120pt"></svg>"#),
        Some((240.0, 160.0))
    );
    assert_eq!(
        parse_svg_intrinsic_size(r#"<svg width="100%"></svg>"#),
        None
    );

    #[cfg(feature = "mermaid")]
    {
        use gpui_rich::model::{BuiltInMermaidBackend, MermaidBackend, MermaidThemePalette};

        let backend = BuiltInMermaidBackend::new();
        let light = backend
            .render_svg(
                "flowchart LR\nA[Input] --> B[Output]",
                &MermaidThemePalette::light(),
            )
            .expect("compile light flowchart");
        assert!(light.svg.contains("<svg"));
        assert!(light.intrinsic_width > 0.0 && light.intrinsic_height > 0.0);

        let dark = backend
            .render_svg(
                "sequenceDiagram\nAlice->>Bob: Hello",
                &MermaidThemePalette::dark(),
            )
            .expect("compile dark sequence diagram");
        assert!(dark.svg.contains("<svg"));
        assert!(dark.intrinsic_width > 0.0 && dark.intrinsic_height > 0.0);

        assert!(
            backend
                .render_svg("   ", &MermaidThemePalette::light())
                .is_err(),
            "empty source must fail cleanly"
        );
    }
}
