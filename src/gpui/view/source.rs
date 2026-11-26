use super::*;
use crate::model::{
    BlockKind, BlockNode, BlockStyle, InlineContent, SourceBias, SourceBuffer, SourceChangeSet,
    SourceEdit, SourceEditError, SourceLinePresentation, SourcePresentation,
};

#[derive(Clone, Copy)]
struct SourceSelection {
    anchor: usize,
    focus: usize,
    affinity: Affinity,
}

impl SourceSelection {
    fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            focus: offset,
            affinity: Affinity::Downstream,
        }
    }

    fn range(self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    fn mapped(self, change: &SourceChangeSet) -> Self {
        let map = |offset| {
            change
                .map_offset(offset, SourceBias::After)
                .unwrap_or_else(|| {
                    change
                        .changes
                        .iter()
                        .find(|edit| edit.old.contains(&offset))
                        .map_or(offset, |edit| edit.new.end)
                })
        };
        Self {
            anchor: map(self.anchor),
            focus: map(self.focus),
            ..self
        }
    }
}

struct HistoryEntry {
    edits: Vec<SourceEdit>,
    selection: Option<SourceSelection>,
}

pub(super) struct SourceView {
    buffer: SourceBuffer,
    provider: Option<Box<dyn SourcePresentation>>,
    presentation: Vec<SourceLinePresentation>,
    reveal_all: bool,
    revealed_by_focus: Option<BlockId>,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

impl SourceView {
    fn project(&mut self, change: Option<&SourceChangeSet>) {
        self.presentation = self
            .provider
            .as_mut()
            .map_or_else(Vec::new, |provider| provider.project(&self.buffer, change));
        self.presentation.sort_by_key(|entry| entry.line);
        self.presentation.dedup_by_key(|entry| entry.line);
        self.presentation.retain_mut(|entry| {
            let Some(line) = self.buffer.lines().get(entry.line) else {
                return false;
            };
            let text = &self.buffer.text()[line.clone()];
            entry.hidden.sort_by_key(|range| (range.start, range.end));
            let mut boundaries = text
                .grapheme_indices(true)
                .map(|(offset, _)| offset)
                .chain(std::iter::once(text.len()))
                .peekable();
            let mut end = None;
            entry.hidden.retain_mut(|range| {
                if range.start >= range.end || end.is_some_and(|previous| previous > range.start) {
                    return false;
                }
                let mut candidate = boundaries.clone();
                let mut is_boundary = |target| {
                    while candidate.peek().is_some_and(|&offset| offset < target) {
                        candidate.next();
                    }
                    candidate.peek() == Some(&target)
                };
                if !is_boundary(range.start) || !is_boundary(range.end) {
                    return false;
                }
                boundaries = candidate;
                end = Some(range.end);
                true
            });
            true
        });
    }

    fn line_presentation(&self, index: usize) -> Option<&SourceLinePresentation> {
        let at = self
            .presentation
            .binary_search_by_key(&index, |entry| entry.line)
            .ok()?;
        self.presentation.get(at)
    }
}

impl RichDocumentView {
    /// Opens exact UTF-8 text in the document editor. Save with `source_buffer().text()` rather
    /// than exporting the derived rich document. Start with plain text or attach a presentation.
    pub fn new_source(source: SourceBuffer, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(Document::new(), cx).with_policy(InteractionPolicy::editable());
        view.set_source_buffer(source, cx);
        view
    }

    /// Replaces the current document with a new exact source buffer and clears editor history.
    pub fn set_source_buffer(&mut self, source: SourceBuffer, cx: &mut Context<Self>) {
        let mut document = Document::new();
        document.config = self.document.config.clone();
        document.stylesheet = self.document.stylesheet.clone();
        self.replace_document(document, cx);
        self.source = Some(Box::new(SourceView {
            buffer: source,
            provider: None,
            presentation: Vec::new(),
            reveal_all: false,
            revealed_by_focus: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }));
        self.sync_source_lines();
        self.resync_layout_with_anchor(None);
        cx.notify();
    }

    /// Authoritative file text and edit revision; the rich block tree is a derived display only.
    pub fn source_buffer(&self) -> Option<&SourceBuffer> {
        self.source.as_ref().map(|source| &source.buffer)
    }

    /// Installs a presentation provider without editing the source or affecting its history.
    pub fn set_source_presentation(
        &mut self,
        provider: Option<Box<dyn SourcePresentation>>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = &mut self.source else {
            return;
        };
        source.provider = provider;
        source.project(None);
        let scroll = self.flow.capture_anchor(self.scroll_y);
        self.sync_source_lines();
        self.resync_layout_with_anchor(scroll);
        cx.notify();
    }

    /// Switches between projected and raw presentation without creating an undo entry.
    pub fn reveal_source(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let Some(source) = &mut self.source else {
            return;
        };
        if source.reveal_all != reveal {
            source.reveal_all = reveal;
            let scroll = self.flow.capture_anchor(self.scroll_y);
            self.sync_source_lines();
            self.resync_layout_with_anchor(scroll);
            cx.notify();
        }
    }

    /// Whether the view is showing the raw source instead of its projection.
    pub fn source_syntax_revealed(&self) -> Option<bool> {
        self.source.as_ref().map(|source| source.reveal_all)
    }

    /// Active source selection as ordered file byte offsets, including hidden bytes and separators.
    pub fn source_selection(&self) -> Option<Range<usize>> {
        self.current_source_selection().map(SourceSelection::range)
    }

    /// Places the native selection at file byte offsets without creating an undo entry.
    pub fn select_source(
        &mut self,
        anchor: usize,
        focus: usize,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let (Some(anchor), Some(focus)) = (self.source_doc_pos(anchor), self.source_doc_pos(focus))
        else {
            return CommandOutcome::NoOp;
        };
        self.dispatch_command(
            RichCommand::SetSelection(Some(Box::new(DocumentSelection::Text {
                anchor,
                focus,
                affinity: Affinity::Downstream,
                goal_x: None,
                stored_marks: None,
            }))),
            cx,
        )
    }

    /// Applies a batch of source edits as one undoable operation. A stale revision is rejected.
    /// Like programmatic rich transactions, this deliberately bypasses the user interaction policy.
    pub fn apply_source_edits(
        &mut self,
        revision: u64,
        edits: Vec<SourceEdit>,
        cx: &mut Context<Self>,
    ) -> Result<bool, SourceEditError> {
        self.edit_source(revision, edits, None, cx)
    }

    fn source_offset(&self, position: &DocPos) -> Option<usize> {
        let buffer = self.source_buffer()?;
        let index = self.document.top_level_index(position.leaf_block_id()?)?;
        let line = buffer.lines().get(index)?;
        let offset = line.start + position.offset.min(line.len());
        buffer.text().is_char_boundary(offset).then_some(offset)
    }

    fn source_doc_pos(&self, offset: usize) -> Option<DocPos> {
        let (index, local) = self.source_buffer()?.line_position(offset)?;
        Some(DocPos::top_level(
            self.document.blocks.get(index)?.id,
            local,
        ))
    }

    fn current_source_selection(&self) -> Option<SourceSelection> {
        let DocumentSelection::Text {
            anchor,
            focus,
            affinity,
            ..
        } = self.selection.as_ref()?
        else {
            return None;
        };
        Some(SourceSelection {
            anchor: self.source_offset(anchor)?,
            focus: self.source_offset(focus)?,
            affinity: *affinity,
        })
    }

    fn sync_source_lines(&mut self) -> ChangeSet {
        let Some(source) = &mut self.source else {
            return ChangeSet::default();
        };
        let source_lines = source.buffer.lines();
        let text = source.buffer.text();
        let previous = &self.document.blocks;
        let same_line = |block: &BlockNode, line: &Range<usize>| {
            block
                .kind
                .inline_content()
                .is_some_and(|content| content.text == text[line.clone()])
        };
        let prefix = previous
            .iter()
            .zip(source_lines)
            .take_while(|(block, line)| same_line(block, line))
            .count();
        let suffix = previous[prefix..]
            .iter()
            .rev()
            .zip(source_lines[prefix..].iter().rev())
            .take_while(|(block, line)| same_line(block, line))
            .count();
        let old_end = previous.len() - suffix;
        let new_end = source_lines.len() - suffix;
        let reuse_end = old_end.min(new_end);
        let mut changes = ChangeSet {
            structure_changed: old_end != new_end,
            ..Default::default()
        };
        for (block, line) in self.document.blocks[prefix..reuse_end]
            .iter_mut()
            .zip(&source_lines[prefix..reuse_end])
        {
            let mut content = InlineContent::empty();
            content.push_text(&text[line.clone()], Default::default());
            block.kind = BlockKind::paragraph(content);
            block.bump_version();
            changes.dirty_blocks.push(block.id);
        }
        for block in &self.document.blocks[reuse_end..old_end] {
            self.flow
                .set_source_hidden_ranges(&self.document, block.id, &[]);
            self.flow.cache.invalidate_block(block.id);
        }
        if new_end > reuse_end {
            let added: Vec<_> = source_lines[reuse_end..new_end]
                .iter()
                .map(|line| {
                    let mut content = InlineContent::empty();
                    content.push_text(&text[line.clone()], Default::default());
                    let id = self.document.alloc_block_id();
                    changes.dirty_blocks.push(id);
                    BlockNode::new(id, BlockKind::paragraph(content), BlockStyle::default())
                })
                .collect();
            self.document.blocks.splice(reuse_end..old_end, added);
        } else {
            self.document.blocks.drain(reuse_end..old_end);
        }
        let mut entries = source.presentation.iter().peekable();
        for index in 0..self.document.blocks.len() {
            let presentation = entries.next_if(|entry| entry.line == index);
            let block = &mut self.document.blocks[index];
            let style = if source.reveal_all {
                BlockStyle::default()
            } else {
                presentation
                    .map(|entry| entry.style.clone())
                    .unwrap_or_default()
            };
            if block.style != style {
                block.style = style;
                block.bump_version();
            }
            let id = block.id;
            let ranges = if source.reveal_all || source.revealed_by_focus == Some(id) {
                &[][..]
            } else {
                presentation.map_or(&[][..], |entry| entry.hidden.as_slice())
            };
            self.flow
                .set_source_hidden_ranges(&self.document, id, ranges);
        }
        changes
    }

    pub(crate) fn refresh_source_reveal(&mut self) {
        let Some(source) = &mut self.source else {
            return;
        };
        let focus = match &self.selection {
            Some(DocumentSelection::Text { focus, .. }) => Some(focus),
            _ => None,
        };
        let next = self
            .ime_preedit
            .as_ref()
            .map(|preedit| preedit.block_id)
            .or_else(|| {
                let focus = focus?;
                let id = focus.leaf_block_id()?;
                let index = self.document.top_level_index(id)?;
                let line_len = source.buffer.lines().get(index)?.len();
                source
                    .line_presentation(index)?
                    .hidden
                    .iter()
                    .any(|range| {
                        if range.end == line_len && range.start != 0 {
                            range.start < focus.offset && focus.offset <= range.end
                        } else {
                            range.contains(&focus.offset)
                        }
                    })
                    .then_some(id)
            });
        let previous = std::mem::replace(&mut source.revealed_by_focus, next);
        if previous == next || source.reveal_all {
            return;
        }
        if let Some(previous) = previous
            && let Some(index) = self.document.top_level_index(previous)
        {
            let ranges = source
                .line_presentation(index)
                .map_or(&[][..], |entry| entry.hidden.as_slice());
            self.flow
                .set_source_hidden_ranges(&self.document, previous, ranges);
        }
        if let Some(next) = next {
            self.flow
                .set_source_hidden_ranges(&self.document, next, &[]);
        }
    }

    fn finish_source_edit(
        &mut self,
        change: SourceChangeSet,
        selection: Option<SourceSelection>,
        cx: &mut Context<Self>,
    ) {
        let scroll = self.flow.capture_anchor(self.scroll_y);
        self.ime_preedit = None;
        if let Some(source) = &mut self.source {
            source.project(Some(&change));
        }
        let changes = self.sync_source_lines();
        let selection = selection.and_then(|selection| {
            Some(DocumentSelection::Text {
                anchor: self.source_doc_pos(selection.anchor)?,
                focus: self.source_doc_pos(selection.focus)?,
                affinity: selection.affinity,
                goal_x: None,
                stored_marks: None,
            })
        });
        self.update_selection(selection, cx);
        self.refresh_source_reveal();
        self.resync_layout_with_anchor(scroll);
        self.pending_reveal = self.active_text_selection().map(|(_, focus, _)| focus);
        self.notify_transaction_applied(changes, cx);
        self.reset_caret_blink(cx);
        cx.emit(RichDocumentEvent::SourceEdited { change });
        cx.notify();
    }

    fn edit_source(
        &mut self,
        revision: u64,
        edits: Vec<SourceEdit>,
        caret_after: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Result<bool, SourceEditError> {
        let previous = self.current_source_selection();
        let source = self
            .source
            .as_mut()
            .ok_or(SourceEditError::NotSourceBacked)?;
        let Some(applied) = source.buffer.apply(revision, edits)? else {
            return Ok(false);
        };
        let next = caret_after
            .map(SourceSelection::caret)
            .or_else(|| previous.map(|selection| selection.mapped(&applied.changes)));
        source.undo.push(HistoryEntry {
            edits: applied.inverse,
            selection: previous,
        });
        source.redo.clear();
        self.finish_source_edit(applied.changes, next, cx);
        Ok(true)
    }

    fn source_history(&mut self, redo: bool, cx: &mut Context<Self>) -> CommandOutcome {
        let previous = self.current_source_selection();
        let Some(source) = &mut self.source else {
            return CommandOutcome::NoOp;
        };
        let (from, to) = if redo {
            (&mut source.redo, &mut source.undo)
        } else {
            (&mut source.undo, &mut source.redo)
        };
        let Some(entry) = from.pop() else {
            return CommandOutcome::NoOp;
        };
        match source.buffer.apply(source.buffer.revision(), entry.edits) {
            Ok(Some(applied)) => {
                to.push(HistoryEntry {
                    edits: applied.inverse,
                    selection: previous,
                });
                self.finish_source_edit(applied.changes, entry.selection, cx);
                CommandOutcome::Executed
            }
            Ok(None) => CommandOutcome::NoOp,
            Err(error) => {
                eprintln!("gpui-rich source history: {error}");
                CommandOutcome::NoOp
            }
        }
    }

    fn source_replace(
        &mut self,
        revision: u64,
        range: Range<usize>,
        text: String,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let caret = range.start + text.len();
        let replaced_selection = !range.is_empty();
        match self.edit_source(revision, vec![SourceEdit { range, text }], Some(caret), cx) {
            Ok(true) => CommandOutcome::Executed,
            Ok(false) if replaced_selection => self.select_source(caret, caret, cx),
            Ok(false) => CommandOutcome::NoOp,
            Err(error) => {
                eprintln!("gpui-rich source edit: {error}");
                CommandOutcome::NoOp
            }
        }
    }

    pub(super) fn execute_source_command(
        &mut self,
        command: &RichCommand,
        cx: &mut Context<Self>,
    ) -> Option<CommandOutcome> {
        let source = self.source_buffer()?;
        let selection = self.current_source_selection();
        let revision = source.revision();
        let selection_range = || selection.map(SourceSelection::range).unwrap_or(0..0);
        let outcome = match command {
            RichCommand::Undo => self.source_history(false, cx),
            RichCommand::Redo => self.source_history(true, cx),
            RichCommand::InsertText(text) | RichCommand::PasteText(text) => {
                self.source_replace(revision, selection_range(), text.clone(), cx)
            }
            RichCommand::PasteHtml { html, plain_text } => {
                let text = plain_text
                    .clone()
                    .unwrap_or_else(|| ClipboardFragment::from_html(html).to_plain_text());
                self.source_replace(revision, selection_range(), text, cx)
            }
            RichCommand::PasteFragment {
                fragment,
                plain_text,
                ..
            } => {
                let text = plain_text
                    .clone()
                    .unwrap_or_else(|| fragment.to_plain_text());
                self.source_replace(revision, selection_range(), text, cx)
            }
            RichCommand::SplitBlock | RichCommand::InsertSoftBreak => {
                let range = selection_range();
                let line = source
                    .line_position(range.start)
                    .map_or(0, |(line, _)| line);
                let separator = source.line_break_at(line).to_owned();
                self.source_replace(revision, range, separator, cx)
            }
            RichCommand::Delete { direction, unit } => {
                let Some(selection) = selection else {
                    return Some(CommandOutcome::NoOp);
                };
                let mut range = selection.range();
                if range.is_empty() {
                    let offset = range.start;
                    let text = source.text();
                    use input::{
                        next_grapheme_boundary, next_word_boundary, previous_grapheme_boundary,
                        previous_word_boundary,
                    };
                    let backward = matches!(direction, MoveDirection::Backward | MoveDirection::Up);
                    let boundary = match (unit, backward) {
                        (MoveUnit::Word, true) => previous_word_boundary(text, offset),
                        (MoveUnit::Word, false) => next_word_boundary(text, offset),
                        (MoveUnit::LineBoundary, _) => self
                            .source_doc_pos(offset)
                            .and_then(|pos| {
                                let (local, _) = self.flow.cached_line_boundary(
                                    &pos,
                                    selection.affinity,
                                    !backward,
                                )?;
                                Some(offset - pos.offset + local)
                            })
                            .unwrap_or(offset),
                        (MoveUnit::DocumentBoundary, true) => 0,
                        (MoveUnit::DocumentBoundary, false) => text.len(),
                        (_, true) => previous_grapheme_boundary(text, offset),
                        (_, false) => next_grapheme_boundary(text, offset),
                    };
                    range = boundary.min(offset)..boundary.max(offset);
                }
                self.source_replace(revision, range, String::new(), cx)
            }
            RichCommand::Indent => {
                self.source_replace(revision, selection_range(), "\t".into(), cx)
            }
            command if command.is_document_mutation() => {
                let source = self.source.as_mut()?;
                let Some(edits) = source.provider.as_mut().and_then(|provider| {
                    provider.plan_edit(&source.buffer, selection_range(), command)
                }) else {
                    return Some(CommandOutcome::RejectedByPolicy);
                };
                match self.apply_source_edits(revision, edits, cx) {
                    Ok(true) => CommandOutcome::Executed,
                    Ok(false) => CommandOutcome::NoOp,
                    Err(error) => {
                        eprintln!("gpui-rich source extension edit: {error}");
                        CommandOutcome::NoOp
                    }
                }
            }
            _ => return None,
        };
        Some(outcome)
    }
}
