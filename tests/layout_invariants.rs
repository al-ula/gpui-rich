use gpui_rich::layout::*;
use gpui_rich::model::*;

#[test]
fn mixed_font_sizes_and_superscript_share_baseline() {
    let mut shaper = MockShaper;
    let sheet = StyleSheet::default();
    let config = DocumentLayoutConfig::default();

    let mut content = InlineContent::empty();
    content.push_text("E = mc", InlineStyle::default().with_font_size(12.0));
    content.push_text(
        "2",
        InlineStyle::default()
            .with_font_size(12.0)
            .with_baseline_shift(BaselineShift::Superscript),
    );
    content.push_text(" HUGE", InlineStyle::default().with_font_size(28.0));

    let layout = layout_paragraph(
        &content,
        &BlockStyle::default(),
        &sheet,
        &config,
        600.0,
        1.0,
        &mut shaper,
    );

    assert_eq!(layout.lines.len(), 1);
    let line = &layout.lines[0];
    assert_eq!(line.fragments.len(), 3);

    // The 28pt segment should drive line_ascent above the 12pt segment's ascent.
    let normal_ascent = pt_to_px(12.0, 1.0) * 0.8;
    let huge_ascent = pt_to_px(28.0, 1.0) * 0.8;
    assert!(line.ascent >= huge_ascent - 0.01);
    assert!(line.ascent > normal_ascent * 2.0);

    // Caret height inside the huge run should be taller than inside the 12pt run.
    let small_caret = layout.caret_rect(1, Affinity::Downstream);
    let huge_caret = layout.caret_rect(content.len() - 1, Affinity::Downstream);
    assert!(huge_caret.height > small_caret.height * 2.0);
}

#[test]
fn wrapping_justification_and_hit_test_roundtrip() {
    let mut shaper = MockShaper;
    let sheet = StyleSheet::default();
    let config = DocumentLayoutConfig::default();

    let content = InlineContent::plain(
        "The quick brown fox jumps over the lazy dog and keeps running across multiple wrapped visual lines.",
    );
    let style = BlockStyle::default().with_align(TextAlign::Justify);

    let layout = layout_paragraph(&content, &style, &sheet, &config, 220.0, 1.0, &mut shaper);
    assert!(layout.lines.len() >= 3);
    assert!(layout.lines[0].soft_wrapped);
    assert!(!layout.lines.last().unwrap().soft_wrapped);

    // Justified first line should expand nearly to the full 220px available width.
    let first_line = &layout.lines[0];
    let last_frag = first_line.fragments.last().unwrap();
    assert!(last_frag.x() + last_frag.width() >= 210.0);

    // Round-trip caret -> hit_test across interior character boundaries.
    for offset in [0, 4, 10, first_line.doc_range.end - 1, content.len()] {
        let rect = layout.caret_rect(offset, Affinity::Downstream);
        let hit = layout.hit_test(Point::new(rect.x, rect.center().y));
        assert_eq!(hit.offset, offset);
    }

    // Upstream vs Downstream affinity at soft-wrap boundary resolves to distinct lines.
    let boundary = first_line.doc_range.end;
    let up_line = layout.line_index_for_pos(boundary, Affinity::Upstream);
    let down_line = layout.line_index_for_pos(boundary, Affinity::Downstream);
    assert_eq!(up_line, 0);
    assert_eq!(down_line, 1);
}

#[test]
fn fenwick_height_index_collapsible_headings_and_scroll_anchor() {
    let mut doc = Document::new();
    let h1 = doc.add_heading(1, InlineContent::plain("Section One"));
    let p1 = doc.add_paragraph(
        InlineContent::plain("First body paragraph with enough text to wrap onto multiple lines inside a narrow column."),
        BlockStyle::default(),
    );
    let p2 = doc.add_paragraph(
        InlineContent::plain("Second paragraph."),
        BlockStyle::default(),
    );
    let h2 = doc.add_heading(1, InlineContent::plain("Section Two"));
    let p3 = doc.add_paragraph(
        InlineContent::plain("Third paragraph below."),
        BlockStyle::default(),
    );

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut shaper = MockShaper;
    flow.sync_document(&doc, 400.0, 1.0);

    let visible_before =
        flow.layout_visible_blocks(&doc, 0.0, 400.0, 600.0, 50.0, 1.0, &mut shaper);
    assert_eq!(visible_before.len(), 5);
    let h2_y_before = flow.block_y_by_id(h2).unwrap();
    let p3_y_before = flow.block_y_by_id(p3).unwrap();
    assert!(p3_y_before > h2_y_before);

    // Collapse Section One: p1 and p2 hide, h2 shifts up while keeping h2 anchored if scrolled there.
    let scroll_at_h2 = h2_y_before;
    let anchor = flow.capture_anchor(scroll_at_h2).unwrap();
    assert_eq!(anchor.block_id, h2);

    doc.toggle_heading_collapsed(h1);
    flow.sync_document(&doc, 400.0, 1.0);
    assert!(!flow.is_visible(doc.top_level_index(p1).unwrap()));
    assert!(!flow.is_visible(doc.top_level_index(p2).unwrap()));
    assert!(flow.is_visible(doc.top_level_index(h2).unwrap()));

    let h2_y_after = flow.block_y_by_id(h2).unwrap();
    assert!(h2_y_after < h2_y_before);

    let restored = flow.restore_anchor(anchor, 40.0);
    let expected = h2_y_after.min((flow.total_height() - 40.0).max(0.0));
    assert!((restored - expected).abs() < 0.5);
}

#[test]
fn recursive_table_layout_and_colspan() {
    let mut doc = Document::new();
    let t_id = doc.alloc_block_id();
    let c1 = doc.alloc_block_id();
    let c2 = doc.alloc_block_id();
    let p1 = doc.alloc_block_id();
    let p2 = doc.alloc_block_id();

    let table = TableBlock {
        columns: vec![ColumnSpec::Ratio(1.0), ColumnSpec::Ratio(2.0)],
        rows: vec![TableRow {
            is_header: true,
            cells: vec![
                TableCell::simple(
                    c1,
                    vec![BlockNode::new(
                        p1,
                        BlockKind::paragraph(InlineContent::plain("Metric")),
                        BlockStyle::default(),
                    )],
                ),
                TableCell::simple(
                    c2,
                    vec![BlockNode::new(
                        p2,
                        BlockKind::paragraph(InlineContent::plain(
                            "Description spanning a wider column with wrapping text.",
                        )),
                        BlockStyle::default(),
                    )],
                ),
            ],
        }],
        column_kinds: Default::default(),
        border_style: TableBorderStyle::default(),
    };

    doc.push_block(
        BlockNode::new(t_id, BlockKind::table(table), BlockStyle::default())
            .with_width_mode(BlockWidth::Wide),
    );

    let mut cache = LayoutCache::<MockBackend>::default();
    let mut shaper = MockShaper;
    let layout = cache.get_or_layout(
        &doc.blocks[0],
        None,
        &doc.stylesheet,
        &doc.config,
        600.0,
        1.0,
        &mut shaper,
    );

    match &layout.kind {
        BlockLayoutKind::Table(tl) => {
            assert_eq!(tl.cells.len(), 2);
            assert!((tl.column_widths[0] - 200.0).abs() < 1.0);
            assert!((tl.column_widths[1] - 400.0).abs() < 1.0);
            assert!(tl.total_height > 20.0);
        }
        _ => panic!("expected table layout"),
    }
}

#[test]
fn vertical_navigation_preserves_goal_x_across_short_middle_block() {
    let mut doc = Document::new();
    let first_id = doc.add_paragraph(
        InlineContent::plain("First paragraph with plenty of horizontal text"),
        BlockStyle::default(),
    );
    let middle_id = doc.add_paragraph(InlineContent::plain("Hi"), BlockStyle::default());
    let third_id = doc.add_paragraph(
        InlineContent::plain("Third paragraph with plenty of horizontal text"),
        BlockStyle::default(),
    );

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut shaper = MockShaper;
    flow.layout_visible_blocks(&doc, 0.0, 900.0, 600.0, 200.0, 1.0, &mut shaper);

    let start_pos = doc.doc_pos_for_block(first_id, 20).unwrap();
    let (mid_pos, mid_aff, goal_x) = flow
        .cached_vertical_move(&start_pos, Affinity::Downstream, None, 1, &doc, 900.0, 1.0)
        .unwrap();
    assert_eq!(mid_pos.leaf_block_id(), Some(middle_id));
    assert_eq!(mid_pos.offset, 2);

    // Moving down again using the preserved goal_x restores column alignment in the third block.
    let (third_pos, _, third_goal_x) = flow
        .cached_vertical_move(&mid_pos, mid_aff, Some(goal_x), 1, &doc, 900.0, 1.0)
        .unwrap();
    assert_eq!(third_pos.leaf_block_id(), Some(third_id));
    assert!((third_goal_x - goal_x).abs() < 0.01);
    assert!(third_pos.offset >= 15);

    let range_bounds = flow
        .range_bounds_in_block(first_id, 0..5, &doc, 900.0, 1.0, &mut shaper)
        .unwrap();
    assert!(range_bounds.width > 0.0);
    assert!(range_bounds.height > 0.0);
}

#[test]
fn custom_block_and_inline_object_measure_via_shaper() {
    struct BadgeSpec;
    impl InlineSpec for BadgeSpec {
        const KIND: &'static str = "badge";
        type Data = String;
    }

    struct CalloutLeafSpec;
    impl BlockSpec for CalloutLeafSpec {
        const KIND: &'static str = "callout_leaf";
        type Shape = block_shape::Leaf;
        type Data = String;
    }
    struct CalloutSpec;
    impl BlockSpec for CalloutSpec {
        const KIND: &'static str = "callout";
        type Shape = block_shape::Container;
        type Data = String;
    }

    struct CustomMetricShaper;

    impl TextShaper for CustomMetricShaper {
        type Backend = MockBackend;

        fn shape_segment(
            &mut self,
            text: &str,
            font_size_px: f32,
            runs: &[ShaperRunSpec],
        ) -> ShapedSegment<Self::Backend> {
            MockShaper.shape_segment(text, font_size_px, runs)
        }

        fn measure_inline_object(&mut self, object: &InlineObject, zoom: f32) -> InlineBoxMetrics {
            if object.read_data::<BadgeSpec>().is_ok() {
                return InlineBoxMetrics {
                    width: 64.0 * zoom,
                    height: 20.0 * zoom,
                    valign: InlineVAlign::Middle,
                };
            }
            MockShaper.measure_inline_object(object, zoom)
        }

        fn measure_custom_block(
            &mut self,
            kind: &str,
            _data: &CustomPayload,
            max_width: f32,
            zoom: f32,
        ) -> Size {
            if kind == CalloutLeafSpec::KIND {
                Size::new(max_width, 84.0 * zoom)
            } else {
                Size::new(max_width, 40.0 * zoom)
            }
        }

        fn measure_custom_chrome(
            &mut self,
            shape: BlockShapeKind,
            kind: &str,
            _data: &CustomPayload,
            _max_width: f32,
            zoom: f32,
        ) -> Insets {
            if shape == BlockShapeKind::Container && kind == CalloutSpec::KIND {
                Insets::new(28.0 * zoom, 12.0 * zoom, 10.0 * zoom, 16.0 * zoom)
            } else {
                Insets::xy(12.0 * zoom, 8.0 * zoom)
            }
        }
    }

    let mut document = Document::new();
    let mut inline = InlineContent::empty();
    inline.push_text("Before ", InlineStyle::default());
    inline.push_object(
        InlineObject::custom::<BadgeSpec>("PRO".into()),
        InlineStyle::default(),
    );
    document.add_paragraph(inline, BlockStyle::default());
    let leaf_id = document.add_leaf_block::<CalloutLeafSpec>("Note".into());
    let child_id = document.alloc_block_id();
    let container_id = document.add_container_block::<CalloutSpec>(
        "Editable Callout".into(),
        vec![BlockNode::new(
            child_id,
            BlockKind::paragraph(InlineContent::plain("Inside container")),
            BlockStyle::default(),
        )],
    );

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut shaper = CustomMetricShaper;
    let visible = flow.layout_visible_blocks(&document, 0.0, 600.0, 600.0, 100.0, 1.0, &mut shaper);
    assert_eq!(visible.len(), 3);

    let leaf = visible.iter().find(|entry| entry.id == leaf_id).unwrap();
    assert!((leaf.layout.height - 84.0).abs() < 0.01);
    assert!(matches!(
        &leaf.layout.kind,
        BlockLayoutKind::CustomLeaf { kind, .. } if kind.as_ref() == "callout_leaf"
    ));

    let container = visible
        .iter()
        .find(|entry| entry.id == container_id)
        .unwrap();
    assert!(matches!(
        &container.layout.kind,
        BlockLayoutKind::CustomContainer { kind, children, chrome, .. }
            if kind.as_ref() == "callout" && children.len() == 1 && (chrome.top - 28.0).abs() < 0.01
    ));
    let hit_pt = Point::new(container.x + 24.0, container.y + 34.0);
    let hit = flow
        .hit_test_canvas(hit_pt, &document, 600.0, 1.0, &mut shaper)
        .expect("expected hit inside custom container child");
    assert_eq!(hit.pos.leaf_block_id(), Some(child_id));
    assert_eq!(hit.pos.top_level_block_id(), Some(container_id));
}

#[test]
fn invalid_custom_metrics_fall_back_and_large_table_culls_offscreen_leaves() {
    struct BadMetricShaper;
    impl TextShaper for BadMetricShaper {
        type Backend = MockBackend;
        fn shape_segment(
            &mut self,
            text: &str,
            font_size_px: f32,
            runs: &[ShaperRunSpec],
        ) -> ShapedSegment<Self::Backend> {
            MockShaper.shape_segment(text, font_size_px, runs)
        }
        fn measure_inline_object(&mut self, _obj: &InlineObject, _zoom: f32) -> InlineBoxMetrics {
            InlineBoxMetrics {
                width: f32::NAN,
                height: -10.0,
                valign: InlineVAlign::Middle,
            }
        }
        fn measure_custom_block(
            &mut self,
            _kind: &str,
            _data: &CustomPayload,
            _max_width: f32,
            _zoom: f32,
        ) -> Size {
            Size::new(f32::INFINITY, -5.0)
        }
        fn measure_custom_chrome(
            &mut self,
            _shape: BlockShapeKind,
            _kind: &str,
            _data: &CustomPayload,
            _max_width: f32,
            _zoom: f32,
        ) -> Insets {
            Insets::new(f32::NAN, -1.0, f32::INFINITY, 4.0)
        }
    }

    struct BadLeafSpec;
    impl BlockSpec for BadLeafSpec {
        const KIND: &'static str = "bad_leaf";
        type Shape = block_shape::Leaf;
        type Data = ();
    }
    struct BadInlineSpec;
    impl InlineSpec for BadInlineSpec {
        const KIND: &'static str = "bad_inline";
        type Data = ();
    }

    let mut doc = Document::new();
    let mut inline = InlineContent::empty();
    inline.push_object(
        InlineObject::custom::<BadInlineSpec>(()),
        InlineStyle::default(),
    );
    doc.add_paragraph(inline, BlockStyle::default());
    let leaf_id = doc.add_leaf_block::<BadLeafSpec>(());

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut bad_shaper = BadMetricShaper;
    let visible = flow.layout_visible_blocks(&doc, 0.0, 500.0, 500.0, 50.0, 1.0, &mut bad_shaper);
    assert!(visible[0].layout.height.is_finite() && visible[0].layout.height > 0.0);
    let leaf = visible.iter().find(|entry| entry.id == leaf_id).unwrap();
    assert!(leaf.layout.width.is_finite() && leaf.layout.width > 0.0);
    assert!(leaf.layout.height.is_finite() && leaf.layout.height > 0.0);

    // Large table culling: only rows intersecting viewport + overscan are returned.
    let mut table_doc = Document::new();
    let table_id = table_doc.alloc_block_id();
    let mut rows = Vec::new();
    for row_index in 0..80 {
        let cell_id = table_doc.alloc_block_id();
        let para_id = table_doc.alloc_block_id();
        rows.push(TableRow {
            is_header: row_index == 0,
            cells: vec![TableCell::simple(
                cell_id,
                vec![BlockNode::new(
                    para_id,
                    BlockKind::paragraph(InlineContent::plain(format!("Row {row_index}"))),
                    BlockStyle::default(),
                )],
            )],
        });
    }
    table_doc.push_block(BlockNode::new(
        table_id,
        BlockKind::table(TableBlock {
            columns: vec![ColumnSpec::Auto],
            column_kinds: Default::default(),
            rows,
            border_style: TableBorderStyle::default(),
        }),
        BlockStyle::default(),
    ));

    let mut table_flow = FlowLayout::<MockBackend>::default();
    let mut mock_shaper = MockShaper;
    let table_visible = table_flow.layout_visible_blocks(
        &table_doc,
        0.0,
        600.0,
        200.0,
        40.0,
        1.0,
        &mut mock_shaper,
    );
    let culled = collect_visible_leaf_paragraphs_in_y_range(&table_visible, -40.0, 240.0);
    assert!(!culled.is_empty());
    assert!(culled.len() < 20);
}

#[test]
fn asymmetric_document_margins_control_paper_and_fill_geometry() {
    let mut document = Document::new();
    document.add_paragraph(
        InlineContent::plain("Margin invariant verification paragraph."),
        BlockStyle::default(),
    );

    let asymmetric = Edges::new(48.0, 96.0, 60.0, 36.0);
    document.config.horizontal_layout =
        HorizontalLayout::Paper(PaperSpec::LETTER.with_margins(asymmetric));

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut shaper = MockShaper;
    let visible_paper =
        flow.layout_visible_blocks(&document, 0.0, 1200.0, 800.0, 100.0, 1.0, &mut shaper);
    let paper_metrics = resolve_horizontal_metrics(1200.0, &document.config, 1.0);
    let sheet = paper_metrics.paper_sheet.expect("paper sheet present");
    let expected_left = pt_to_px(36.0, 1.0);
    let expected_right = pt_to_px(96.0, 1.0);
    let expected_top = pt_to_px(48.0, 1.0);
    let expected_bottom = pt_to_px(60.0, 1.0);

    assert!((paper_metrics.column_x - (sheet.x + expected_left)).abs() < 0.1);
    assert!(
        (paper_metrics.column_width - (sheet.width - expected_left - expected_right)).abs() < 0.1
    );
    assert!((flow.block_y(0) - (sheet.top_y + expected_top)).abs() < 0.1);
    let content_height = visible_paper[0].layout.height;
    let expected_total_paper =
        sheet.top_y + expected_top + content_height + expected_bottom + sheet.desk_margin;
    assert!((flow.total_height() - expected_total_paper).abs() < 0.5);

    document.config.horizontal_layout = HorizontalLayout::Fill;
    document.config.set_active_margins_pt(asymmetric);
    let visible_fill =
        flow.layout_visible_blocks(&document, 0.0, 1000.0, 800.0, 100.0, 1.0, &mut shaper);
    let fill_metrics = resolve_horizontal_metrics(1000.0, &document.config, 1.0);
    assert!(fill_metrics.paper_sheet.is_none());
    assert!((fill_metrics.column_x - expected_left).abs() < 0.1);
    assert!((fill_metrics.column_width - (1000.0 - expected_left - expected_right)).abs() < 0.1);
    assert!((flow.block_y(0) - expected_top).abs() < 0.1);
    let expected_total_fill = expected_top + visible_fill[0].layout.height + expected_bottom;
    assert!((flow.total_height() - expected_total_fill).abs() < 0.5);
}

#[test]
fn bidi_run_splitting_visual_reordering_and_rtl_hit_test_roundtrip() {
    let mut shaper = MockShaper;
    let sheet = StyleSheet::default();
    let config = DocumentLayoutConfig::default();

    // Mixed LTR + RTL sentence in an LTR paragraph: "Hello שלום world"
    let mixed = InlineContent::plain("Hello שלום world");
    let mixed_layout = layout_paragraph(
        &mixed,
        &BlockStyle::default(),
        &sheet,
        &config,
        600.0,
        1.0,
        &mut shaper,
    );
    assert_eq!(mixed_layout.lines.len(), 1);
    let line = &mixed_layout.lines[0];
    assert!(!line.base_direction_rtl);
    assert!(line.fragments.len() >= 3);

    // Verify the Hebrew run has an odd BiDi level and descends visually in x.
    let rtl_frag = line
        .fragments
        .iter()
        .find_map(|frag| match frag {
            LineFragment::Text(t) if t.is_rtl() => Some(t),
            _ => None,
        })
        .expect("expected RTL fragment");
    let rtl_start_x = rtl_frag.local_x_for_byte(rtl_frag.doc_range.start);
    let rtl_end_x = rtl_frag.local_x_for_byte(rtl_frag.doc_range.end);
    assert!(rtl_start_x > rtl_end_x);

    // Caret -> hit_test round-trip inside the RTL run.
    let hebrew_mid = rtl_frag.doc_range.start + "ש".len();
    let caret = mixed_layout.caret_rect(hebrew_mid, Affinity::Downstream);
    let hit = mixed_layout.hit_test(Point::new(caret.x, caret.center().y));
    assert_eq!(hit.offset, hebrew_mid);

    // Selection spanning from "lo " into the middle of "שלום" produces disjoint visual rects
    // because RTL advances right-to-left while LTR advances left-to-right.
    let sel_rects = mixed_layout.selection_rects(3..hebrew_mid, false);
    assert_eq!(sel_rects.len(), 2);

    // RTL-base paragraph ("שלום Hello") reorders visual fragments so "Hello" sits on the left
    // and "שלום" sits on the right, right-aligned by default.
    let rtl_para = InlineContent::plain("שלום Hello");
    let rtl_layout = layout_paragraph(
        &rtl_para,
        &BlockStyle::default(),
        &sheet,
        &config,
        600.0,
        1.0,
        &mut shaper,
    );
    let rtl_line = &rtl_layout.lines[0];
    assert!(rtl_line.base_direction_rtl);
    let first_visual = rtl_line.fragments.first().unwrap();
    let last_visual = rtl_line.fragments.last().unwrap();
    assert!(first_visual.doc_range().start > last_visual.doc_range().start);
    assert!(last_visual.x() + last_visual.width() >= 595.0);
}

#[test]
fn interactive_checklist_code_copy_and_table_divider_hit_tests() {
    let mut doc = Document::new();
    let task_id = doc.add_list_item(
        ListId(1),
        ListKind::Checklist,
        0,
        Some(false),
        InlineContent::plain("Check item"),
    );
    let code_id = doc.add_code_block(Some("rust".into()), InlineContent::plain("fn hi() {}"));
    let table_id = doc.alloc_block_id();
    let c1 = doc.alloc_block_id();
    let c2 = doc.alloc_block_id();
    let p1 = doc.alloc_block_id();
    let p2 = doc.alloc_block_id();
    doc.push_block(BlockNode::new(
        table_id,
        BlockKind::table(TableBlock {
            columns: vec![ColumnSpec::Ratio(1.0), ColumnSpec::Ratio(1.0)],
            rows: vec![TableRow {
                is_header: false,
                cells: vec![
                    TableCell::simple(
                        c1,
                        vec![BlockNode::new(
                            p1,
                            BlockKind::paragraph(InlineContent::plain("A")),
                            BlockStyle::default(),
                        )],
                    ),
                    TableCell::simple(
                        c2,
                        vec![BlockNode::new(
                            p2,
                            BlockKind::paragraph(InlineContent::plain("B")),
                            BlockStyle::default(),
                        )],
                    ),
                ],
            }],
            column_kinds: Default::default(),
            border_style: TableBorderStyle::default(),
        }),
        BlockStyle::default(),
    ));

    let mut flow = FlowLayout::<MockBackend>::default();
    let mut shaper = MockShaper;
    let visible = flow.layout_visible_blocks(&doc, 0.0, 900.0, 800.0, 100.0, 1.0, &mut shaper);

    // Checklist hit test.
    let task_vis = visible.iter().find(|v| v.id == task_id).unwrap();
    let BlockLayoutKind::ListItem {
        checkbox_rect: Some(cb),
        ..
    } = &task_vis.layout.kind
    else {
        panic!("expected checklist rect");
    };
    let cb_center = cb.translate(task_vis.x, task_vis.y).center();
    let cb_hit = flow
        .hit_test_canvas(cb_center, &doc, 900.0, 1.0, &mut shaper)
        .unwrap();
    assert_eq!(cb_hit.toggled_checklist, Some(task_id));

    // Code block copy button hit test.
    let code_vis = visible.iter().find(|v| v.id == code_id).unwrap();
    let BlockLayoutKind::CodeBlock {
        copy_button_rect, ..
    } = &code_vis.layout.kind
    else {
        panic!("expected code block");
    };
    let copy_center = copy_button_rect.translate(code_vis.x, code_vis.y).center();
    let copy_hit = flow
        .hit_test_canvas(copy_center, &doc, 900.0, 1.0, &mut shaper)
        .unwrap();
    assert_eq!(copy_hit.code_block_copy, Some(code_id));

    // Table column divider hit test.
    let table_vis = visible.iter().find(|v| v.id == table_id).unwrap();
    let BlockLayoutKind::Table(table_layout) = &table_vis.layout.kind else {
        panic!("expected table layout");
    };
    let divider_pt = Point::new(
        table_vis.x + table_layout.column_widths[0],
        table_vis.y + table_layout.total_height * 0.5,
    );
    let divider = flow
        .hit_test_table_column_border(divider_pt, &doc, 900.0, 1.0, 5.0)
        .expect("expected column divider hit");
    assert_eq!(divider.table_id, table_id);
    assert_eq!(divider.column_index, 0);
}
