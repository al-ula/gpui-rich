//! Occupancy-aware edits for rectangular table selections.
use crate::block::{
    BlockId, BlockKind, BlockNode, ColumnSpec, Document, TableBlock, TableCell, TableRow,
};
use crate::clipboard::{
    ClipboardBlock, ClipboardFragment, ClipboardTable, ClipboardTableCell, ClipboardTableRow,
};
use crate::inline::InlineContent;
use crate::selection::{Affinity, DocumentSelection, PathSegment};
use crate::style::{BlockStyle, TextAlign};
use crate::transaction::{Step, Transaction};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GridCommand {
    ClearSelection,
    PasteHtml {
        html: String,
    },
    PasteTsv {
        text: String,
    },
    MoveRows {
        start: usize,
        end: usize,
        to: usize,
    },
    MoveColumns {
        start: usize,
        end: usize,
        to: usize,
    },
    SortRows {
        start: usize,
        end: usize,
        column: usize,
        descending: bool,
    },
    Subdivide {
        rows: usize,
        columns: usize,
    },
    SetColumnKind {
        column: usize,
        kind: Option<crate::PlaceholderKind>,
    },
}

impl GridCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::ClearSelection => "rich.grid.clear",
            Self::PasteHtml { .. } => "rich.grid.paste_html",
            Self::PasteTsv { .. } => "rich.grid.paste_tsv",
            Self::MoveRows { .. } => "rich.grid.move_rows",
            Self::MoveColumns { .. } => "rich.grid.move_columns",
            Self::SortRows { .. } => "rich.grid.sort_rows",
            Self::Subdivide { .. } => "rich.grid.subdivide",
            Self::SetColumnKind { .. } => "rich.grid.set_column_kind",
        }
    }

    pub fn is_document_mutation(&self) -> bool {
        true
    }
}

fn table_position(selection: &DocumentSelection) -> Option<(BlockId, (usize, usize), bool)> {
    match selection {
        DocumentSelection::TableCells {
            table_id,
            anchor_cell,
            focus_cell,
        } => Some((
            *table_id,
            (
                anchor_cell.0.min(focus_cell.0),
                anchor_cell.1.min(focus_cell.1),
            ),
            true,
        )),
        DocumentSelection::Text { focus, .. } => focus.path.iter().rev().find_map(|segment| {
            if let PathSegment::TableCell { table_id, row, col } = *segment {
                Some((table_id, (row, col), false))
            } else {
                None
            }
        }),
    }
}

impl Document {
    pub fn build_grid_transaction(
        &mut self,
        command: &GridCommand,
        selection: &DocumentSelection,
    ) -> Option<Transaction> {
        self.build_grid_edit_transaction(selection, |document, table, into| match command {
            GridCommand::ClearSelection => clear_cells(document, table, selection),
            GridCommand::PasteTsv { text } => paste_cells(document, table, into, &tsv_table(text)),
            GridCommand::PasteHtml { html } => {
                let fragment = ClipboardFragment::from_html(html);
                if let [ClipboardBlock::Table(source)] = fragment.blocks.as_slice() {
                    paste_cells(document, table, into, source)
                } else {
                    false
                }
            }
            GridCommand::MoveRows { start, end, to } => move_rows(table, *start..*end, *to),
            GridCommand::MoveColumns { start, end, to } => move_columns(table, *start..*end, *to),
            GridCommand::SortRows {
                start,
                end,
                column,
                descending,
            } => sort_rows(table, *start..*end, *column, *descending),
            GridCommand::SetColumnKind { column, kind } => {
                if *column >= table.grid_col_count()
                    || table.column_kinds.get(column).copied() == *kind
                {
                    return false;
                }
                match kind {
                    Some(kind) => {
                        table.column_kinds.insert(*column, *kind);
                    }
                    None => {
                        table.column_kinds.remove(column);
                    }
                }
                true
            }
            GridCommand::Subdivide { rows, columns } => {
                if let DocumentSelection::TableCells {
                    anchor_cell,
                    focus_cell,
                    ..
                } = selection
                    && anchor_cell != focus_cell
                {
                    return false;
                }
                subdivide(document, table, into, *rows, *columns)
            }
        })
    }

    pub fn build_grid_paste_transaction(
        &mut self,
        source: &ClipboardTable,
        selection: &DocumentSelection,
    ) -> Option<Transaction> {
        self.build_grid_edit_transaction(selection, |document, table, into| {
            paste_cells(document, table, into, source)
        })
    }

    fn build_grid_edit_transaction(
        &mut self,
        selection: &DocumentSelection,
        edit: impl FnOnce(&mut Self, &mut TableBlock, (usize, usize)) -> bool,
    ) -> Option<Transaction> {
        let (table_id, into, visual) = table_position(selection)?;
        let BlockKind::Grid {
            kind,
            data,
            mut table,
        } = self.find_block(table_id)?.kind.clone()
        else {
            return None;
        };
        let into = if visual {
            into
        } else {
            let placement = table.placement_for_stored_cell(into.0, into.1)?;
            (placement.row_index, placement.visual_col)
        };
        let mut selection_after = selection.clone();
        if !edit(self, &mut table, into) {
            return None;
        }
        let step = Step::SetBlockKind {
            block_id: table_id,
            kind: BlockKind::grid(kind, data, table),
        };
        if let DocumentSelection::Text {
            anchor,
            focus,
            affinity,
            goal_x,
            stored_marks,
        } = selection
            && let Step::SetBlockKind {
                kind: BlockKind::Grid { table, .. },
                ..
            } = &step
        {
            let remap = |position: &crate::DocPos| {
                let mut path = position.path.clone();
                let leaf = position.leaf_block_id()?;
                for (row, line) in table.rows.iter().enumerate() {
                    for (col, cell) in line.cells.iter().enumerate() {
                        if let Some(length) = text_length(&cell.blocks, leaf) {
                            if let Some(segment) = path.iter_mut().find(|segment| matches!(
                                segment, PathSegment::TableCell { table_id: id, .. } if *id == table_id
                            )) {
                                *segment = PathSegment::TableCell { table_id, row, col };
                            }
                            return Some(crate::DocPos {
                                path,
                                offset: position.offset.min(length),
                            });
                        }
                    }
                }
                None
            };
            if let (Some(anchor), Some(focus)) = (remap(anchor), remap(focus)) {
                selection_after = DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: *affinity,
                    goal_x: *goal_x,
                    stored_marks: stored_marks.clone(),
                };
            } else if let Some((_, placements, occupancy)) = Some(table.resolve_grid())
                && let Some(placement) = occupancy
                    .get(into.0)
                    .and_then(|slots| slots.get(into.1))
                    .and_then(|slot| *slot)
                    .and_then(|index| placements.get(index))
                && let Some(first) = table.rows[placement.row_index].cells[placement.cell_index]
                    .blocks
                    .first()
                && let Some(position) = self.table_cell_doc_pos(
                    table_id,
                    placement.row_index,
                    placement.cell_index,
                    first.id,
                    0,
                )
            {
                selection_after = DocumentSelection::caret(position, Affinity::Downstream);
            }
        }
        Some(
            Transaction::new(vec![step])
                .with_selections(Some(selection.clone()), Some(selection_after)),
        )
    }
}

fn text_length(blocks: &[BlockNode], id: BlockId) -> Option<usize> {
    blocks.iter().find_map(|block| {
        if block.id == id {
            return block.kind.inline_content().map(InlineContent::len);
        }
        match &block.kind {
            BlockKind::Container { children, .. } => text_length(children, id),
            BlockKind::Grid { table, .. } => table
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .find_map(|cell| text_length(&cell.blocks, id)),
            _ => None,
        }
    })
}

fn clear_cells(
    document: &mut Document,
    table: &mut TableBlock,
    selection: &DocumentSelection,
) -> bool {
    let DocumentSelection::TableCells {
        anchor_cell,
        focus_cell,
        ..
    } = selection
    else {
        return false;
    };
    let (columns, placements, _) = table.resolve_grid();
    let rows = anchor_cell.0.min(focus_cell.0)..=anchor_cell.0.max(focus_cell.0);
    let cols = anchor_cell.1.min(focus_cell.1)..=anchor_cell.1.max(focus_cell.1);
    if *rows.end() >= table.rows.len() || *cols.end() >= columns {
        return false;
    }
    let mut changed = false;
    for placement in placements {
        if placement.row_index <= *rows.end()
            && placement.row_index + placement.rowspan > *rows.start()
            && placement.visual_col <= *cols.end()
            && placement.visual_col + placement.colspan > *cols.start()
        {
            let cell = &mut table.rows[placement.row_index].cells[placement.cell_index];
            if cell.blocks.len() == 1
                && cell.blocks[0].kind.is_paragraph()
                && cell.blocks[0]
                    .kind
                    .inline_content()
                    .is_some_and(InlineContent::is_empty)
            {
                continue;
            }
            replace_cell(document, cell, Vec::new());
            changed = true;
        }
    }
    changed
}

fn replace_cell(document: &mut Document, cell: &mut TableCell, mut blocks: Vec<BlockNode>) {
    let old_first = cell.blocks.first();
    if blocks.is_empty() {
        blocks.push(BlockNode::new(
            old_first.map_or_else(|| document.alloc_block_id(), |block| block.id),
            BlockKind::paragraph(InlineContent::empty()),
            old_first.map_or_else(BlockStyle::default, |block| block.style.clone()),
        ));
    }
    if let Some(old) = old_first
        && let Some(first) = blocks.first_mut()
        && first.kind.inline_content().is_some()
    {
        first.id = old.id;
        first.version = old.version;
        first.bump_version();
    }
    cell.blocks = blocks;
}

fn tsv_table(text: &str) -> ClipboardTable {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    ClipboardTable {
        column_kinds: Default::default(),
        rows: normalized
            .strip_suffix('\n')
            .unwrap_or(&normalized)
            .split('\n')
            .map(|line| ClipboardTableRow {
                is_header: false,
                cells: line
                    .split('\t')
                    .map(|value| ClipboardTableCell {
                        rowspan: 1,
                        colspan: 1,
                        blocks: vec![ClipboardBlock::Paragraph {
                            content: InlineContent::plain(value),
                            align: TextAlign::Left,
                        }],
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn paste_cells(
    document: &mut Document,
    destination: &mut TableBlock,
    start: (usize, usize),
    source: &ClipboardTable,
) -> bool {
    let fragment = ClipboardFragment::from_blocks(vec![ClipboardBlock::Table(source.clone())]);
    let Some(BlockKind::Grid { table: source, .. }) = fragment
        .to_block_nodes(document)
        .pop()
        .map(|node| node.kind)
    else {
        return false;
    };
    let (width, source_cells, source_grid) = source.resolve_grid();
    let (destination_width, destination_cells, destination_grid) = destination.resolve_grid();
    if source_grid.is_empty()
        || source_grid.iter().flatten().any(Option::is_none)
        || start.0.saturating_add(source_grid.len()) > destination_grid.len()
        || start.1.saturating_add(width) > destination_width
    {
        return false;
    }
    let mut targets = Vec::with_capacity(source_cells.len());
    for cell in &source_cells {
        let row = start.0 + cell.row_index;
        let col = start.1 + cell.visual_col;
        let Some(target) =
            destination_grid[row][col].and_then(|index| destination_cells.get(index))
        else {
            return false;
        };
        if (
            target.row_index,
            target.visual_col,
            target.rowspan,
            target.colspan,
        ) != (row, col, cell.rowspan, cell.colspan)
        {
            return false;
        }
        targets.push((target.row_index, target.cell_index));
    }
    let mut source_rows = source.rows;
    for (cell, (row, col)) in source_cells.into_iter().zip(targets) {
        let blocks = std::mem::take(&mut source_rows[cell.row_index].cells[cell.cell_index].blocks);
        replace_cell(document, &mut destination.rows[row].cells[col], blocks);
    }
    true
}

fn permutation(length: usize, range: Range<usize>, to: usize) -> Option<Vec<usize>> {
    if range.is_empty()
        || range.end > length
        || to > length
        || (range.start..=range.end).contains(&to)
    {
        return None;
    }
    let mut order: Vec<_> = (0..length).collect();
    move_range(&mut order, range, to);
    let mut positions = vec![0; length];
    for (index, original) in order.into_iter().enumerate() {
        positions[original] = index;
    }
    Some(positions)
}

fn move_range<T>(items: &mut [T], range: Range<usize>, to: usize) {
    if to < range.start {
        items[to..range.end].rotate_right(range.len());
    } else {
        items[range.start..to].rotate_left(range.len());
    }
}

fn survives_move(positions: &[usize], start: usize, span: usize) -> bool {
    positions[start..start + span]
        .windows(2)
        .all(|pair| pair[0] + 1 == pair[1])
}

fn move_rows(table: &mut TableBlock, range: Range<usize>, to: usize) -> bool {
    let Some(positions) = permutation(table.rows.len(), range.clone(), to) else {
        return false;
    };
    if table
        .resolve_grid()
        .1
        .iter()
        .any(|cell| !survives_move(&positions, cell.row_index, cell.rowspan))
    {
        return false;
    }
    move_range(&mut table.rows, range, to);
    true
}

fn move_columns(table: &mut TableBlock, range: Range<usize>, to: usize) -> bool {
    let (width, cells, _) = table.resolve_grid();
    let Some(positions) = permutation(width, range.clone(), to) else {
        return false;
    };
    if cells
        .iter()
        .any(|cell| !survives_move(&positions, cell.visual_col, cell.colspan))
    {
        return false;
    }
    let mut by_row = vec![Vec::new(); table.rows.len()];
    for cell in cells {
        by_row[cell.row_index].push(positions[cell.visual_col]);
    }
    for (row, positions) in table.rows.iter_mut().zip(by_row) {
        let mut cells: Vec<_> = positions
            .into_iter()
            .zip(std::mem::take(&mut row.cells))
            .collect();
        cells.sort_unstable_by_key(|(position, _)| *position);
        row.cells = cells.into_iter().map(|(_, cell)| cell).collect();
    }
    table.columns.resize(width, ColumnSpec::Auto);
    move_range(&mut table.columns, range, to);
    table.column_kinds = std::mem::take(&mut table.column_kinds)
        .into_iter()
        .filter_map(|(column, kind)| Some((*positions.get(column)?, kind)))
        .collect();
    true
}

fn sort_rows(table: &mut TableBlock, range: Range<usize>, column: usize, descending: bool) -> bool {
    let (width, placements, grid) = table.resolve_grid();
    if range.is_empty()
        || range.end > table.rows.len()
        || column >= width
        || placements.iter().any(|cell| {
            cell.rowspan > 1
                && cell.row_index < range.end
                && cell.row_index + cell.rowspan > range.start
        })
    {
        return false;
    }
    let mut order: Vec<_> = range
        .filter(|&index| !table.rows[index].is_header)
        .map(|index| {
            let key = grid[index][column]
                .and_then(|slot| placements.get(slot))
                .map(|cell| {
                    table.rows[cell.row_index].cells[cell.cell_index]
                        .blocks
                        .iter()
                        .filter_map(|block| block.kind.inline_content())
                        .map(|inline| inline.text.as_str())
                        .collect::<String>()
                })
                .unwrap_or_default();
            (index, key)
        })
        .collect();
    let destination: Vec<_> = order.iter().map(|(index, _)| *index).collect();
    order.sort_by(|(_, left), (_, right)| {
        if descending {
            right.cmp(left)
        } else {
            left.cmp(right)
        }
    });
    if order
        .iter()
        .map(|(index, _)| *index)
        .eq(destination.iter().copied())
    {
        return false;
    }
    let moved: Vec<_> = order
        .into_iter()
        .map(|(index, _)| std::mem::take(&mut table.rows[index].cells))
        .collect();
    for (index, cells) in destination.into_iter().zip(moved) {
        table.rows[index].cells = cells;
    }
    true
}

fn subdivide(
    document: &mut Document,
    table: &mut TableBlock,
    origins: (usize, usize),
    rows: usize,
    columns: usize,
) -> bool {
    if rows == 0 || columns == 0 || rows > 64 || columns > 64 || (rows, columns) == (1, 1) {
        return false;
    }
    let (width, placements, grid) = table.resolve_grid();
    let Some(target) = grid
        .get(origins.0)
        .and_then(|line| line.get(origins.1))
        .and_then(|slot| *slot)
        .and_then(|index| placements.get(index))
        .copied()
    else {
        return false;
    };
    let row_break = target.row_index + target.rowspan;
    let column_break = target.visual_col + target.colspan;
    let extra_rows = rows.saturating_sub(target.rowspan);
    let extra_columns = columns.saturating_sub(target.colspan);
    let map_row = |index| index + if index >= row_break { extra_rows } else { 0 };
    let map_column = |index| {
        index
            + if index >= column_break {
                extra_columns
            } else {
                0
            }
    };
    let mut entries: Vec<Vec<(usize, TableCell)>> = (0..table.rows.len() + extra_rows)
        .map(|_| Vec::new())
        .collect();
    let mut headers = vec![false; entries.len()];
    for (index, row) in table.rows.iter().enumerate() {
        headers[map_row(index)..map_row(index + 1)].fill(row.is_header);
    }
    if placements.iter().any(|cell| {
        map_row(cell.row_index + cell.rowspan) - map_row(cell.row_index) > u16::MAX as usize
            || map_column(cell.visual_col + cell.colspan) - map_column(cell.visual_col)
                > u16::MAX as usize
    }) {
        return false;
    }
    let mut originals: Vec<_> = std::mem::take(&mut table.rows)
        .into_iter()
        .map(|row| row.cells.into_iter().map(Some).collect::<Vec<_>>())
        .collect();
    for placement in placements {
        let Some(mut cell) = originals[placement.row_index][placement.cell_index].take() else {
            continue;
        };
        let row = map_row(placement.row_index);
        let column = map_column(placement.visual_col);
        let row_span = map_row(placement.row_index + placement.rowspan) - row;
        let column_span = map_column(placement.visual_col + placement.colspan) - column;
        if placement != target {
            cell.rowspan = row_span as u16;
            cell.colspan = column_span as u16;
            entries[row].push((column, cell));
            continue;
        }
        let padding_pt = cell.padding_pt;
        let background = cell.background;
        let valign = cell.valign;
        cell.rowspan = (row_span / rows) as u16;
        cell.colspan = (column_span / columns) as u16;
        entries[row].push((column, cell));
        for r in 0..rows {
            for c in 0..columns {
                if r == 0 && c == 0 {
                    continue;
                }
                let block = BlockNode::new(
                    document.alloc_block_id(),
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                );
                entries[row + r * row_span / rows].push((
                    column + c * column_span / columns,
                    TableCell {
                        id: document.alloc_block_id(),
                        rowspan: (((r + 1) * row_span / rows) - (r * row_span / rows)) as u16,
                        colspan: (((c + 1) * column_span / columns) - (c * column_span / columns))
                            as u16,
                        padding_pt,
                        background,
                        valign,
                        blocks: vec![block],
                    },
                ));
            }
        }
    }
    table.rows = entries
        .into_iter()
        .zip(headers)
        .map(|(mut entries, is_header)| {
            entries.sort_unstable_by_key(|entry| entry.0);
            TableRow {
                is_header,
                cells: entries.into_iter().map(|(_, cell)| cell).collect(),
            }
        })
        .collect();
    if extra_columns > 0 {
        table.columns.resize(width, ColumnSpec::Auto);
        let fraction = (extra_columns + 1) as f32;
        let shared = match table.columns[column_break - 1] {
            ColumnSpec::FixedPt(width) => ColumnSpec::FixedPt(width / fraction),
            ColumnSpec::Ratio(weight) => ColumnSpec::Ratio(weight / fraction),
            ColumnSpec::Auto => ColumnSpec::Auto,
        };
        table.columns.splice(
            column_break - 1..column_break,
            std::iter::repeat_n(shared, extra_columns + 1),
        );
        let split_kind = table.column_kinds.get(&(column_break - 1)).copied();
        table.column_kinds = std::mem::take(&mut table.column_kinds)
            .into_iter()
            .map(|(column, kind)| {
                (
                    column
                        + if column >= column_break {
                            extra_columns
                        } else {
                            0
                        },
                    kind,
                )
            })
            .collect();
        if let Some(kind) = split_kind {
            table
                .column_kinds
                .extend((column_break..column_break + extra_columns).map(|column| (column, kind)));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdividing_cell_keeps_neighbors_and_restores_grid_on_undo() {
        let mut document = Document::new();
        document.blocks =
            ClipboardFragment::from_html("<table><tr><td>A</td><td>B</td></tr></table>")
                .to_block_nodes(&mut document);
        let id = document.blocks[0].id;
        let table = document.blocks[0].kind.table_block().unwrap();
        let right_id = table.rows[0].cells[1].id;
        let selection = DocumentSelection::TableCells {
            table_id: id,
            anchor_cell: (0, 0),
            focus_cell: (0, 0),
        };
        let transaction = document
            .build_grid_transaction(
                &GridCommand::Subdivide {
                    rows: 2,
                    columns: 2,
                },
                &selection,
            )
            .unwrap();
        let applied = document.apply(transaction);
        let table = document.blocks[0].kind.table_block().unwrap();
        assert_eq!(
            table
                .resolve_grid()
                .2
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            [3, 3]
        );
        assert_eq!(
            (table.rows[0].cells[2].id, table.rows[0].cells[2].rowspan),
            (right_id, 2)
        );
        document.apply(applied.inverse);
        let table = document.blocks[0].kind.table_block().unwrap();
        assert_eq!((table.rows.len(), table.rows[0].cells[1].id), (1, right_id));
    }
}
