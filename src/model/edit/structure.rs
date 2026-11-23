//! Structural editing commands and derived list marker values.

use crate::model::block::{BlockId, Document, ListKind, NumberStyle};
use crate::model::transaction::{Step, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Numbering metadata attached to an ordered-list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListNumbering {
    Restart(u32),
    ContinueFrom(BlockId),
}

/// Sparse per-block ordered-list numbering overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderedListState {
    pub numbering: HashMap<BlockId, ListNumbering>,
}

impl OrderedListState {
    pub fn set(
        &mut self,
        block_id: BlockId,
        numbering: Option<ListNumbering>,
    ) -> Option<ListNumbering> {
        match numbering {
            Some(numbering) => self.numbering.insert(block_id, numbering),
            None => self.numbering.remove(&block_id),
        }
    }

    pub fn get(&self, block_id: BlockId) -> Option<ListNumbering> {
        self.numbering.get(&block_id).copied()
    }
}

/// Undoable structural operations on sibling blocks and ordered-list numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StructureCommand {
    MoveSibling {
        block_id: BlockId,
        to_index: usize,
    },
    RestartOrderedList {
        block_id: BlockId,
        start: u32,
    },
    ContinueOrderedList {
        block_id: BlockId,
        previous_item_id: BlockId,
    },
}

impl StructureCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::MoveSibling { .. } => "structure.move_sibling",
            Self::RestartOrderedList { .. } => "structure.restart_ordered_list",
            Self::ContinueOrderedList { .. } => "structure.continue_ordered_list",
        }
    }

    pub fn is_document_mutation(&self) -> bool {
        true
    }

    /// Builds an undoable model transaction; callers apply it through `Document::apply`.
    pub fn transaction(&self, document: &Document) -> Option<Transaction> {
        match *self {
            Self::MoveSibling { block_id, to_index } => {
                let (target, from) = document.locate_block(block_id)?;
                let blocks = document.container_blocks(target)?;
                if from >= blocks.len() || from == to_index || to_index >= blocks.len() {
                    return None;
                }
                Some(Transaction::new(vec![Step::MoveBlockIn {
                    target,
                    from,
                    to: to_index,
                }]))
            }
            Self::RestartOrderedList { block_id, start } => {
                ordered_item(document, block_id)?;
                Some(Transaction::new(vec![Step::SetListNumbering {
                    block_id,
                    numbering: Some(ListNumbering::Restart(start.max(1))),
                }]))
            }
            Self::ContinueOrderedList {
                block_id,
                previous_item_id,
            } => {
                let item = ordered_item(document, block_id)?;
                let previous = ordered_item(document, previous_item_id)?;
                let (target_container, target_index) = document.locate_block(block_id)?;
                let (source_container, source_index) = document.locate_block(previous_item_id)?;
                if target_container != source_container || source_index >= target_index {
                    return None;
                }
                if item.indent_level != previous.indent_level || item.kind != previous.kind {
                    return None;
                }
                Some(Transaction::new(vec![Step::SetListNumbering {
                    block_id,
                    numbering: Some(ListNumbering::ContinueFrom(previous_item_id)),
                }]))
            }
        }
    }
}

fn ordered_item(
    document: &Document,
    block_id: BlockId,
) -> Option<crate::model::block::ListItemData> {
    let item = document.find_block(block_id)?.kind.list_item_data()?;
    matches!(item.kind, ListKind::Ordered(_)).then_some(item)
}

/// Computes all list markers once, suitable for reuse by flow and recursive block layout.
pub fn ordered_list_markers(document: &Document) -> HashMap<BlockId, String> {
    let mut markers = HashMap::new();
    let mut ordinals = HashMap::new();
    compute_markers(&document.blocks, document, &mut markers, &mut ordinals);
    markers
}

fn compute_markers(
    blocks: &[crate::model::block::BlockNode],
    document: &Document,
    out: &mut HashMap<BlockId, String>,
    ordinals: &mut HashMap<BlockId, u32>,
) {
    let mut last_by_list = HashMap::<(crate::model::block::ListId, u8), (u32, NumberStyle)>::new();
    for block in blocks {
        if let Some(item) = block.kind.list_item_data() {
            if matches!(item.kind, ListKind::Ordered(_)) {
                last_by_list.retain(|(_, indent_level), _| *indent_level <= item.indent_level);
            }
            let marker = match item.kind {
                ListKind::Bullet => Some("•".to_owned()),
                ListKind::Checklist => Some(
                    if item.checked == Some(true) {
                        "☑"
                    } else {
                        "☐"
                    }
                    .to_owned(),
                ),
                ListKind::Ordered(style) => {
                    let numbering = document.ordered_lists.get(block.id);
                    let ordinal = match numbering {
                        Some(ListNumbering::Restart(start)) => start.max(1),
                        Some(ListNumbering::ContinueFrom(previous)) => ordinals
                            .get(&previous)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(1),
                        None => last_by_list
                            .get(&(item.list_id, item.indent_level))
                            .filter(|(_, prior_style)| *prior_style == style)
                            .map_or(1, |(prior, _)| prior.saturating_add(1)),
                    };
                    ordinals.insert(block.id, ordinal);
                    last_by_list.insert((item.list_id, item.indent_level), (ordinal, style));
                    Some(format_marker(ordinal, style))
                }
            };
            if let Some(marker) = marker {
                out.insert(block.id, marker);
            }
        }
        if let Some(children) = block.kind.children() {
            compute_markers(children, document, out, ordinals);
        }
        if let Some(table) = block.kind.table_block() {
            for row in &table.rows {
                for cell in &row.cells {
                    compute_markers(&cell.blocks, document, out, ordinals);
                }
            }
        }
    }
}

fn format_marker(value: u32, style: NumberStyle) -> String {
    match style {
        NumberStyle::Decimal => value.to_string(),
        NumberStyle::LowerAlpha => alpha_marker(value),
        NumberStyle::LowerRoman => roman_marker(value),
    }
}

fn alpha_marker(mut value: u32) -> String {
    let mut bytes = Vec::new();
    while value > 0 {
        value -= 1;
        bytes.push(b'a' + (value % 26) as u8);
        value /= 26;
    }
    bytes.reverse();
    String::from_utf8(bytes).unwrap_or_default()
}

fn roman_marker(value: u32) -> String {
    const VALUES: &[(u32, &str)] = &[
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut remaining = value;
    let mut result = String::new();
    for (number, symbol) in VALUES {
        while remaining >= *number {
            result.push_str(symbol);
            remaining -= *number;
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::block::{ListId, ListKind};
    use crate::model::inline::InlineContent;

    #[test]
    fn markers_continue_across_prose_and_restart_explicitly() {
        let mut document = Document::new();
        let first = document.add_list_item(
            ListId(9),
            ListKind::Ordered(NumberStyle::Decimal),
            0,
            None,
            InlineContent::plain("first"),
        );
        document.add_paragraph(InlineContent::plain("prose"), Default::default());
        let second = document.add_list_item(
            ListId(9),
            ListKind::Ordered(NumberStyle::Decimal),
            0,
            None,
            InlineContent::plain("second"),
        );
        document
            .ordered_lists
            .numbering
            .insert(second, ListNumbering::Restart(7));
        let markers = ordered_list_markers(&document);
        assert_eq!(markers.get(&first).map(String::as_str), Some("1"));
        assert_eq!(markers.get(&second).map(String::as_str), Some("7"));
    }

    #[test]
    fn continuation_crosses_list_ids_and_nested_levels_restart() {
        let mut document = Document::new();
        let source = document.add_list_item(
            ListId(1),
            ListKind::Ordered(NumberStyle::Decimal),
            0,
            None,
            InlineContent::plain("source"),
        );
        let nested = document.add_list_item(
            ListId(1),
            ListKind::Ordered(NumberStyle::Decimal),
            1,
            None,
            InlineContent::plain("nested"),
        );
        let parent_again = document.add_list_item(
            ListId(1),
            ListKind::Ordered(NumberStyle::Decimal),
            0,
            None,
            InlineContent::plain("parent again"),
        );
        let nested_again = document.add_list_item(
            ListId(1),
            ListKind::Ordered(NumberStyle::Decimal),
            1,
            None,
            InlineContent::plain("nested again"),
        );
        let target = document.add_list_item(
            ListId(2),
            ListKind::Ordered(NumberStyle::Decimal),
            0,
            None,
            InlineContent::plain("target"),
        );
        document
            .ordered_lists
            .set(source, Some(ListNumbering::Restart(4)));
        let command = StructureCommand::ContinueOrderedList {
            block_id: target,
            previous_item_id: source,
        };
        if let Some(transaction) = command.transaction(&document) {
            document.apply(transaction);
        }
        let markers = ordered_list_markers(&document);
        assert_eq!(markers.get(&parent_again).map(String::as_str), Some("5"));
        assert_eq!(markers.get(&nested).map(String::as_str), Some("1"));
        assert_eq!(markers.get(&nested_again).map(String::as_str), Some("1"));
        assert_eq!(markers.get(&target).map(String::as_str), Some("5"));
    }
}
