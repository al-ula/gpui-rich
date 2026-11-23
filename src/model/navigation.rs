use crate::model::block::{BlockId, BlockNode, Document};
use crate::model::inline::OBJECT_REPLACEMENT_CHAR;
use crate::model::selection::{DocPos, PathSegment};
use crate::model::transaction::{Step, Transaction};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Previous,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationTarget {
    Exact(DocPos),
    Heading(BlockId),
    PreviousHeading(Option<u8>),
    NextHeading(Option<u8>),
    PreviousParagraph,
    NextParagraph,
    PreviousTable,
    NextTable,
    TableCell {
        table_id: BlockId,
        row: usize,
        col: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub visible_only: bool,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            regex: false,
            visible_only: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub block_id: BlockId,
    pub range: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchState {
    pub query: String,
    pub options: SearchOptions,
    pub matches: Vec<SearchMatch>,
    pub active: Option<usize>,
}
impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            options: SearchOptions::default(),
            matches: Vec::new(),
            active: None,
        }
    }
}
impl SearchState {
    pub fn advance(&mut self, direction: NavigationDirection) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            self.active = None;
            return None;
        }
        let len = self.matches.len();
        self.active = Some(match (self.active, direction) {
            (None, NavigationDirection::Next) => 0,
            (None, NavigationDirection::Previous) => len - 1,
            (Some(i), NavigationDirection::Next) => (i + 1) % len,
            (Some(i), NavigationDirection::Previous) => (i + len - 1) % len,
        });
        self.active.and_then(|i| self.matches.get(i))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NavigationCommand {
    GoTo(NavigationTarget),
    Find {
        query: String,
        options: SearchOptions,
    },
    FindNext,
    FindPrevious,
    Replace {
        replacement: String,
    },
    ReplaceAll {
        replacement: String,
    },
}
impl NavigationCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::GoTo(_) => "navigation.go_to",
            Self::Find { .. } => "navigation.find",
            Self::FindNext => "navigation.find_next",
            Self::FindPrevious => "navigation.find_previous",
            Self::Replace { .. } => "navigation.replace",
            Self::ReplaceAll { .. } => "navigation.replace_all",
        }
    }
    pub fn is_document_mutation(&self) -> bool {
        matches!(self, Self::Replace { .. } | Self::ReplaceAll { .. })
    }
}

/// Canonicalizes the current path using leaf identity and validates a grapheme caret boundary.
pub fn validate_doc_pos(document: &Document, pos: &DocPos) -> Option<DocPos> {
    let leaf = pos.leaf_block_id()?;
    let text = &document.find_block(leaf)?.kind.inline_content()?.text;
    if pos.offset != text.len()
        && !text
            .grapheme_indices(true)
            .any(|(offset, _)| offset == pos.offset)
    {
        return None;
    }
    document.doc_pos_for_block(leaf, pos.offset)
}

/// Resolves an absolute or relative document target. Relative movement skips folded content.
pub fn navigate_target(
    document: &Document,
    target: &NavigationTarget,
    current: Option<&DocPos>,
) -> Option<DocPos> {
    match target {
        NavigationTarget::Exact(position) => return validate_doc_pos(document, position),
        NavigationTarget::Heading(id) => {
            document.find_block(*id)?.kind.heading_data()?;
            return document.doc_pos_for_block(*id, 0);
        }
        NavigationTarget::TableCell { table_id, row, col } => {
            let table = document.find_block(*table_id)?.kind.table_block()?;
            let (_, placements, grid) = table.resolve_grid();
            let placement = placements.get((*grid.get(*row)?.get(*col)?)?)?;
            let cell = table
                .rows
                .get(placement.row_index)?
                .cells
                .get(placement.cell_index)?;
            return document.doc_pos_for_block(first_text_leaf(&cell.blocks)?, 0);
        }
        _ => {}
    }
    let visible: HashSet<_> = document
        .collect_visible_leaf_blocks()
        .into_iter()
        .map(|block| block.id)
        .collect();
    let mut order = Vec::new();
    collect_navigation_nodes(&document.blocks, &mut order);
    let table_move = matches!(
        target,
        NavigationTarget::PreviousTable | NavigationTarget::NextTable
    );
    let current_id = current.and_then(|position| {
        if table_move {
            position
                .path
                .iter()
                .rev()
                .find_map(|segment| match *segment {
                    PathSegment::TableCell { table_id, .. } => Some(table_id),
                    _ => None,
                })
                .or_else(|| position.leaf_block_id())
        } else {
            position.leaf_block_id()
        }
    });
    let at = current_id.and_then(|id| order.iter().position(|block| block.id == id));
    let candidate = |block: &&BlockNode| -> Option<BlockId> {
        let id = match target {
            NavigationTarget::PreviousHeading(level) | NavigationTarget::NextHeading(level) => {
                let heading = block.kind.heading_data()?;
                if level.is_some_and(|level| level != heading.level) {
                    return None;
                }
                block.id
            }
            NavigationTarget::PreviousParagraph | NavigationTarget::NextParagraph => {
                block.kind.inline_content()?;
                block.id
            }
            NavigationTarget::PreviousTable | NavigationTarget::NextTable => {
                let table = block.kind.table_block()?;
                table
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .find_map(|cell| first_text_leaf(&cell.blocks))?
            }
            _ => return None,
        };
        visible.contains(&id).then_some(id)
    };
    let next = matches!(
        target,
        NavigationTarget::NextHeading(_)
            | NavigationTarget::NextParagraph
            | NavigationTarget::NextTable
    );
    let id = if next {
        order[at.map_or(0, |index| index + 1)..]
            .iter()
            .find_map(candidate)
    } else {
        order[..at.unwrap_or(order.len())]
            .iter()
            .rev()
            .find_map(candidate)
    }?;
    document.doc_pos_for_block(id, 0)
}

fn collect_navigation_nodes<'a>(nodes: &'a [BlockNode], order: &mut Vec<&'a BlockNode>) {
    for node in nodes {
        order.push(node);
        if let Some(children) = node.kind.children() {
            collect_navigation_nodes(children, order);
        } else if let Some(table) = node.kind.table_block() {
            for cell in table.rows.iter().flat_map(|row| &row.cells) {
                collect_navigation_nodes(&cell.blocks, order);
            }
        }
    }
}

fn first_text_leaf(nodes: &[BlockNode]) -> Option<BlockId> {
    nodes.iter().find_map(|node| {
        if node.kind.inline_content().is_some() {
            Some(node.id)
        } else if let Some(children) = node.kind.children() {
            first_text_leaf(children)
        } else {
            node.kind
                .table_block()?
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .find_map(|cell| first_text_leaf(&cell.blocks))
        }
    })
}

pub fn find_matches(document: &Document, query: &str, options: SearchOptions) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let pattern;
    let regex = if !options.regex && options.case_sensitive {
        None
    } else {
        let expression = if options.regex {
            query
        } else {
            pattern = regex::escape(query);
            &pattern
        };
        let Ok(regex) = RegexBuilder::new(expression)
            .case_insensitive(!options.case_sensitive)
            .build()
        else {
            return Vec::new();
        };
        Some(regex)
    };
    let leaves = if options.visible_only {
        document.collect_visible_leaf_blocks()
    } else {
        document.collect_leaf_blocks()
    };
    let mut matches = Vec::new();
    for block in leaves {
        let Some(content) = block.kind.inline_content() else {
            continue;
        };
        let slices = content
            .objects
            .iter()
            .scan(0, |start, object| {
                let range = *start..object.byte_offset;
                *start = object.byte_offset + OBJECT_REPLACEMENT_CHAR.len_utf8();
                Some(range)
            })
            .chain(std::iter::once(
                content.objects.last().map_or(0, |entry| {
                    entry.byte_offset + OBJECT_REPLACEMENT_CHAR.len_utf8()
                })..content.len(),
            ));
        for range in slices {
            let Some(text) = content.text.get(range.clone()) else {
                continue;
            };
            if let Some(regex) = &regex {
                matches.extend(regex.find_iter(text).filter(|found| !found.is_empty()).map(
                    |found| SearchMatch {
                        block_id: block.id,
                        range: range.start + found.start()..range.start + found.end(),
                    },
                ));
            } else {
                matches.extend(text.match_indices(query).map(|(start, _)| SearchMatch {
                    block_id: block.id,
                    range: range.start + start..range.start + start + query.len(),
                }));
            }
        }
    }
    matches
}

/// Applies literal replacements in descending offset order per block, without crossing atoms.
pub fn replace_matches(
    document: &Document,
    matches: &[SearchMatch],
    replacement: &str,
) -> Transaction {
    let mut ordered: Vec<_> = matches.iter().collect();
    ordered.sort_unstable_by_key(|found| (found.block_id, std::cmp::Reverse(found.range.start)));
    let mut last = None;
    let mut steps = Vec::with_capacity(ordered.len() * 2);
    for found in ordered {
        let Some(content) = document
            .find_block(found.block_id)
            .and_then(|block| block.kind.inline_content())
        else {
            continue;
        };
        let range = &found.range;
        let Some(text) = content.text.get(range.clone()) else {
            continue;
        };
        if text.is_empty()
            || text == replacement
            || text.contains(OBJECT_REPLACEMENT_CHAR)
            || last.is_some_and(|(block_id, start)| block_id == found.block_id && start < range.end)
        {
            continue;
        }
        steps.push(Step::DeleteRange {
            block_id: found.block_id,
            range: range.clone(),
        });
        if !replacement.is_empty() {
            steps.push(Step::InsertText {
                block_id: found.block_id,
                offset: range.start,
                text: replacement.to_owned(),
                style: Some(content.style_at(range.start)),
            });
        }
        last = Some((found.block_id, range.start));
    }
    Transaction::new(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BlockStyle, InlineContent};

    #[test]
    fn unicode_search_preserves_original_offsets_and_literal_replacement() {
        let mut document = Document::new();
        let id =
            document.add_paragraph(InlineContent::plain("é İ K kelvin"), BlockStyle::default());
        let matches = find_matches(&document, "k", SearchOptions::default());
        let text = &document
            .find_block(id)
            .unwrap()
            .kind
            .inline_content()
            .unwrap()
            .text;
        assert_eq!(
            matches
                .iter()
                .map(|found| &text[found.range.clone()])
                .collect::<Vec<_>>(),
            ["K", "k"]
        );
        let applied = document.apply(replace_matches(&document, &matches, "$1"));
        assert_eq!(
            document
                .find_block(id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "é İ $1 $1elvin"
        );
        document.apply(applied.inverse);
        assert_eq!(
            document
                .find_block(id)
                .unwrap()
                .kind
                .inline_content()
                .unwrap()
                .text,
            "é İ K kelvin"
        );
    }
}
