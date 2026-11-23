use super::*;
use rich_model::navigation::{
    NavigationCommand, NavigationDirection, SearchMatch, SearchState, find_matches,
    navigate_target, replace_matches, validate_doc_pos,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct NavigationState {
    pub search: SearchState,
    by_block: HashMap<BlockId, Vec<usize>>,
}

impl NavigationState {
    fn index_matches(&mut self) {
        self.by_block.clear();
        for (index, fragment) in self.search.matches.iter().enumerate() {
            self.by_block
                .entry(fragment.block_id)
                .or_default()
                .push(index);
        }
    }

    pub(crate) fn write_decorations(
        &self,
        block_id: BlockId,
        writer: &mut rich_model::DecorationWriter<'_>,
    ) {
        for &index in self.by_block.get(&block_id).into_iter().flatten() {
            let color = if self.search.active == Some(index) {
                RgbaColor::rgb(0xff, 0xd5, 0x67)
            } else {
                RgbaColor::rgb(0xff, 0xee, 0xa0)
            };
            let range = rich_model::BlockByteRange::from_usize_range(
                self.search.matches[index].range.clone(),
            );
            let style = rich_model::InlineDecorationStyle::new()
                .with_background(color)
                .with_priority(20);
            if let Err(error) = writer.push_inline(range, style) {
                debug_assert!(false, "search match decoration rejected: {error}");
            }
        }
    }
}

impl RichDocumentView {
    pub fn search_state(&self) -> &SearchState {
        &self.navigation.search
    }

    pub(super) fn execute_navigation_command_inner(
        &mut self,
        command: &NavigationCommand,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        match command {
            NavigationCommand::GoTo(target) => {
                let current = self.selection.as_ref().and_then(|sel| match sel {
                    DocumentSelection::Text { focus, .. } => Some(focus),
                    DocumentSelection::TableCells { .. } => None,
                });
                let Some(pos) = navigate_target(&self.document, target, current) else {
                    return CommandOutcome::NoOp;
                };
                self.select_navigation_position(pos, cx)
            }
            NavigationCommand::Find { query, options } => {
                self.navigation.search = SearchState {
                    query: query.clone(),
                    options: options.clone(),
                    matches: find_matches(&self.document, query, options.clone()),
                    active: None,
                };
                self.navigation.index_matches();
                let Some(found) = self
                    .navigation
                    .search
                    .advance(NavigationDirection::Next)
                    .cloned()
                else {
                    cx.notify();
                    return CommandOutcome::NoOp;
                };
                self.select_search_match(&found, cx)
            }
            NavigationCommand::FindNext | NavigationCommand::FindPrevious => {
                let direction = if matches!(command, NavigationCommand::FindNext) {
                    NavigationDirection::Next
                } else {
                    NavigationDirection::Previous
                };
                let Some(found) = self.navigation.search.advance(direction).cloned() else {
                    return CommandOutcome::NoOp;
                };
                self.select_search_match(&found, cx)
            }
            NavigationCommand::Replace { replacement } => {
                let Some(active) = self.navigation.search.active else {
                    return CommandOutcome::NoOp;
                };
                let Some(found) = self.navigation.search.matches.get(active).cloned() else {
                    return CommandOutcome::NoOp;
                };
                let caret = self
                    .document
                    .doc_pos_for_block(found.block_id, found.range.start + replacement.len());
                let tx = replace_matches(&self.document, &[found], replacement);
                self.apply_search_replacement(tx, caret, cx)
            }
            NavigationCommand::ReplaceAll { replacement } => {
                let tx =
                    replace_matches(&self.document, &self.navigation.search.matches, replacement);
                let caret = tx.steps.iter().rev().find_map(|step| {
                    let Step::DeleteRange { block_id, range } = step else {
                        return None;
                    };
                    let position = self.document.doc_pos_for_block(*block_id, range.start)?;
                    position_is_visible(&self.document, &position).then_some(position)
                });
                self.apply_search_replacement(tx, caret, cx)
            }
        }
    }

    fn apply_search_replacement(
        &mut self,
        transaction: Transaction,
        caret: Option<DocPos>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if transaction.steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_after = caret
            .clone()
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));
        let transaction = transaction.with_selections(self.selection.clone(), selection_after);
        self.apply_transaction(transaction, true, cx);
        self.pending_reveal = caret;
        CommandOutcome::Executed
    }

    pub(super) fn jump_to_position(
        &mut self,
        pos: &DocPos,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(pos) = validate_doc_pos(&self.document, pos) else {
            return CommandOutcome::NoOp;
        };
        if !position_is_visible(&self.document, &pos) {
            return CommandOutcome::TargetHidden;
        }
        self.select_navigation_position(pos, cx)
    }

    fn select_navigation_position(
        &mut self,
        pos: DocPos,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if !position_is_visible(&self.document, &pos) {
            return CommandOutcome::TargetHidden;
        }
        let selection = DocumentSelection::caret(pos.clone(), Affinity::Downstream);
        self.history.break_coalescing();
        self.ime_preedit = None;
        self.update_selection(Some(selection), cx);
        self.reset_caret_blink(cx);
        self.pending_reveal = Some(pos);
        cx.notify();
        CommandOutcome::Executed
    }

    fn select_search_match(
        &mut self,
        found: &SearchMatch,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(start) = self
            .document
            .doc_pos_for_block(found.block_id, found.range.start)
        else {
            return CommandOutcome::NoOp;
        };
        let Some(end) = self
            .document
            .doc_pos_for_block(found.block_id, found.range.end)
        else {
            return CommandOutcome::NoOp;
        };
        if !position_is_visible(&self.document, &start) {
            return CommandOutcome::TargetHidden;
        }
        self.history.break_coalescing();
        self.ime_preedit = None;
        self.update_selection(
            Some(DocumentSelection::Text {
                anchor: start.clone(),
                focus: end,
                affinity: Affinity::Downstream,
                goal_x: None,
                stored_marks: None,
            }),
            cx,
        );
        self.reset_caret_blink(cx);
        self.pending_reveal = Some(start);
        cx.notify();
        CommandOutcome::Executed
    }

    pub(super) fn refresh_search_matches(&mut self) {
        if self.navigation.search.query.is_empty() {
            return;
        }
        self.navigation.search.matches = find_matches(
            &self.document,
            &self.navigation.search.query,
            self.navigation.search.options.clone(),
        );
        self.navigation.search.active = None;
        self.navigation.index_matches();
    }
}

fn position_is_visible(document: &Document, pos: &DocPos) -> bool {
    let Some(top_id) = pos.top_level_block_id() else {
        return false;
    };
    document.top_level_index(top_id).is_some_and(|index| {
        document.top_level_visibility().nth(index).unwrap_or(false)
    })
}
