use super::{
    CommandOutcome, CommandSource, Context, DocumentSelection, HistoryMergeGroup, RichDocumentView,
};
use rich_model::{ClipboardBlock, ClipboardFragment, ClipboardTable, GridCommand, Step};

impl RichDocumentView {
    pub(super) fn execute_grid_command(
        &mut self,
        command: &GridCommand,
        context: &mut Context<Self>,
    ) -> CommandOutcome {
        if let GridCommand::PasteHtml { html } = command {
            let mut fragment = ClipboardFragment::from_html(html);
            let definitions = super::editor_clipboard::import_component_definitions(
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
                    super::table_cell_of(focus).is_some() && text.contains('\t')
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
