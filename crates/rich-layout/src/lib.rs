pub mod block;
pub mod flow;
pub mod geometry;
pub mod line;
pub mod shaper;

pub use block::*;
pub use flow::*;
pub use geometry::*;
pub use line::*;
pub use shaper::*;

#[cfg(test)]
mod tests {
    use super::*;
    use rich_model::*;

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
        let p2 = doc.add_paragraph(InlineContent::plain("Second paragraph."), BlockStyle::default());
        let h2 = doc.add_heading(1, InlineContent::plain("Section Two"));
        let p3 = doc.add_paragraph(InlineContent::plain("Third paragraph below."), BlockStyle::default());

        let mut flow = FlowLayout::<MockBackend>::default();
        let mut shaper = MockShaper;
        flow.sync_document(&doc, 400.0, 1.0);

        let visible_before = flow.layout_visible_blocks(&doc, 0.0, 400.0, 600.0, 50.0, 1.0, &mut shaper);
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
                            BlockKind::Paragraph(InlineContent::plain("Metric")),
                            BlockStyle::default(),
                        )],
                    ),
                    TableCell::simple(
                        c2,
                        vec![BlockNode::new(
                            p2,
                            BlockKind::Paragraph(InlineContent::plain(
                                "Description spanning a wider column with wrapping text.",
                            )),
                            BlockStyle::default(),
                        )],
                    ),
                ],
            }],
            border_style: TableBorderStyle::default(),
        };

        doc.push_block(
            BlockNode::new(t_id, BlockKind::Table(table), BlockStyle::default())
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
            .cached_vertical_move(
                &mid_pos,
                mid_aff,
                Some(goal_x),
                1,
                &doc,
                900.0,
                1.0,
            )
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
                if let InlineObject::Custom { kind, .. } = object
                    && kind == "badge"
                {
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
                _data: &serde_json::Value,
                max_width: f32,
                zoom: f32,
            ) -> Size {
                if kind == "callout" {
                    Size::new(max_width, 84.0 * zoom)
                } else {
                    Size::new(max_width, 40.0 * zoom)
                }
            }
        }

        let mut document = Document::new();
        let mut inline = InlineContent::empty();
        inline.push_text("Before ", InlineStyle::default());
        inline.push_object(
            InlineObject::Custom {
                kind: "badge".into(),
                data: serde_json::json!({ "label": "PRO" }),
            },
            InlineStyle::default(),
        );
        document.add_paragraph(inline, BlockStyle::default());
        let callout_id = document.add_custom_block("callout", serde_json::json!({ "title": "Note" }));

        let mut flow = FlowLayout::<MockBackend>::default();
        let mut shaper = CustomMetricShaper;
        let visible = flow.layout_visible_blocks(&document, 0.0, 600.0, 600.0, 100.0, 1.0, &mut shaper);
        assert_eq!(visible.len(), 2);

        let callout = visible.iter().find(|entry| entry.id == callout_id).unwrap();
        assert!((callout.layout.height - 84.0).abs() < 0.01);
        assert!(matches!(
            &callout.layout.kind,
            BlockLayoutKind::Custom { kind, .. } if kind == "callout"
        ));
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
        let visible_paper = flow.layout_visible_blocks(&document, 0.0, 1200.0, 800.0, 100.0, 1.0, &mut shaper);
        let paper_metrics = resolve_horizontal_metrics(1200.0, &document.config, 1.0);
        let sheet = paper_metrics.paper_sheet.expect("paper sheet present");
        let expected_left = pt_to_px(36.0, 1.0);
        let expected_right = pt_to_px(96.0, 1.0);
        let expected_top = pt_to_px(48.0, 1.0);
        let expected_bottom = pt_to_px(60.0, 1.0);

        assert!((paper_metrics.column_x - (sheet.x + expected_left)).abs() < 0.1);
        assert!(
            (paper_metrics.column_width - (sheet.width - expected_left - expected_right)).abs()
                < 0.1
        );
        assert!((flow.block_y(0) - (sheet.top_y + expected_top)).abs() < 0.1);
        let content_height = visible_paper[0].layout.height;
        let expected_total_paper =
            sheet.top_y + expected_top + content_height + expected_bottom + sheet.desk_margin;
        assert!((flow.total_height() - expected_total_paper).abs() < 0.5);

        document.config.horizontal_layout = HorizontalLayout::Fill;
        document.config.set_active_margins_pt(asymmetric);
        let visible_fill = flow.layout_visible_blocks(&document, 0.0, 1000.0, 800.0, 100.0, 1.0, &mut shaper);
        let fill_metrics = resolve_horizontal_metrics(1000.0, &document.config, 1.0);
        assert!(fill_metrics.paper_sheet.is_none());
        assert!((fill_metrics.column_x - expected_left).abs() < 0.1);
        assert!((fill_metrics.column_width - (1000.0 - expected_left - expected_right)).abs() < 0.1);
        assert!((flow.block_y(0) - expected_top).abs() < 0.1);
        let expected_total_fill = expected_top + visible_fill[0].layout.height + expected_bottom;
        assert!((flow.total_height() - expected_total_fill).abs() < 0.5);
    }
}
