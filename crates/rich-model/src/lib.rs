pub mod block;
pub mod inline;
pub mod selection;
pub mod style;
pub mod transaction;

pub use block::*;
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
}
