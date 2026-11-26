//! Table structure commands and rectangular grid editing on [`RichDocumentView`].

use super::*;
use crate::model::{ClipboardTable, GridCommand};

impl RichDocumentView {
    pub(super) fn execute_set_table_columns(
        &mut self,
        table_id: BlockId,
        columns: Vec<ColumnSpec>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(transaction) = self.document.build_set_table_columns_transaction(
            table_id,
            columns,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_insert_table_row(
        &mut self,
        after: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let target_row = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                if after {
                    anchor_cell.0.max(focus_cell.0)
                } else {
                    anchor_cell.0.min(focus_cell.0)
                }
            }
            _ => context.row,
        };
        let Some(transaction) = self.document.build_insert_table_row_transaction(
            context.table_id,
            target_row,
            after,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_delete_table_row(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let row_range = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                let start = anchor_cell.0.min(focus_cell.0);
                let end = anchor_cell.0.max(focus_cell.0) + 1;
                start..end
            }
            _ => context.row..(context.row + 1),
        };
        let Some(transaction) = self.document.build_delete_table_row_transaction(
            context.table_id,
            row_range,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_insert_table_column(
        &mut self,
        after: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let target_col = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                if after {
                    anchor_cell.1.max(focus_cell.1)
                } else {
                    anchor_cell.1.min(focus_cell.1)
                }
            }
            _ => context.visual_col,
        };
        let Some(transaction) = self.document.build_insert_table_column_transaction(
            context.table_id,
            target_col,
            after,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_delete_table_column(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let col_range = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                let start = anchor_cell.1.min(focus_cell.1);
                let end = anchor_cell.1.max(focus_cell.1) + 1;
                start..end
            }
            _ => context.visual_col..(context.visual_col + 1),
        };
        let Some(transaction) = self.document.build_delete_table_column_transaction(
            context.table_id,
            col_range,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_merge_table_cells(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(DocumentSelection::TableCells {
            table_id,
            anchor_cell,
            focus_cell,
        }) = self.selection.clone()
        else {
            return CommandOutcome::NoOp;
        };
        let Some(transaction) = self.document.build_merge_table_cells_transaction(
            table_id,
            anchor_cell,
            focus_cell,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_split_table_cell(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let Some(transaction) = self.document.build_split_table_cell_transaction(
            context.table_id,
            context.row,
            context.cell_index,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    pub(super) fn execute_navigate_table_cell(
        &mut self,
        forward: bool,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if source == CommandSource::User && !self.policy.can_select() {
            return CommandOutcome::RejectedByPolicy;
        }
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        match self.document.next_table_cell_pos(
            context.table_id,
            context.row,
            context.cell_index,
            forward,
        ) {
            Some(crate::model::TableCellStep::MoveTo(pos)) => {
                let next_selection = DocumentSelection::caret(pos, Affinity::Downstream);
                self.update_selection(Some(next_selection), cx);
                self.reset_caret_blink(cx);
                self.ensure_caret_visible();
                CommandOutcome::Executed
            }
            Some(crate::model::TableCellStep::AtBoundary)
                if forward
                    && self.policy.editing.enabled
                    && (source == CommandSource::Programmatic
                        || self.policy.can_edit_table_structure()) =>
            {
                // Tab past the final cell appends a new row below and focuses its first cell.
                self.execute_insert_table_row(true, cx)
            }
            _ => CommandOutcome::NoOp,
        }
    }
}

impl RichDocumentView {
    pub(super) fn execute_grid_command(
        &mut self,
        command: &GridCommand,
        context: &mut Context<Self>,
    ) -> CommandOutcome {
        if let GridCommand::PasteHtml { html } = command {
            let mut fragment = ClipboardFragment::from_html(html);
            let definitions = super::clipboard::import_component_definitions(
                &self.document.components,
                &mut fragment,
            )
            .map(|components| Step::SetComponents { components });
            let [ClipboardBlock::Table(table)] = fragment.blocks.as_slice() else {
                return CommandOutcome::NoOp;
            };
            return self.execute_grid_paste(table, definitions, context);
        }
        let Some(selection) = self.selection.as_ref() else {
            return CommandOutcome::NoOp;
        };
        let Some(transaction) = self.document.build_grid_transaction(command, selection) else {
            return CommandOutcome::NoOp;
        };
        self.ime_preedit = None;
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), context);
        CommandOutcome::Executed
    }

    pub(super) fn execute_grid_paste(
        &mut self,
        source: &ClipboardTable,
        definitions: Option<Step>,
        context: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(selection) = self.selection.as_ref() else {
            return CommandOutcome::NoOp;
        };
        let Some(mut transaction) = self
            .document
            .build_grid_paste_transaction(source, selection)
        else {
            return CommandOutcome::NoOp;
        };
        transaction.steps.extend(definitions);
        self.ime_preedit = None;
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), context);
        CommandOutcome::Executed
    }

    pub(super) fn execute_paste_text(
        &mut self,
        text: &str,
        source: CommandSource,
        context: &mut Context<Self>,
    ) -> CommandOutcome {
        self.history.break_coalescing();
        let in_cell = self
            .selection
            .as_ref()
            .is_some_and(|selection| match selection {
                DocumentSelection::TableCells { .. } => true,
                DocumentSelection::Text { focus, .. } => {
                    super::table_cell_of(focus).is_some()
                        && text.contains('\t')
                        && !focus
                            .leaf_block_id()
                            .is_some_and(|id| self.document.is_editable_slot(id))
                }
            });
        if in_cell {
            if source == CommandSource::User && !self.policy.can_edit_grid() {
                return CommandOutcome::RejectedByPolicy;
            }
            return self.execute_grid_command(
                &GridCommand::PasteTsv {
                    text: text.to_owned(),
                },
                context,
            );
        }
        self.execute_insert_text_inner(text, context)
    }
}
