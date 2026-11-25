//! Command dispatch, transaction application, text/formatting edits, paste, and style queries.

use super::*;

impl RichDocumentView {
    /// Sets document or paper margins through [`RichCommand::SetMargins`].
    pub fn set_margins(&mut self, margins_pt: Edges, cx: &mut Context<Self>) -> CommandOutcome {
        self.dispatch_command_with_source(
            RichCommand::SetMargins(margins_pt),
            CommandSource::Programmatic,
            cx,
        )
    }

    /// Dispatches a user-initiated [`RichCommand`] through middleware, policy checks, and events.
    pub fn dispatch_command(
        &mut self,
        command: RichCommand,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.dispatch_command_with_source(command, CommandSource::User, cx)
    }

    /// Dispatches a programmatic [`RichCommand`] bypassing user-only [`InteractionPolicy`] gates
    /// while still executing middleware, transactions, layout sync, and host events.
    pub fn dispatch_programmatic_command(
        &mut self,
        command: RichCommand,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.dispatch_command_with_source(command, CommandSource::Programmatic, cx)
    }

    /// Central command execution seam for toolbars, GPUI actions, mouse interactions,
    /// command palettes, programmatic APIs, editing mutations, and typed extension commands.
    pub fn dispatch_command_with_source(
        &mut self,
        command: RichCommand,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let mut current_command = command;
        let mut resolution = self.extensions.resolve_command(&current_command);

        for (_, middleware) in &mut self.middleware {
            let resolved = ResolvedCommand::new(&current_command, resolution);
            match middleware.before_command(
                &resolved,
                source,
                &self.document,
                self.selection.as_ref(),
            ) {
                CommandDecision::Continue => {}
                CommandDecision::Cancel => {
                    cx.emit(RichDocumentEvent::CommandExecuted {
                        command_id: current_command.command_id(),
                        effect: current_command.effect(),
                        resolution,
                        command: current_command,
                        source,
                        outcome: CommandOutcome::Cancelled,
                    });
                    return CommandOutcome::Cancelled;
                }
                CommandDecision::Replace(replacement) => {
                    current_command = *replacement;
                    resolution = self.extensions.resolve_command(&current_command);
                }
            }
        }

        match resolution {
            CommandResolution::UnregisteredExtension => {
                let outcome = CommandOutcome::UnregisteredCommand;
                cx.emit(RichDocumentEvent::CommandExecuted {
                    command_id: current_command.command_id(),
                    effect: current_command.effect(),
                    resolution,
                    command: current_command,
                    source,
                    outcome,
                });
                return outcome;
            }
            CommandResolution::MismatchedExtension => {
                let outcome = CommandOutcome::MismatchedCommandType;
                cx.emit(RichDocumentEvent::CommandExecuted {
                    command_id: current_command.command_id(),
                    effect: current_command.effect(),
                    resolution,
                    command: current_command,
                    source,
                    outcome,
                });
                return outcome;
            }
            CommandResolution::BuiltIn | CommandResolution::RegisteredExtension => {}
        }

        if let Some(rejection) = self.policy.check_command(&current_command, source) {
            cx.emit(RichDocumentEvent::CommandExecuted {
                command_id: current_command.command_id(),
                effect: current_command.effect(),
                resolution,
                command: current_command,
                source,
                outcome: rejection,
            });
            return rejection;
        }

        if !matches!(
            current_command,
            RichCommand::InsertText(_)
                | RichCommand::Delete {
                    direction: MoveDirection::Backward,
                    unit: MoveUnit::Grapheme
                }
        ) {
            self.pending_input_rule_undo = None;
        }
        let outcome = self.execute_command(&current_command, source, cx);
        if source == CommandSource::User {
            match &current_command {
                RichCommand::InsertText(_)
                | RichCommand::Delete {
                    direction: MoveDirection::Backward,
                    unit: MoveUnit::Grapheme,
                } => {
                    self.refresh_completion_after_edit(cx);
                }
                RichCommand::SetSelection(_)
                | RichCommand::MoveCaret { .. }
                | RichCommand::Undo
                | RichCommand::Redo
                | RichCommand::PasteText(_)
                | RichCommand::PasteHtml { .. }
                | RichCommand::PasteFragment { .. }
                | RichCommand::SplitBlock
                | RichCommand::Delete { .. } => {
                    self.reconcile_completion_with_selection(cx);
                }
                _ => {}
            }
        }
        cx.emit(RichDocumentEvent::CommandExecuted {
            command_id: current_command.command_id(),
            effect: current_command.effect(),
            resolution,
            command: current_command,
            source,
            outcome,
        });
        outcome
    }

    /// Applies `transaction` to the document, synchronizes scroll-anchored layout, updates history
    /// when requested, invokes `after_transaction` middleware, and emits host events.
    pub fn apply_transaction(
        &mut self,
        transaction: Transaction,
        record_history: bool,
        cx: &mut Context<Self>,
    ) -> ChangeSet {
        self.apply_transaction_grouped(
            transaction,
            if record_history {
                Some(HistoryMergeGroup::Atomic)
            } else {
                None
            },
            cx,
        )
    }

    /// Applies `transaction` with an optional [`HistoryMergeGroup`] for typing/deletion coalescing.
    pub fn apply_transaction_grouped(
        &mut self,
        transaction: Transaction,
        history_group: Option<HistoryMergeGroup>,
        cx: &mut Context<Self>,
    ) -> ChangeSet {
        let anchor = self.flow.capture_anchor(self.scroll_y);
        let next_selection = transaction.selection_after.clone();
        let applied = self.document.apply(transaction);
        if applied.change_set.dirty_blocks.is_empty() && !applied.change_set.structure_changed {
            return applied.change_set;
        }

        if let Some(group) = history_group {
            self.history.push_inverse_grouped(applied.inverse, group);
        }
        let had_next_selection = next_selection.is_some();
        if let Some(selection) = next_selection {
            self.update_selection(Some(selection), cx);
        }

        self.resync_layout_with_anchor(anchor);
        self.notify_transaction_applied(applied.change_set.clone(), cx);
        self.reset_caret_blink(cx);
        // Avoid jumping scroll back to a stale parent caret while focus sits inside a hosted child.
        if self.was_focused || had_next_selection {
            self.ensure_caret_visible();
        }
        cx.notify();
        applied.change_set
    }

    fn execute_command(
        &mut self,
        command: &RichCommand,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        match command {
            RichCommand::SetHorizontalLayout(layout) => {
                if self.document.config.horizontal_layout == *layout {
                    return CommandOutcome::NoOp;
                }
                let anchor = self.flow.capture_anchor(self.scroll_y);
                self.document.config.horizontal_layout = *layout;
                self.scroll_x = 0.0;
                self.resync_layout_with_anchor(anchor);
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::SetMargins(margins) => {
                let clamped = margins.clamped_non_negative();
                if self.document.config.active_margins_pt() == clamped {
                    return CommandOutcome::NoOp;
                }
                let anchor = self.flow.capture_anchor(self.scroll_y);
                self.document.config.set_active_margins_pt(clamped);
                self.resync_layout_with_anchor(anchor);
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::SetZoom(target_zoom) => self.apply_zoom(*target_zoom, cx),
            RichCommand::AdjustZoom(delta) => self.apply_zoom(self.zoom + *delta, cx),
            RichCommand::ResetZoom => self.apply_zoom(1.0, cx),
            RichCommand::ScrollBy { delta_x, delta_y } => {
                let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
                let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
                let horizontal =
                    resolve_horizontal_metrics(viewport_width, &self.document.config, self.zoom);
                let max_scroll_x = (horizontal.canvas_width - viewport_width).max(0.0);
                let max_scroll_y = (self.flow.total_height() - viewport_height).max(0.0);

                let next_x = (self.scroll_x - *delta_x).clamp(0.0, max_scroll_x);
                let next_y = (self.scroll_y - *delta_y).clamp(0.0, max_scroll_y);
                if (next_x - self.scroll_x).abs() < 0.01 && (next_y - self.scroll_y).abs() < 0.01 {
                    return CommandOutcome::NoOp;
                }
                self.scroll_x = next_x;
                self.scroll_y = next_y;
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::ToggleDebugOverlay => {
                self.debug_overlay = !self.debug_overlay;
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::SetSelection(selection) => {
                self.ime_preedit = None;
                self.history.break_coalescing();
                if self.update_selection(selection.as_deref().cloned(), cx) {
                    self.reset_caret_blink(cx);
                    CommandOutcome::Executed
                } else {
                    CommandOutcome::NoOp
                }
            }
            RichCommand::ClearSelection => {
                self.ime_preedit = None;
                self.history.break_coalescing();
                let dismissed = self.active_chip_popover.take().is_some()
                    || self.selected_image.take().is_some();
                let cleared = self.update_selection(None, cx);
                if dismissed || cleared {
                    cx.notify();
                    CommandOutcome::Executed
                } else {
                    CommandOutcome::NoOp
                }
            }
            RichCommand::SelectAll => {
                self.ime_preedit = None;
                self.history.break_coalescing();
                let leaves = self.document.collect_leaf_blocks();
                let (Some(first), Some(last)) = (leaves.first(), leaves.last()) else {
                    return CommandOutcome::NoOp;
                };
                let end_offset = last
                    .kind
                    .inline_content()
                    .map(|inline| inline.text.len())
                    .unwrap_or(0);
                let (Some(anchor), Some(focus)) = (
                    self.document.doc_pos_for_block(first.id, 0),
                    self.document.doc_pos_for_block(last.id, end_offset),
                ) else {
                    return CommandOutcome::NoOp;
                };
                let next_selection = DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: None,
                };
                if self.update_selection(Some(next_selection), cx) {
                    CommandOutcome::Executed
                } else {
                    CommandOutcome::NoOp
                }
            }
            RichCommand::SelectWordAt(position) => {
                self.ime_preedit = None;
                self.history.break_coalescing();
                let Some((word_start, word_end)) = self.word_bounds_at(position) else {
                    return CommandOutcome::NoOp;
                };
                let mut anchor_position = position.clone();
                anchor_position.offset = word_start;
                let mut focus_position = position.clone();
                focus_position.offset = word_end;
                let next_selection = DocumentSelection::Text {
                    anchor: anchor_position,
                    focus: focus_position,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: None,
                };
                if self.update_selection(Some(next_selection), cx) {
                    CommandOutcome::Executed
                } else {
                    CommandOutcome::NoOp
                }
            }
            RichCommand::MoveCaret {
                direction,
                unit,
                extend_selection,
            } => self.execute_move_caret(*direction, *unit, *extend_selection, cx),
            RichCommand::CopySelection => {
                let Some(item) = self.clipboard_item_for_selection() else {
                    return CommandOutcome::NoOp;
                };
                cx.write_to_clipboard(item);
                CommandOutcome::Executed
            }
            RichCommand::CopyCodeBlock(block_id) => self.execute_copy_code_block(*block_id, cx),
            RichCommand::OpenLink(href) => {
                if href.is_empty() {
                    return CommandOutcome::NoOp;
                }
                cx.emit(RichDocumentEvent::LinkActivated { href: href.clone() });
                if self.open_external_links && is_external_uri(href) {
                    cx.open_url(href);
                }
                CommandOutcome::Executed
            }
            RichCommand::ToggleHeadingCollapsed(heading_id) => {
                let change_set = self.apply_transaction(
                    Transaction::new(vec![Step::ToggleHeadingCollapsed {
                        block_id: *heading_id,
                    }]),
                    false,
                    cx,
                );
                if change_set.dirty_blocks.is_empty() && !change_set.structure_changed {
                    return CommandOutcome::NoOp;
                }
                CommandOutcome::Executed
            }
            RichCommand::Undo => {
                self.ime_preedit = None;
                let anchor = self.flow.capture_anchor(self.scroll_y);
                let Some((change_set, restored_selection)) = self.history.undo(&mut self.document)
                else {
                    return CommandOutcome::NoOp;
                };
                if let Some(selection) = restored_selection {
                    self.update_selection(Some(selection), cx);
                }
                self.resync_layout_with_anchor(anchor);
                self.notify_transaction_applied(change_set, cx);
                self.reset_caret_blink(cx);
                self.ensure_caret_visible();
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::Redo => {
                self.ime_preedit = None;
                let anchor = self.flow.capture_anchor(self.scroll_y);
                let Some((change_set, restored_selection)) = self.history.redo(&mut self.document)
                else {
                    return CommandOutcome::NoOp;
                };
                if let Some(selection) = restored_selection {
                    self.update_selection(Some(selection), cx);
                }
                self.resync_layout_with_anchor(anchor);
                self.notify_transaction_applied(change_set, cx);
                self.reset_caret_blink(cx);
                self.ensure_caret_visible();
                cx.notify();
                CommandOutcome::Executed
            }
            RichCommand::InsertText(text) => {
                if source == CommandSource::User
                    && self.policy.can_apply_input_rules()
                    && self.try_apply_input_rule(text, cx)
                {
                    CommandOutcome::Executed
                } else {
                    self.execute_insert_text_inner(text, cx)
                }
            }
            RichCommand::InsertInlineObject(object) => {
                self.execute_insert_inline_object(object.clone(), cx)
            }
            RichCommand::ToggleChecklistItem(block_id) => {
                self.execute_toggle_checklist_item(*block_id, cx)
            }
            RichCommand::ResizeInlineImage {
                block_id,
                object_id,
                width_pt,
                height_pt,
            } => self.execute_resize_inline_image(*block_id, *object_id, *width_pt, *height_pt, cx),
            RichCommand::Delete { direction, unit } => {
                self.execute_delete(*direction, *unit, source, cx)
            }
            RichCommand::SplitBlock => self.execute_split_block(cx),
            RichCommand::ExitBlock => self.execute_exit_block(cx),
            RichCommand::InsertSoftBreak => self.execute_insert_soft_break(cx),
            RichCommand::Indent | RichCommand::Outdent if self.active_table_context().is_some() => {
                self.execute_navigate_table_cell(matches!(command, RichCommand::Indent), source, cx)
            }
            RichCommand::Indent => self.execute_indent(source, cx),
            RichCommand::Outdent => self.execute_outdent(source, cx),
            RichCommand::ToggleMark(mark) => self.execute_toggle_mark(*mark, cx),
            RichCommand::SetInlineStyle(style) => self.execute_set_inline_style(style, cx),
            RichCommand::ClearFormatting => self.execute_clear_formatting(cx),
            RichCommand::AdjustFontSize(delta_pt) => self.execute_adjust_font_size(*delta_pt, cx),
            RichCommand::SetLink(target) => self.execute_set_link(target.clone(), cx),
            RichCommand::SetBlockStyle(style) => self.execute_set_block_style(style, cx),
            RichCommand::UpdateNamedStyle { style_id, style } => {
                self.execute_update_named_style(*style_id, style.clone(), cx)
            }
            RichCommand::SetHeading(level) => self.execute_set_heading(*level, cx),
            RichCommand::ToggleList(list_kind) => self.execute_toggle_list(*list_kind, cx),
            RichCommand::ToggleBlockquote => self.execute_toggle_blockquote(cx),
            RichCommand::ToggleCodeBlock => self.execute_toggle_code_block(cx),
            RichCommand::SetCodeBlockLanguage(language) => {
                self.execute_set_code_block_language(language.clone(), cx)
            }
            RichCommand::InsertHorizontalRule => self.execute_insert_horizontal_rule(cx),
            RichCommand::InsertTable {
                rows,
                columns,
                with_header,
            } => self.execute_insert_table(*rows, *columns, *with_header, cx),
            RichCommand::SetTextAlign(align) => self.execute_set_text_align(*align, cx),
            RichCommand::PasteHtml { html, plain_text } => {
                self.execute_paste_html(html, plain_text.as_deref(), source, cx)
            }
            RichCommand::PasteFragment {
                fragment,
                html_fallback,
                plain_text,
            } => {
                if fragment.is_empty() {
                    if let Some(html) = html_fallback.as_deref() {
                        self.execute_paste_html(html, plain_text.as_deref(), source, cx)
                    } else if let Some(plain) = plain_text.as_deref() {
                        self.execute_paste_text(plain, source, cx)
                    } else {
                        CommandOutcome::NoOp
                    }
                } else {
                    self.execute_paste_fragment(fragment.clone(), plain_text.as_deref(), source, cx)
                }
            }
            RichCommand::PasteText(text) => self.execute_paste_text(text, source, cx),
            RichCommand::SetTableColumns { table_id, columns } => {
                self.execute_set_table_columns(*table_id, columns.clone(), cx)
            }
            RichCommand::InsertTableRow { after } => self.execute_insert_table_row(*after, cx),
            RichCommand::DeleteTableRow => self.execute_delete_table_row(cx),
            RichCommand::InsertTableColumn { after } => {
                self.execute_insert_table_column(*after, cx)
            }
            RichCommand::DeleteTableColumn => self.execute_delete_table_column(cx),
            RichCommand::MergeTableCells => self.execute_merge_table_cells(cx),
            RichCommand::SplitTableCell => self.execute_split_table_cell(cx),
            RichCommand::NavigateTableCell { forward } => {
                self.execute_navigate_table_cell(*forward, source, cx)
            }
            RichCommand::Navigation(command) => self.execute_navigation_command_inner(command, cx),
            RichCommand::Grid(command) => self.execute_grid_command(command, cx),
            RichCommand::Component(command) => {
                components::execute_component_command(self, command, cx)
            }
            RichCommand::Structure(command) => self.execute_structure_command(command, cx),
            RichCommand::Anchor(command) => self.execute_anchor_command(command, cx),
            RichCommand::Extension(invocation) => self.execute_extension_command(invocation, cx),
        }
    }

    fn execute_move_caret(
        &mut self,
        direction: MoveDirection,
        unit: MoveUnit,
        extend_selection: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();

        let visible_leaves = self.document.collect_visible_leaf_blocks();
        if visible_leaves.is_empty() {
            return CommandOutcome::NoOp;
        }

        let (anchor, focus, affinity, goal_x) = match self.selection.clone() {
            Some(DocumentSelection::Text {
                anchor,
                focus,
                affinity,
                goal_x,
                ..
            }) => (anchor, focus, affinity, goal_x),
            Some(DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                ..
            }) => {
                let Some(pos) = self.first_pos_in_table_cell(table_id, anchor_cell) else {
                    return CommandOutcome::NoOp;
                };
                (pos.clone(), pos, Affinity::Downstream, None)
            }
            None => {
                let fallback_leaf = match direction {
                    MoveDirection::Backward | MoveDirection::Up => visible_leaves.last(),
                    MoveDirection::Forward | MoveDirection::Down => visible_leaves.first(),
                };
                let Some(leaf) = fallback_leaf else {
                    return CommandOutcome::NoOp;
                };
                let offset = match direction {
                    MoveDirection::Backward | MoveDirection::Up => {
                        leaf.kind.inline_content().map(|c| c.len()).unwrap_or(0)
                    }
                    MoveDirection::Forward | MoveDirection::Down => 0,
                };
                let Some(pos) = self.document.doc_pos_for_block(leaf.id, offset) else {
                    return CommandOutcome::NoOp;
                };
                let next_selection = DocumentSelection::caret(pos, Affinity::Downstream);
                self.update_selection(Some(next_selection), cx);
                self.reset_caret_blink(cx);
                self.ensure_caret_visible();
                return CommandOutcome::Executed;
            }
        };

        // Collapsing a non-empty selection with Left/Right jumps to the selection edge.
        if !extend_selection
            && anchor != focus
            && unit == MoveUnit::Grapheme
            && matches!(direction, MoveDirection::Backward | MoveDirection::Forward)
        {
            let (start_pos, end_pos) = self.ordered_positions(&anchor, &focus);
            let collapsed = if direction == MoveDirection::Backward {
                start_pos
            } else {
                end_pos
            };
            let next_selection = DocumentSelection::caret(collapsed, Affinity::Downstream);
            self.update_selection(Some(next_selection), cx);
            self.reset_caret_blink(cx);
            self.ensure_caret_visible();
            return CommandOutcome::Executed;
        }

        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let mut next_goal_x = None;
        let (next_focus, next_affinity) = match unit {
            MoveUnit::DocumentBoundary => {
                let target_leaf = match direction {
                    MoveDirection::Backward | MoveDirection::Up => visible_leaves.first(),
                    MoveDirection::Forward | MoveDirection::Down => visible_leaves.last(),
                };
                let Some(leaf) = target_leaf else {
                    return CommandOutcome::NoOp;
                };
                let offset = match direction {
                    MoveDirection::Backward | MoveDirection::Up => 0,
                    MoveDirection::Forward | MoveDirection::Down => {
                        leaf.kind.inline_content().map(|c| c.len()).unwrap_or(0)
                    }
                };
                let Some(pos) = self.document.doc_pos_for_block(leaf.id, offset) else {
                    return CommandOutcome::NoOp;
                };
                (pos, Affinity::Downstream)
            }
            MoveUnit::LineBoundary => {
                let to_end = matches!(direction, MoveDirection::Forward | MoveDirection::Down);
                let (offset, line_affinity) = self
                    .flow
                    .cached_line_boundary(&focus, affinity, to_end)
                    .unwrap_or_else(|| {
                        let fallback = if to_end {
                            focus
                                .leaf_block_id()
                                .and_then(|id| self.document.find_block(id))
                                .and_then(|block| block.kind.inline_content())
                                .map(|inline| inline.len())
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        (fallback, Affinity::Downstream)
                    });
                let mut pos = focus.clone();
                pos.offset = offset;
                (pos, line_affinity)
            }
            MoveUnit::Line => {
                let delta = match direction {
                    MoveDirection::Up | MoveDirection::Backward => -1,
                    MoveDirection::Down | MoveDirection::Forward => 1,
                };
                if let Some((pos, vertical_affinity, preserved_goal_x)) =
                    self.flow.cached_vertical_move(
                        &focus,
                        affinity,
                        goal_x,
                        delta,
                        &self.document,
                        viewport_width.max(200.0),
                        self.zoom,
                    )
                {
                    next_goal_x = Some(preserved_goal_x);
                    (pos, vertical_affinity)
                } else {
                    return CommandOutcome::NoOp;
                }
            }
            MoveUnit::Grapheme => {
                let Some(pos) = self.step_horizontal(&focus, direction, false) else {
                    return CommandOutcome::NoOp;
                };
                (pos, Affinity::Downstream)
            }
            MoveUnit::Word => {
                let Some(pos) = self.step_horizontal(&focus, direction, true) else {
                    return CommandOutcome::NoOp;
                };
                (pos, Affinity::Downstream)
            }
            MoveUnit::Paragraph => {
                let target = match direction {
                    MoveDirection::Backward | MoveDirection::Up => {
                        crate::model::NavigationTarget::PreviousParagraph
                    }
                    MoveDirection::Forward | MoveDirection::Down => {
                        crate::model::NavigationTarget::NextParagraph
                    }
                };
                let Some(position) =
                    crate::model::navigate_target(&self.document, &target, Some(&focus))
                else {
                    return CommandOutcome::NoOp;
                };
                self.pending_reveal = Some(position.clone());
                (position, Affinity::Downstream)
            }
        };

        let next_selection = if extend_selection {
            DocumentSelection::Text {
                anchor,
                focus: next_focus,
                affinity: next_affinity,
                goal_x: next_goal_x,
                stored_marks: None,
            }
        } else {
            DocumentSelection::Text {
                anchor: next_focus.clone(),
                focus: next_focus,
                affinity: next_affinity,
                goal_x: next_goal_x,
                stored_marks: None,
            }
        };

        if self.update_selection(Some(next_selection), cx) {
            self.reset_caret_blink(cx);
            self.ensure_caret_visible();
            CommandOutcome::Executed
        } else {
            CommandOutcome::NoOp
        }
    }

    pub(super) fn execute_insert_text_inner(
        &mut self,
        raw_text: &str,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let normalized = raw_text.replace("\r\n", "\n").replace('\r', "\n");
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let was_collapsed = anchor == focus;

        let Some((mut steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = caret_pos.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };

        if normalized.is_empty() {
            if steps.is_empty() {
                return CommandOutcome::NoOp;
            }
            let transaction = Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(caret_pos, Affinity::Downstream)),
            );
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }

        let is_code_block = self
            .document
            .find_block(block_id)
            .is_some_and(|block| block.kind.code_block_data().is_some());

        // Multi-line paste outside a code block splits into sibling paragraphs atomically.
        if normalized.contains('\n') && !is_code_block {
            return self.execute_insert_multiline_text(
                &normalized,
                steps,
                caret_pos,
                stored_marks,
                selection_before,
                cx,
            );
        }

        let inserted_len = normalized.len();
        let is_single_typing_burst =
            was_collapsed && normalized.graphemes(true).count() == 1 && normalized != "\n";
        let ends_word = normalized.chars().all(char::is_whitespace);

        steps.push(Step::InsertText {
            block_id,
            offset: caret_pos.offset,
            text: normalized,
            style: stored_marks,
        });

        let mut next_pos = caret_pos;
        next_pos.offset += inserted_len;
        let transaction = Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        );

        let group = if is_single_typing_burst {
            HistoryMergeGroup::Typing { block_id }
        } else {
            HistoryMergeGroup::Atomic
        };
        self.apply_transaction_grouped(transaction, Some(group), cx);
        if ends_word {
            self.history.break_coalescing();
        }
        CommandOutcome::Executed
    }

    fn execute_insert_multiline_text(
        &mut self,
        normalized: &str,
        initial_delete_steps: Vec<Step>,
        caret_pos: DocPos,
        stored_marks: Option<crate::model::InlineStyle>,
        selection_before: Option<DocumentSelection>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(first_block_id) = caret_pos.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let Some((target, start_index)) = self.document.locate_block(first_block_id) else {
            return CommandOutcome::NoOp;
        };
        let Some(first_block) = self.document.find_block(first_block_id).cloned() else {
            return CommandOutcome::NoOp;
        };
        let Some(first_inline) = first_block.kind.inline_content() else {
            return CommandOutcome::NoOp;
        };

        let anchor_scroll = self.flow.capture_anchor(self.scroll_y);
        let mut combined_inverse_steps = Vec::new();
        let mut combined_change_set = ChangeSet::default();

        // Apply any selection deletion first so offsets on `first_block_id` are exact.
        if !initial_delete_steps.is_empty() {
            let applied_del = self.document.apply(Transaction::new(initial_delete_steps));
            combined_inverse_steps.extend(applied_del.inverse.steps);
            combined_change_set.structure_changed |= applied_del.change_set.structure_changed;
            for dirty in applied_del.change_set.dirty_blocks {
                if !combined_change_set.dirty_blocks.contains(&dirty) {
                    combined_change_set.dirty_blocks.push(dirty);
                }
            }
        }

        let current_inline = self
            .document
            .find_block(first_block_id)
            .and_then(|block| block.kind.inline_content())
            .cloned()
            .unwrap_or_else(|| first_inline.clone());
        let split_offset = caret_pos.offset.min(current_inline.len());
        let trailing_after_caret = current_inline.slice(split_offset..current_inline.len());

        let lines: Vec<&str> = normalized.split('\n').collect();
        let mut steps = Vec::new();
        if split_offset < current_inline.len() {
            steps.push(Step::DeleteRange {
                block_id: first_block_id,
                range: split_offset..current_inline.len(),
            });
        }
        if let Some(first_line) = lines.first()
            && !first_line.is_empty()
        {
            steps.push(Step::InsertText {
                block_id: first_block_id,
                offset: split_offset,
                text: (*first_line).to_string(),
                style: stored_marks.clone(),
            });
        }

        let mut last_block_id = first_block_id;
        let mut last_offset = split_offset + lines.first().map(|s| s.len()).unwrap_or(0);

        for (line_offset, line_text) in lines.iter().enumerate().skip(1) {
            let new_id = self.document.alloc_block_id();
            let mut content = InlineContent::empty();
            if !line_text.is_empty() {
                content.push_text(line_text, stored_marks.clone().unwrap_or_default());
            }
            let caret_in_line = content.len();
            if line_offset + 1 == lines.len() && !trailing_after_caret.is_empty() {
                content.insert_content(caret_in_line, &trailing_after_caret);
            }
            let new_block = crate::model::BlockNode::new(
                new_id,
                BlockKind::paragraph(content),
                BlockStyle::default(),
            );
            steps.push(Step::InsertBlockIn {
                target,
                index: start_index + line_offset,
                block: new_block,
            });
            last_block_id = new_id;
            last_offset = caret_in_line;
        }

        let applied_paste = self.document.apply(Transaction::new(steps));
        let mut merged_inverse = applied_paste.inverse.steps;
        merged_inverse.extend(combined_inverse_steps);
        combined_change_set.structure_changed |= applied_paste.change_set.structure_changed;
        for dirty in applied_paste.change_set.dirty_blocks {
            if !combined_change_set.dirty_blocks.contains(&dirty) {
                combined_change_set.dirty_blocks.push(dirty);
            }
        }

        let next_selection = self
            .document
            .doc_pos_for_block(last_block_id, last_offset)
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        self.history.push_inverse_grouped(
            Transaction {
                steps: merged_inverse,
                selection_before: next_selection.clone(),
                selection_after: selection_before,
            },
            HistoryMergeGroup::Atomic,
        );
        if let Some(selection) = next_selection {
            self.update_selection(Some(selection), cx);
        }
        self.resync_layout_with_anchor(anchor_scroll);
        self.notify_transaction_applied(combined_change_set, cx);
        self.reset_caret_blink(cx);
        self.ensure_caret_visible();
        cx.notify();
        CommandOutcome::Executed
    }

    fn execute_delete(
        &mut self,
        direction: MoveDirection,
        unit: MoveUnit,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if source == CommandSource::User
            && matches!(self.selection, Some(DocumentSelection::TableCells { .. }))
            && !self.policy.can_edit_grid()
        {
            return CommandOutcome::RejectedByPolicy;
        }
        self.ime_preedit = None;
        if matches!(self.selection, Some(DocumentSelection::TableCells { .. })) {
            return self.execute_grid_command(&crate::model::GridCommand::ClearSelection, cx);
        }
        if source == CommandSource::User
            && direction == MoveDirection::Backward
            && unit == MoveUnit::Grapheme
            && self.try_undo_input_rule(cx)
        {
            return CommandOutcome::Executed;
        }
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();

        // Delete active non-collapsed range atomically.
        if anchor != focus {
            let Some((steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
            else {
                return CommandOutcome::NoOp;
            };
            if steps.is_empty() {
                return CommandOutcome::NoOp;
            }
            let transaction = Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(caret_pos, Affinity::Downstream)),
            );
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }

        let Some(block_id) = focus.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let Some(inline) = self
            .document
            .find_block(block_id)
            .and_then(|block| block.kind.inline_content())
            .cloned()
        else {
            return CommandOutcome::NoOp;
        };
        let offset = focus.offset.min(inline.len());

        match direction {
            MoveDirection::Backward | MoveDirection::Up => {
                if offset == 0 {
                    let Some(outcome) = self
                        .document
                        .plan_backward_boundary_delete(&focus, selection_before)
                    else {
                        return CommandOutcome::NoOp;
                    };
                    return self.apply_boundary_delete_outcome(outcome, cx);
                }

                let start = match unit {
                    MoveUnit::Grapheme => previous_grapheme_boundary(&inline.text, offset),
                    MoveUnit::Word => previous_word_boundary(&inline.text, offset),
                    MoveUnit::LineBoundary => self
                        .flow
                        .cached_line_boundary(&focus, Affinity::Downstream, false)
                        .map(|(line_start, _)| line_start.min(offset))
                        .unwrap_or(0),
                    _ => previous_grapheme_boundary(&inline.text, offset),
                };
                if start >= offset {
                    return CommandOutcome::NoOp;
                }
                let mut next_pos = focus;
                next_pos.offset = start;
                let transaction = Transaction::new(vec![Step::DeleteRange {
                    block_id,
                    range: start..offset,
                }])
                .with_selections(
                    selection_before,
                    Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
                );
                let group = if unit == MoveUnit::Grapheme {
                    HistoryMergeGroup::Deleting { block_id }
                } else {
                    HistoryMergeGroup::Atomic
                };
                self.apply_transaction_grouped(transaction, Some(group), cx);
                CommandOutcome::Executed
            }
            MoveDirection::Forward | MoveDirection::Down => {
                if offset >= inline.len() {
                    let Some(outcome) = self
                        .document
                        .plan_forward_boundary_delete(&focus, selection_before)
                    else {
                        return CommandOutcome::NoOp;
                    };
                    return self.apply_boundary_delete_outcome(outcome, cx);
                }

                let end = match unit {
                    MoveUnit::Grapheme => next_grapheme_boundary(&inline.text, offset),
                    MoveUnit::Word => next_word_boundary(&inline.text, offset),
                    MoveUnit::LineBoundary => self
                        .flow
                        .cached_line_boundary(&focus, Affinity::Downstream, true)
                        .map(|(line_end, _)| line_end.max(offset).min(inline.len()))
                        .unwrap_or(inline.len()),
                    _ => next_grapheme_boundary(&inline.text, offset),
                };
                if end <= offset {
                    return CommandOutcome::NoOp;
                }
                let transaction = Transaction::new(vec![Step::DeleteRange {
                    block_id,
                    range: offset..end,
                }])
                .with_selections(
                    selection_before,
                    Some(DocumentSelection::caret(focus, Affinity::Downstream)),
                );
                let group = if unit == MoveUnit::Grapheme {
                    HistoryMergeGroup::Deleting { block_id }
                } else {
                    HistoryMergeGroup::Atomic
                };
                self.apply_transaction_grouped(transaction, Some(group), cx);
                CommandOutcome::Executed
            }
        }
    }

    fn apply_boundary_delete_outcome(
        &mut self,
        outcome: BoundaryDeleteOutcome,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        match outcome {
            BoundaryDeleteOutcome::Edit(transaction) => {
                self.apply_transaction_grouped(*transaction, Some(HistoryMergeGroup::Atomic), cx);
                CommandOutcome::Executed
            }
            BoundaryDeleteOutcome::MoveSelection(selection) => {
                if self.update_selection(Some(*selection), cx) {
                    self.reset_caret_blink(cx);
                    self.ensure_caret_visible();
                    CommandOutcome::Executed
                } else {
                    CommandOutcome::NoOp
                }
            }
        }
    }

    fn execute_delete_then(
        &mut self,
        cx: &mut Context<Self>,
        build: impl FnOnce(&mut Document, &DocPos, Option<DocumentSelection>) -> Option<Transaction>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();

        if anchor == focus {
            let Some(transaction) = build(&mut self.document, &focus, selection_before) else {
                return CommandOutcome::NoOp;
            };
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }

        let anchor_scroll = self.flow.capture_anchor(self.scroll_y);
        let Some((delete_steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let applied_del = self.document.apply(Transaction::new(delete_steps));
        let Some(follow_up_tx) = build(&mut self.document, &caret_pos, None) else {
            self.document.apply(applied_del.inverse);
            return CommandOutcome::NoOp;
        };
        let next_selection = follow_up_tx.selection_after.clone();
        let applied_follow_up = self.document.apply(follow_up_tx);

        let mut combined_inverse = applied_follow_up.inverse.steps;
        combined_inverse.extend(applied_del.inverse.steps);
        let mut combined_change_set = applied_del.change_set;
        combined_change_set.structure_changed |= applied_follow_up.change_set.structure_changed;
        for dirty in applied_follow_up.change_set.dirty_blocks {
            if !combined_change_set.dirty_blocks.contains(&dirty) {
                combined_change_set.dirty_blocks.push(dirty);
            }
        }

        self.history.push_inverse_grouped(
            Transaction {
                steps: combined_inverse,
                selection_before: next_selection.clone(),
                selection_after: selection_before,
            },
            HistoryMergeGroup::Atomic,
        );
        if let Some(selection) = next_selection {
            self.update_selection(Some(selection), cx);
        }
        self.resync_layout_with_anchor(anchor_scroll);
        self.notify_transaction_applied(combined_change_set, cx);
        self.reset_caret_blink(cx);
        self.ensure_caret_visible();
        cx.notify();
        CommandOutcome::Executed
    }

    fn execute_split_block(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.execute_delete_then(cx, |document, pos, selection_before| {
            document.build_split_block_transaction(pos, selection_before)
        })
    }

    fn execute_exit_block(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((_, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let Some(outcome) = self.document.plan_exit_block(&focus, selection_before) else {
            return CommandOutcome::NoOp;
        };
        self.apply_boundary_delete_outcome(outcome, cx)
    }

    fn execute_toggle_blockquote(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        let selection_before = self.selection.clone();
        let Some(transaction) = self
            .document
            .build_toggle_blockquote_transaction(&block_ids, selection_before)
        else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_toggle_code_block(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        let selection_before = self.selection.clone();
        let Some(transaction) = self
            .document
            .build_toggle_code_block_transaction(&block_ids, selection_before)
        else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_code_block_language(
        &mut self,
        language: Option<String>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((_, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = focus.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let Some(transaction) = self.document.build_set_code_block_language_transaction(
            block_id,
            language,
            selection_before,
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_insert_horizontal_rule(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.execute_delete_then(cx, |document, pos, selection_before| {
            document.build_insert_horizontal_rule_transaction(pos, selection_before)
        })
    }

    fn execute_insert_table(
        &mut self,
        rows: usize,
        columns: usize,
        with_header: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.execute_delete_then(cx, |document, pos, selection_before| {
            document.build_insert_table_transaction(
                pos,
                rows,
                columns,
                with_header,
                selection_before,
            )
        })
    }

    fn execute_insert_soft_break(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let Some((mut steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = caret_pos.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        steps.push(Step::InsertText {
            block_id,
            offset: caret_pos.offset,
            text: "\n".into(),
            style: stored_marks,
        });
        let mut next_pos = caret_pos;
        next_pos.offset += 1;
        let transaction = Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        );
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_indent(&mut self, source: CommandSource, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((_, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = focus.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let Some(block) = self.document.find_block(block_id).cloned() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();

        if let Some(item) = block.kind.list_item_data()
            && let Some(content) = block.kind.inline_content().cloned()
        {
            if source == CommandSource::User && !self.policy.can_format_lists() {
                return CommandOutcome::RejectedByPolicy;
            }
            if item.indent_level >= MAX_LIST_INDENT_LEVEL {
                return CommandOutcome::NoOp;
            }
            let next_level = item.indent_level + 1;
            let mut next_style = block.style.clone();
            next_style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
            let Some(transaction) = self.document.build_convert_block_transaction(
                block_id,
                BlockKind::list_item(item.list_id, item.kind, next_level, item.checked, content),
                next_style,
                selection_before.clone(),
                selection_before,
            ) else {
                return CommandOutcome::NoOp;
            };
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }
        if block.kind.code_block_data().is_some() {
            return self.execute_insert_text_inner("    ", cx);
        }
        if block.kind.inline_content().is_some() {
            return self.execute_insert_text_inner("\t", cx);
        }
        CommandOutcome::NoOp
    }

    fn execute_outdent(&mut self, source: CommandSource, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((_, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = focus.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let Some(block) = self.document.find_block(block_id).cloned() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();

        if let Some(item) = block.kind.list_item_data()
            && let Some(content) = block.kind.inline_content().cloned()
        {
            if source == CommandSource::User && !self.policy.can_format_lists() {
                return CommandOutcome::RejectedByPolicy;
            }
            let (next_kind, next_style) = if item.indent_level > 0 {
                let next_level = item.indent_level - 1;
                let mut style = block.style.clone();
                style.indent_start_pt = 24.0 * (next_level as f32 + 1.0);
                (
                    BlockKind::list_item(
                        item.list_id,
                        item.kind,
                        next_level,
                        item.checked,
                        content,
                    ),
                    style,
                )
            } else {
                (BlockKind::paragraph(content), BlockStyle::default())
            };
            let Some(transaction) = self.document.build_convert_block_transaction(
                block_id,
                next_kind,
                next_style,
                selection_before.clone(),
                selection_before,
            ) else {
                return CommandOutcome::NoOp;
            };
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }
        if block.kind.code_block_data().is_some()
            && let Some(content) = block.kind.inline_content()
        {
            let offset = focus.offset.min(content.len());
            let line_start = content.text[..offset]
                .rfind('\n')
                .map(|index| index + 1)
                .unwrap_or(0);
            let line_slice = content.text.get(line_start..).unwrap_or("");
            let remove_len = if line_slice.starts_with('\t') {
                1
            } else {
                line_slice
                    .bytes()
                    .take(4)
                    .take_while(|&byte| byte == b' ')
                    .count()
            };
            if remove_len == 0 {
                return CommandOutcome::NoOp;
            }
            let mut next_pos = focus;
            next_pos.offset = offset.saturating_sub(remove_len).max(line_start);
            let transaction = Transaction::new(vec![Step::DeleteRange {
                block_id,
                range: line_start..(line_start + remove_len),
            }])
            .with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            );
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }
        if block.kind.inline_content().is_some() {
            if source == CommandSource::User && !self.policy.can_apply_rich_styles() {
                return CommandOutcome::RejectedByPolicy;
            }
            if block.style.indent_start_pt <= 0.0 {
                return CommandOutcome::NoOp;
            }
            let mut next_style = block.style;
            next_style.indent_start_pt = (next_style.indent_start_pt - 24.0).max(0.0);
            let transaction = Transaction::new(vec![Step::SetBlockStyle {
                block_id,
                style: next_style,
            }])
            .with_selections(selection_before.clone(), selection_before);
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }
        CommandOutcome::NoOp
    }

    fn execute_insert_inline_object(
        &mut self,
        object: InlineObject,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let Some((mut steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let Some(block_id) = caret_pos.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };
        let style = stored_marks.unwrap_or_else(|| {
            self.document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
                .map(|inline| inline.style_at_caret(caret_pos.offset))
                .unwrap_or_default()
        });
        let mut content = InlineContent::empty();
        content.push_object(object, style);
        steps.push(Step::InsertContent {
            block_id,
            offset: caret_pos.offset,
            content,
        });
        let mut next_pos = caret_pos;
        next_pos.offset += OBJECT_REPLACEMENT_UTF8_LEN;
        let transaction = Transaction::new(steps).with_selections(
            selection_before,
            Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
        );
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_toggle_mark(&mut self, mark: MarkKind, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };

        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return CommandOutcome::NoOp;
            };
            let Some(block) = self.document.find_block(block_id) else {
                return CommandOutcome::NoOp;
            };
            let Some(inline) = block.kind.inline_content() else {
                return CommandOutcome::NoOp;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let mut next_marks =
                stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            let enabled = !next_marks.is_mark_active(mark, &base_style);
            next_marks.set_mark(mark, enabled, &base_style);
            self.set_collapsed_stored_marks(next_marks, cx);
            return CommandOutcome::Executed;
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        if covered.is_empty() {
            return CommandOutcome::NoOp;
        }
        let target_enabled = !self.all_ranges_have_mark(&covered, mark);
        let mut steps = Vec::with_capacity(covered.len());
        for (block_id, range) in covered {
            if range.is_empty() {
                continue;
            }
            let Some(block) = self.document.find_block(block_id) else {
                continue;
            };
            let Some(inline) = block.kind.inline_content() else {
                continue;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let mapped = inline.mapped_spans_in_range(range, |style| {
                style.set_mark(mark, target_enabled, &base_style);
            });
            if !mapped.is_empty() {
                steps.push(Step::RestoreSpans {
                    block_id,
                    spans: mapped,
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_inline_style(
        &mut self,
        overlay: &InlineStyle,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };

        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return CommandOutcome::NoOp;
            };
            let Some(inline) = self
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
            else {
                return CommandOutcome::NoOp;
            };
            let current = stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            let next_marks = current.merge(overlay);
            self.set_collapsed_stored_marks(next_marks, cx);
            return CommandOutcome::Executed;
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        let mut steps = Vec::new();
        for (block_id, range) in covered {
            if !range.is_empty() {
                steps.push(Step::ApplyInlineStyle {
                    block_id,
                    range,
                    style: overlay.clone(),
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_link(
        &mut self,
        target: Option<LinkTarget>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };

        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return CommandOutcome::NoOp;
            };
            let Some(inline) = self
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
            else {
                return CommandOutcome::NoOp;
            };
            let mut next_marks =
                stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            next_marks.link = target;
            self.set_collapsed_stored_marks(next_marks, cx);
            return CommandOutcome::Executed;
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        let mut steps = Vec::new();
        for (block_id, range) in covered {
            if range.is_empty() {
                continue;
            }
            let Some(inline) = self
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
            else {
                continue;
            };
            let mapped = inline.mapped_spans_in_range(range, |style| {
                style.link = target.clone();
            });
            if !mapped.is_empty() {
                steps.push(Step::RestoreSpans {
                    block_id,
                    spans: mapped,
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_block_style(
        &mut self,
        style: &BlockStyle,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        if block_ids.is_empty() {
            return CommandOutcome::NoOp;
        }
        let mut steps = Vec::with_capacity(block_ids.len());
        for block_id in block_ids {
            if self
                .document
                .find_block(block_id)
                .is_some_and(|block| &block.style != style)
            {
                steps.push(Step::SetBlockStyle {
                    block_id,
                    style: style.clone(),
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_text_align(
        &mut self,
        align: TextAlign,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        if block_ids.is_empty() {
            return CommandOutcome::NoOp;
        }
        let mut steps = Vec::new();
        for block_id in block_ids {
            if let Some(block) = self.document.find_block(block_id)
                && block.style.align != align
            {
                let mut next_style = block.style.clone();
                next_style.align = align;
                steps.push(Step::SetBlockStyle {
                    block_id,
                    style: next_style,
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_set_heading(
        &mut self,
        target_level: Option<u8>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        if block_ids.is_empty() {
            return CommandOutcome::NoOp;
        }
        let clamped_level = target_level.map(|level| level.clamp(1, 6));
        let mut steps = Vec::new();

        for block_id in block_ids {
            let Some(block) = self.document.find_block(block_id) else {
                continue;
            };
            let align = block.style.align;
            if block.kind.code_block_data().is_some() {
                continue;
            }
            let Some(mut content) = block.kind.inline_content().cloned() else {
                continue;
            };
            let has_inline_size = content
                .spans
                .iter()
                .any(|span| span.style.font_size_pt.is_some());

            let already_matches = match clamped_level {
                Some(target) => {
                    block.kind.heading_data().is_some_and(|h| h.level == target)
                        && block.style.named_style == Some(StyleId::heading(target))
                        && !has_inline_size
                }
                None => {
                    block.kind.is_paragraph()
                        && block.style.named_style == Some(StyleId::NORMAL)
                        && !has_inline_size
                }
            };
            if already_matches {
                continue;
            }

            // Clear explicit inline font-size overrides so the block adopts the target style size.
            if has_inline_size {
                for span in &mut content.spans {
                    span.style.font_size_pt = None;
                }
                content.normalize();
            }

            let (next_kind, next_style) = match clamped_level {
                Some(level) => (
                    BlockKind::heading(level, false, content),
                    BlockStyle::named(StyleId::heading(level)).with_align(align),
                ),
                None => (
                    BlockKind::paragraph(content),
                    BlockStyle::default().with_align(align),
                ),
            };
            steps.push(Step::SetBlockKind {
                block_id,
                kind: next_kind,
            });
            steps.push(Step::SetBlockStyle {
                block_id,
                style: next_style,
            });
        }

        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_before = self.selection.clone();
        let selection_after = selection_before.clone().map(|selection| match selection {
            DocumentSelection::Text {
                anchor,
                focus,
                affinity,
                goal_x,
                mut stored_marks,
            } => {
                if let Some(marks) = stored_marks.as_mut() {
                    marks.font_size_pt = None;
                }
                DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity,
                    goal_x,
                    stored_marks,
                }
            }
            other => other,
        });
        let transaction =
            Transaction::new(steps).with_selections(selection_before, selection_after);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_toggle_list(
        &mut self,
        target_kind: ListKind,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let block_ids = self.selected_leaf_block_ids();
        let convertible: Vec<BlockId> = block_ids
            .into_iter()
            .filter(|block_id| {
                self.document.find_block(*block_id).is_some_and(|block| {
                    block.kind.inline_content().is_some() && block.kind.code_block_data().is_none()
                })
            })
            .collect();
        if convertible.is_empty() {
            return CommandOutcome::NoOp;
        }

        let all_already_target = convertible.iter().all(|block_id| {
            self.document.find_block(*block_id).is_some_and(|block| {
                block
                    .kind
                    .list_item_data()
                    .is_some_and(|item| item.kind == target_kind)
            })
        });

        let shared_list_id = if all_already_target {
            self.document.alloc_list_id()
        } else {
            self.adjacent_list_id_for(convertible[0], target_kind)
                .unwrap_or_else(|| self.document.alloc_list_id())
        };

        let mut steps = Vec::new();
        for block_id in convertible {
            let Some(block) = self.document.find_block(block_id) else {
                continue;
            };
            let align = block.style.align;
            let Some(content) = block.kind.inline_content().cloned() else {
                continue;
            };
            let (existing_indent, existing_checked) = block
                .kind
                .list_item_data()
                .map(|item| (item.indent_level, item.checked))
                .unwrap_or((0, None));

            let (next_kind, next_style) = if all_already_target {
                (
                    BlockKind::paragraph(content),
                    BlockStyle::default().with_align(align),
                )
            } else {
                let indent = 24.0 * (f32::from(existing_indent) + 1.0);
                let mut style = BlockStyle::default()
                    .with_indent(indent, 0.0)
                    .with_align(align);
                style.space_after_pt = Some(4.0);
                let checked = match target_kind {
                    ListKind::Checklist => Some(existing_checked.unwrap_or(false)),
                    _ => None,
                };
                (
                    BlockKind::list_item(
                        shared_list_id,
                        target_kind,
                        existing_indent,
                        checked,
                        content,
                    ),
                    style,
                )
            };

            steps.push(Step::SetBlockKind {
                block_id,
                kind: next_kind,
            });
            steps.push(Step::SetBlockStyle {
                block_id,
                style: next_style,
            });
        }

        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_paste_html(
        &mut self,
        html: &str,
        plain_fallback: Option<&str>,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let fragment = ClipboardFragment::from_html(html);
        self.execute_paste_fragment(fragment, plain_fallback, source, cx)
    }

    fn execute_paste_fragment(
        &mut self,
        mut fragment: ClipboardFragment,
        plain_fallback: Option<&str>,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();

        if fragment.is_empty() {
            return match plain_fallback {
                Some(fallback) if !fallback.is_empty() => {
                    self.execute_paste_text(fallback, source, cx)
                }
                _ => CommandOutcome::NoOp,
            };
        }

        let component_step =
            clipboard::import_component_definitions(&self.document.components, &mut fragment)
                .map(|components| Step::SetComponents { components });
        let rectangle = matches!(self.selection, Some(DocumentSelection::TableCells { .. }));
        let in_table = rectangle
            || self
                .selection
                .as_ref()
                .is_some_and(|selection| match selection {
                    DocumentSelection::Text { focus, .. } => table_cell_of(focus).is_some(),
                    _ => false,
                });
        let single_table = match fragment.blocks.as_slice() {
            [ClipboardBlock::Table(table)] | [ClipboardBlock::CustomGrid { table, .. }] => {
                Some(table)
            }
            _ => None,
        };
        if source == CommandSource::User
            && !self.policy.can_edit_grid()
            && (rectangle || (in_table && single_table.is_some()))
        {
            return CommandOutcome::RejectedByPolicy;
        }
        if in_table && let Some(table) = single_table {
            return self.execute_grid_paste(table, component_step, cx);
        }
        if rectangle {
            let plain = plain_fallback.map_or_else(|| fragment.to_plain_text(), ToOwned::to_owned);
            return self
                .execute_grid_command(&crate::model::GridCommand::PasteTsv { text: plain }, cx);
        }

        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();
        let Some((delete_steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let Some(first_block_id) = caret_pos.leaf_block_id() else {
            return CommandOutcome::NoOp;
        };

        // Inside a code block, paste normalized plain text to preserve raw code semantics.
        if self
            .document
            .find_block(first_block_id)
            .is_some_and(|block| block.kind.code_block_data().is_some())
        {
            let plain = plain_fallback
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| fragment.to_plain_text_with(&self.extensions));
            return self.execute_paste_text(&plain, source, cx);
        }

        if fragment.is_open_inline()
            && let Some(ClipboardBlock::Paragraph { content, .. }) = fragment.blocks.first()
        {
            let inserted_len = content.len();
            let mut steps = delete_steps;
            steps.extend(component_step);
            if inserted_len > 0 {
                steps.push(Step::InsertContent {
                    block_id: first_block_id,
                    offset: caret_pos.offset,
                    content: content.clone(),
                });
            }
            if steps.is_empty() {
                return CommandOutcome::NoOp;
            }
            let mut next_pos = caret_pos;
            next_pos.offset += inserted_len;
            let transaction = Transaction::new(steps).with_selections(
                selection_before,
                Some(DocumentSelection::caret(next_pos, Affinity::Downstream)),
            );
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }

        let Some((target, start_index)) = self.document.locate_block(first_block_id) else {
            return CommandOutcome::NoOp;
        };

        let anchor_scroll = self.flow.capture_anchor(self.scroll_y);
        let mut combined_inverse_steps = Vec::new();
        let mut combined_change_set = ChangeSet::default();

        if !delete_steps.is_empty() {
            let applied_del = self.document.apply(Transaction::new(delete_steps));
            combined_inverse_steps.extend(applied_del.inverse.steps);
            combined_change_set.structure_changed |= applied_del.change_set.structure_changed;
            for dirty in applied_del.change_set.dirty_blocks {
                if !combined_change_set.dirty_blocks.contains(&dirty) {
                    combined_change_set.dirty_blocks.push(dirty);
                }
            }
        }

        let current_inline = self
            .document
            .find_block(first_block_id)
            .and_then(|block| block.kind.inline_content())
            .cloned()
            .unwrap_or_default();
        let split_offset = caret_pos.offset.min(current_inline.len());
        let trailing_after_caret = current_inline.slice(split_offset..current_inline.len());

        let mut pasted_nodes = fragment.to_block_nodes(&mut self.document);
        if pasted_nodes.is_empty() {
            return CommandOutcome::NoOp;
        }

        let mut steps: Vec<_> = component_step.into_iter().collect();
        let mut caret_block_id = first_block_id;
        let mut caret_offset = split_offset;

        let merge_first_into_current = fragment.start_open
            && pasted_nodes
                .first()
                .and_then(|node| node.kind.inline_content())
                .is_some();

        if merge_first_into_current {
            let first_pasted = pasted_nodes.remove(0);
            if split_offset < current_inline.len() {
                steps.push(Step::DeleteRange {
                    block_id: first_block_id,
                    range: split_offset..current_inline.len(),
                });
            }
            let first_inline = first_pasted
                .kind
                .inline_content()
                .cloned()
                .unwrap_or_default();
            let first_len = first_inline.len();
            if !first_inline.is_empty() {
                steps.push(Step::InsertContent {
                    block_id: first_block_id,
                    offset: split_offset,
                    content: first_inline,
                });
            }
            caret_block_id = first_block_id;
            caret_offset = split_offset + first_len;

            if pasted_nodes.is_empty() {
                if fragment.end_open {
                    if !trailing_after_caret.is_empty() {
                        steps.push(Step::RestoreInlineContent {
                            block_id: first_block_id,
                            offset: caret_offset,
                            content: trailing_after_caret,
                        });
                    }
                } else {
                    let tail_id = self.document.alloc_block_id();
                    steps.push(Step::InsertBlockIn {
                        target,
                        index: start_index + 1,
                        block: BlockNode::new(
                            tail_id,
                            BlockKind::paragraph(trailing_after_caret),
                            BlockStyle::default(),
                        ),
                    });
                    caret_block_id = tail_id;
                    caret_offset = 0;
                }
            } else {
                let mut trailing_attached = false;
                if fragment.end_open
                    && let Some(last_node) = pasted_nodes.last_mut()
                    && let Some(last_inline) = last_node.kind.inline_content_mut()
                {
                    caret_block_id = last_node.id;
                    caret_offset = last_inline.len();
                    if !trailing_after_caret.is_empty() {
                        last_inline.insert_content(caret_offset, &trailing_after_caret);
                    }
                    trailing_attached = true;
                }
                if !trailing_attached && (!trailing_after_caret.is_empty() || !fragment.end_open) {
                    let tail_id = self.document.alloc_block_id();
                    pasted_nodes.push(BlockNode::new(
                        tail_id,
                        BlockKind::paragraph(trailing_after_caret),
                        BlockStyle::default(),
                    ));
                    caret_block_id = tail_id;
                    caret_offset = 0;
                } else if !trailing_attached
                    && let Some((fallback_id, fallback_offset)) =
                        last_text_pos_in_nodes(&pasted_nodes)
                {
                    caret_block_id = fallback_id;
                    caret_offset = fallback_offset;
                }

                for (offset_index, node) in pasted_nodes.into_iter().enumerate() {
                    steps.push(Step::InsertBlockIn {
                        target,
                        index: start_index + 1 + offset_index,
                        block: node,
                    });
                }
            }
        } else {
            // Caret at offset 0 (or first pasted block is non-inline such as a Table/HR/Custom):
            // split prefix/suffix cleanly around the pasted block sequence.
            let mut insert_index = start_index;
            if split_offset > 0 && split_offset < current_inline.len() {
                steps.push(Step::DeleteRange {
                    block_id: first_block_id,
                    range: split_offset..current_inline.len(),
                });
                insert_index = start_index + 1;
            } else if split_offset > 0 {
                insert_index = start_index + 1;
            }

            let replace_current_block = split_offset == 0;
            let mut trailing_attached = false;
            if fragment.end_open
                && let Some(last_node) = pasted_nodes.last_mut()
                && let Some(last_inline) = last_node.kind.inline_content_mut()
            {
                caret_block_id = last_node.id;
                caret_offset = last_inline.len();
                if !trailing_after_caret.is_empty() {
                    last_inline.insert_content(caret_offset, &trailing_after_caret);
                }
                trailing_attached = true;
            } else if !replace_current_block && !trailing_after_caret.is_empty() {
                let tail_id = self.document.alloc_block_id();
                pasted_nodes.push(BlockNode::new(
                    tail_id,
                    BlockKind::paragraph(trailing_after_caret.clone()),
                    BlockStyle::default(),
                ));
                caret_block_id = tail_id;
                caret_offset = 0;
                trailing_attached = true;
            } else if replace_current_block && !fragment.end_open {
                caret_block_id = first_block_id;
                caret_offset = 0;
            } else if let Some((fallback_id, fallback_offset)) =
                last_text_pos_in_nodes(&pasted_nodes)
            {
                caret_block_id = fallback_id;
                caret_offset = fallback_offset;
            }

            let pasted_count = pasted_nodes.len();
            for (offset_index, node) in pasted_nodes.into_iter().enumerate() {
                steps.push(Step::InsertBlockIn {
                    target,
                    index: insert_index + offset_index,
                    block: node,
                });
            }
            if replace_current_block && trailing_attached {
                steps.push(Step::RemoveBlockIn {
                    target,
                    index: insert_index + pasted_count,
                });
            }
        }

        let applied_paste = self.document.apply(Transaction::new(steps));
        let mut merged_inverse = applied_paste.inverse.steps;
        merged_inverse.extend(combined_inverse_steps);
        combined_change_set.structure_changed |= applied_paste.change_set.structure_changed;
        for dirty in applied_paste.change_set.dirty_blocks {
            if !combined_change_set.dirty_blocks.contains(&dirty) {
                combined_change_set.dirty_blocks.push(dirty);
            }
        }

        let next_selection = self
            .document
            .doc_pos_for_block(caret_block_id, caret_offset)
            .map(|pos| DocumentSelection::caret(pos, Affinity::Downstream));

        self.history.push_inverse_grouped(
            Transaction {
                steps: merged_inverse,
                selection_before: next_selection.clone(),
                selection_after: selection_before,
            },
            HistoryMergeGroup::Atomic,
        );
        if let Some(selection) = next_selection {
            self.update_selection(Some(selection), cx);
        }
        self.resync_layout_with_anchor(anchor_scroll);
        self.notify_transaction_applied(combined_change_set, cx);
        self.reset_caret_blink(cx);
        self.ensure_caret_visible();
        cx.notify();
        CommandOutcome::Executed
    }

    fn execute_extension_command(
        &mut self,
        invocation: &ExtensionInvocation,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        match self.extensions.execute_extension_command(
            invocation,
            &self.document,
            self.selection.as_ref(),
        ) {
            CommandExecutionResult::Read {
                outcome: ReadCommandOutcome::Executed,
                invalidated_blocks,
            } => {
                for block_id in invalidated_blocks {
                    self.flow.invalidate_block(&self.document, block_id);
                }
                cx.notify();
                CommandOutcome::Executed
            }
            CommandExecutionResult::Read {
                outcome: ReadCommandOutcome::NoOp,
                ..
            } => CommandOutcome::NoOp,
            CommandExecutionResult::Edit {
                transaction: Some(transaction),
                next_block_id,
                history_group,
            } => {
                if transaction.steps.is_empty() {
                    return CommandOutcome::NoOp;
                }
                self.document.reserve_block_ids_up_to(next_block_id);
                let change_set =
                    self.apply_transaction_grouped(*transaction, Some(history_group), cx);
                if change_set.dirty_blocks.is_empty() && !change_set.structure_changed {
                    CommandOutcome::NoOp
                } else {
                    CommandOutcome::Executed
                }
            }
            CommandExecutionResult::Edit {
                transaction: None, ..
            } => CommandOutcome::NoOp,
            CommandExecutionResult::Unregistered => CommandOutcome::UnregisteredCommand,
            CommandExecutionResult::Mismatched => CommandOutcome::MismatchedCommandType,
        }
    }

    fn execute_copy_code_block(
        &mut self,
        block_id: BlockId,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(block) = self.document.find_block(block_id) else {
            return CommandOutcome::NoOp;
        };
        if block.kind.code_block_data().is_none() {
            return CommandOutcome::NoOp;
        }
        let Some(content) = block.kind.inline_content() else {
            return CommandOutcome::NoOp;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(content.text.clone()));
        self.copied_code_block = Some(block_id);
        cx.emit(RichDocumentEvent::CodeBlockCopied { block_id });
        cx.notify();

        self._copy_feedback_task = cx.spawn(async move |weak_view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            if weak_view
                .update(cx, |view, cx| {
                    if view.copied_code_block == Some(block_id) {
                        view.copied_code_block = None;
                        cx.notify();
                    }
                })
                .is_err()
            {
                // View dropped before feedback timer elapsed.
            }
        });
        CommandOutcome::Executed
    }

    fn execute_toggle_checklist_item(
        &mut self,
        block_id: BlockId,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let is_checklist = self
            .document
            .find_block(block_id)
            .and_then(|block| block.kind.list_item_data())
            .is_some_and(|item| item.checked.is_some());
        if !is_checklist {
            return CommandOutcome::NoOp;
        }
        let selection_before = self.selection.clone();
        let transaction = Transaction::new(vec![Step::ToggleChecklist { block_id }])
            .with_selections(selection_before.clone(), selection_before);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_resize_inline_image(
        &mut self,
        block_id: BlockId,
        object_id: InlineObjectId,
        width_pt: f32,
        height_pt: f32,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(existing) = self
            .document
            .find_block(block_id)
            .and_then(|block| block.kind.inline_content())
            .and_then(|inline| inline.object_by_id(object_id))
            .map(|entry| entry.object.clone())
        else {
            return CommandOutcome::NoOp;
        };
        let InlineObject::Image {
            asset_id,
            alt,
            valign,
            width_pt: prev_w,
            height_pt: prev_h,
        } = existing
        else {
            return CommandOutcome::NoOp;
        };
        let next_w = width_pt.clamp(24.0, 2000.0);
        let next_h = height_pt.clamp(24.0, 2000.0);
        if (next_w - prev_w).abs() < 0.25 && (next_h - prev_h).abs() < 0.25 {
            return CommandOutcome::NoOp;
        }
        let selection_before = self.selection.clone();
        let transaction = Transaction::new(vec![Step::UpdateInlineObject {
            block_id,
            object_id,
            object: InlineObject::Image {
                asset_id,
                alt,
                width_pt: next_w,
                height_pt: next_h,
                valign,
            },
        }])
        .with_selections(selection_before.clone(), selection_before);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn set_collapsed_stored_marks(&mut self, marks: InlineStyle, cx: &mut Context<Self>) {
        if let Some(DocumentSelection::Text {
            anchor,
            focus,
            affinity,
            goal_x,
            ..
        }) = self.selection.clone()
        {
            let next_selection = DocumentSelection::Text {
                anchor,
                focus,
                affinity,
                goal_x,
                stored_marks: Some(marks),
            };
            self.update_selection(Some(next_selection), cx);
        }
    }

    fn covered_leaf_ranges(&self, anchor: &DocPos, focus: &DocPos) -> Vec<(BlockId, Range<usize>)> {
        let (Some(anchor_leaf_id), Some(focus_leaf_id)) =
            (anchor.leaf_block_id(), focus.leaf_block_id())
        else {
            return Vec::new();
        };
        let leaves = self.document.collect_leaf_blocks();
        let (Some(anchor_idx), Some(focus_idx)) = (
            leaves.iter().position(|block| block.id == anchor_leaf_id),
            leaves.iter().position(|block| block.id == focus_leaf_id),
        ) else {
            return Vec::new();
        };
        let ((start_idx, start_off), (end_idx, end_off)) =
            if (anchor_idx, anchor.offset) <= (focus_idx, focus.offset) {
                ((anchor_idx, anchor.offset), (focus_idx, focus.offset))
            } else {
                ((focus_idx, focus.offset), (anchor_idx, anchor.offset))
            };

        let mut covered = Vec::new();
        for index in start_idx..=end_idx {
            let Some(block) = leaves.get(index) else {
                continue;
            };
            let Some(inline) = block.kind.inline_content() else {
                continue;
            };
            let range_start = if index == start_idx {
                start_off.min(inline.len())
            } else {
                0
            };
            let range_end = if index == end_idx {
                end_off.min(inline.len()).max(range_start)
            } else {
                inline.len()
            };
            covered.push((block.id, range_start..range_end));
        }
        covered
    }

    fn selected_leaf_block_ids(&self) -> Vec<BlockId> {
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return Vec::new();
        };
        if anchor == focus {
            return focus.leaf_block_id().into_iter().collect();
        }
        self.covered_leaf_ranges(&anchor, &focus)
            .into_iter()
            .map(|(block_id, _)| block_id)
            .collect()
    }

    fn all_ranges_have_mark(&self, ranges: &[(BlockId, Range<usize>)], mark: MarkKind) -> bool {
        let mut saw_any_content = false;
        for (block_id, range) in ranges {
            if range.is_empty() {
                continue;
            }
            let Some(block) = self.document.find_block(*block_id) else {
                continue;
            };
            let Some(inline) = block.kind.inline_content() else {
                continue;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            for span in &inline.spans {
                if span.range.end <= range.start || span.range.start >= range.end {
                    continue;
                }
                saw_any_content = true;
                if !span.style.is_mark_active(mark, &base_style) {
                    return false;
                }
            }
        }
        saw_any_content
    }

    fn adjacent_list_id_for(
        &self,
        block_id: BlockId,
        target_kind: ListKind,
    ) -> Option<crate::model::ListId> {
        let (target, index) = self.document.locate_block(block_id)?;
        let siblings = self.document.container_blocks(target)?;
        if index > 0
            && let Some(prev) = siblings.get(index - 1)
            && let Some(item) = prev.kind.list_item_data()
            && item.kind == target_kind
        {
            return Some(item.list_id);
        }
        if let Some(next) = siblings.get(index + 1)
            && let Some(item) = next.kind.list_item_data()
            && item.kind == target_kind
        {
            return Some(item.list_id);
        }
        None
    }

    /// Returns the resolved inline text style at the active caret or selection start,
    /// incorporating `stored_marks` when set.
    pub fn active_inline_style(&self) -> Option<ResolvedTextStyle> {
        let (anchor, focus, stored_marks) = self.active_text_selection()?;
        let is_collapsed = anchor == focus;
        let pos = if is_collapsed {
            focus
        } else {
            self.ordered_positions(&anchor, &focus).0
        };
        let block_id = pos.leaf_block_id()?;
        let block = self.document.find_block(block_id)?;
        let inline = block.kind.inline_content()?;
        let base_style = block.style.resolved_base_text(
            &self.document.stylesheet,
            &self.document.config.default_style,
        );
        let local_style = if is_collapsed {
            stored_marks.unwrap_or_else(|| inline.style_at_caret(pos.offset))
        } else {
            inline.style_at(pos.offset)
        };
        Some(local_style.resolve(&base_style))
    }

    /// Returns true when `mark` is active across the active selection or at the collapsed caret.
    pub fn is_mark_active(&self, mark: MarkKind) -> bool {
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return false;
        };
        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return false;
            };
            let Some(block) = self.document.find_block(block_id) else {
                return false;
            };
            let Some(inline) = block.kind.inline_content() else {
                return false;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let style = stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            return style.is_mark_active(mark, &base_style);
        }
        let covered = self.covered_leaf_ranges(&anchor, &focus);
        self.all_ranges_have_mark(&covered, mark)
    }

    /// Returns the hyperlink active at the caret or selection start, if any.
    pub fn active_link(&self) -> Option<LinkTarget> {
        self.active_inline_style().and_then(|style| style.link)
    }

    /// Returns the active paragraph alignment for the focused block.
    pub fn active_text_align(&self) -> Option<TextAlign> {
        let (_, focus, _) = self.active_text_selection()?;
        let block_id = focus.leaf_block_id()?;
        Some(self.document.find_block(block_id)?.style.align)
    }

    /// Returns `Some(1..=6)` when the focused block is a heading, or `None` otherwise.
    pub fn active_heading_level(&self) -> Option<u8> {
        let (_, focus, _) = self.active_text_selection()?;
        let block_id = focus.leaf_block_id()?;
        self.document
            .find_block(block_id)?
            .kind
            .heading_data()
            .map(|heading| heading.level)
    }

    /// Returns the [`ListKind`] when the focused block is a list item, or `None` otherwise.
    pub fn active_list_kind(&self) -> Option<ListKind> {
        let (_, focus, _) = self.active_text_selection()?;
        let block_id = focus.leaf_block_id()?;
        self.document
            .find_block(block_id)?
            .kind
            .list_item_data()
            .map(|item| item.kind)
    }

    /// Returns `true` when the focused block is nested inside a blockquote container.
    pub fn is_in_blockquote(&self) -> bool {
        let Some((_, focus, _)) = self.active_text_selection() else {
            return false;
        };
        focus
            .path
            .iter()
            .rev()
            .skip(1)
            .any(|segment| match *segment {
                PathSegment::Block(id) => self
                    .document
                    .find_block(id)
                    .is_some_and(|block| block.kind.semantic_kind() == block_kinds::BLOCKQUOTE),
                PathSegment::TableCell { .. } => false,
            })
    }

    /// Returns the active code block's ID and optional language tag when the focus is inside a
    /// code block.
    pub fn active_code_block(&self) -> Option<(BlockId, Option<String>)> {
        let (_, focus, _) = self.active_text_selection()?;
        let block_id = focus.leaf_block_id()?;
        let code = self.document.find_block(block_id)?.kind.code_block_data()?;
        Some((block_id, code.language.clone()))
    }

    fn execute_clear_formatting(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };

        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return CommandOutcome::NoOp;
            };
            let Some(inline) = self
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
            else {
                return CommandOutcome::NoOp;
            };
            let current = stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            let cleared = current.cleared_formatting();
            self.set_collapsed_stored_marks(cleared, cx);
            return CommandOutcome::Executed;
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        let mut steps = Vec::new();
        for (block_id, range) in covered {
            if range.is_empty() {
                continue;
            }
            let Some(inline) = self
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
            else {
                continue;
            };
            let mapped = inline.mapped_spans_in_range(range, |style| {
                *style = style.cleared_formatting();
            });
            if !mapped.is_empty() {
                steps.push(Step::RestoreSpans {
                    block_id,
                    spans: mapped,
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_adjust_font_size(
        &mut self,
        delta_pt: f32,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if delta_pt.abs() < 0.01 {
            return CommandOutcome::NoOp;
        }
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, stored_marks)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };

        if anchor == focus {
            let Some(block_id) = focus.leaf_block_id() else {
                return CommandOutcome::NoOp;
            };
            let Some(block) = self.document.find_block(block_id) else {
                return CommandOutcome::NoOp;
            };
            let Some(inline) = block.kind.inline_content() else {
                return CommandOutcome::NoOp;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let mut next_marks =
                stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            let current_pt = next_marks.font_size_pt.unwrap_or(base_style.font_size_pt);
            let next_pt = (current_pt + delta_pt).clamp(6.0, 96.0);
            if (next_pt - current_pt).abs() < 0.01 {
                return CommandOutcome::NoOp;
            }
            next_marks.font_size_pt = Some(next_pt);
            self.set_collapsed_stored_marks(next_marks, cx);
            return CommandOutcome::Executed;
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        let mut steps = Vec::new();
        for (block_id, range) in covered {
            if range.is_empty() {
                continue;
            }
            let Some(block) = self.document.find_block(block_id) else {
                continue;
            };
            let Some(inline) = block.kind.inline_content() else {
                continue;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let mapped = inline.mapped_spans_in_range(range, |style| {
                let current_pt = style.font_size_pt.unwrap_or(base_style.font_size_pt);
                style.font_size_pt = Some((current_pt + delta_pt).clamp(6.0, 96.0));
            });
            if !mapped.is_empty() {
                steps.push(Step::RestoreSpans {
                    block_id,
                    spans: mapped,
                });
            }
        }
        if steps.is_empty() {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction =
            Transaction::new(steps).with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_update_named_style(
        &mut self,
        style_id: StyleId,
        style: NamedStyle,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        if self.document.stylesheet.get(style_id) == Some(&style) {
            return CommandOutcome::NoOp;
        }
        let selection_snapshot = self.selection.clone();
        let transaction = Transaction::new(vec![Step::UpdateNamedStyle {
            style_id,
            style: Some(style),
        }])
        .with_selections(selection_snapshot.clone(), selection_snapshot);
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn collect_selection_resolved_styles(&self) -> Option<Vec<ResolvedTextStyle>> {
        let (anchor, focus, stored_marks) = self.active_text_selection()?;
        if anchor == focus {
            let block_id = focus.leaf_block_id()?;
            let block = self.document.find_block(block_id)?;
            let inline = block.kind.inline_content()?;
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            let local = stored_marks.unwrap_or_else(|| inline.style_at_caret(focus.offset));
            return Some(vec![local.resolve(&base_style)]);
        }

        let covered = self.covered_leaf_ranges(&anchor, &focus);
        let mut styles = Vec::new();
        for (block_id, range) in covered {
            if range.is_empty() {
                continue;
            }
            let Some(block) = self.document.find_block(block_id) else {
                continue;
            };
            let Some(inline) = block.kind.inline_content() else {
                continue;
            };
            let base_style = block.style.resolved_base_text(
                &self.document.stylesheet,
                &self.document.config.default_style,
            );
            for span in &inline.spans {
                if span.range.end <= range.start || span.range.start >= range.end {
                    continue;
                }
                styles.push(span.style.resolve(&base_style));
            }
        }
        if styles.is_empty() {
            self.active_inline_style().map(|style| vec![style])
        } else {
            Some(styles)
        }
    }

    /// Returns whether the active selection has one uniform font family or spans multiple families.
    pub fn selected_font_family(&self) -> Option<SelectionValue<SharedStr>> {
        let styles = self.collect_selection_resolved_styles()?;
        let first = styles.first()?.font_family.clone();
        if styles.iter().all(|style| style.font_family == first) {
            Some(SelectionValue::Uniform(first))
        } else {
            Some(SelectionValue::Mixed)
        }
    }

    /// Returns whether the active selection has one uniform font size (in pt) or spans multiple sizes.
    pub fn selected_font_size_pt(&self) -> Option<SelectionValue<f32>> {
        let styles = self.collect_selection_resolved_styles()?;
        let first = styles.first()?.font_size_pt;
        if styles
            .iter()
            .all(|style| (style.font_size_pt - first).abs() < 0.05)
        {
            Some(SelectionValue::Uniform(first))
        } else {
            Some(SelectionValue::Mixed)
        }
    }

    /// Returns whether the active selection has one uniform text color or spans multiple colors.
    pub fn selected_text_color(&self) -> Option<SelectionValue<RgbaColor>> {
        let styles = self.collect_selection_resolved_styles()?;
        let first = styles.first()?.color;
        if styles.iter().all(|style| style.color == first) {
            Some(SelectionValue::Uniform(first))
        } else {
            Some(SelectionValue::Mixed)
        }
    }

    /// Returns whether `mark` is uniformly on, uniformly off, or mixed across the active selection.
    pub fn selected_mark_state(&self, mark: MarkKind) -> Option<SelectionValue<bool>> {
        let styles = self.collect_selection_resolved_styles()?;
        let first = styles.first()?.is_mark_active(mark);
        if styles
            .iter()
            .all(|style| style.is_mark_active(mark) == first)
        {
            Some(SelectionValue::Uniform(first))
        } else {
            Some(SelectionValue::Mixed)
        }
    }

    /// Returns whether all selected blocks share one [`TextAlign`] or mix multiple alignments.
    pub fn selected_text_align(&self) -> Option<SelectionValue<TextAlign>> {
        let block_ids = self.selected_leaf_block_ids();
        let first_id = *block_ids.first()?;
        let first_align = self.document.find_block(first_id)?.style.align;
        for block_id in block_ids.into_iter().skip(1) {
            if let Some(block) = self.document.find_block(block_id)
                && block.style.align != first_align
            {
                return Some(SelectionValue::Mixed);
            }
        }
        Some(SelectionValue::Uniform(first_align))
    }

    /// Returns whether all selected blocks share one heading level (`Some(1..=6)` / `None`) or mix levels.
    pub fn selected_heading_level(&self) -> Option<SelectionValue<Option<u8>>> {
        let block_ids = self.selected_leaf_block_ids();
        let first_id = *block_ids.first()?;
        let heading_of = |block_id: BlockId| {
            let block = self.document.find_block(block_id)?;
            Some(block.kind.heading_data().map(|heading| heading.level))
        };
        let first_level = heading_of(first_id)?;
        for block_id in block_ids.into_iter().skip(1) {
            if let Some(level) = heading_of(block_id)
                && level != first_level
            {
                return Some(SelectionValue::Mixed);
            }
        }
        Some(SelectionValue::Uniform(first_level))
    }

    /// Returns whether all selected blocks are inside a blockquote (`Uniform(true)` / `Uniform(false)`)
    /// or span both quoted and unquoted blocks (`Mixed`).
    pub fn selected_in_blockquote(&self) -> Option<SelectionValue<bool>> {
        let block_ids = self.selected_leaf_block_ids();
        let first_id = *block_ids.first()?;
        let in_quote = |block_id: BlockId| {
            self.document.path_for_block(block_id).is_some_and(|path| {
                path.iter().rev().skip(1).any(|segment| match *segment {
                    PathSegment::Block(id) => self
                        .document
                        .find_block(id)
                        .is_some_and(|block| block.kind.semantic_kind() == block_kinds::BLOCKQUOTE),
                    PathSegment::TableCell { .. } => false,
                })
            })
        };
        let first = in_quote(first_id);
        for block_id in block_ids.into_iter().skip(1) {
            if in_quote(block_id) != first {
                return Some(SelectionValue::Mixed);
            }
        }
        Some(SelectionValue::Uniform(first))
    }

    pub(super) fn active_text_selection(
        &self,
    ) -> Option<(DocPos, DocPos, Option<crate::model::InlineStyle>)> {
        match self.selection.as_ref()? {
            DocumentSelection::Text {
                anchor,
                focus,
                stored_marks,
                ..
            } => Some((anchor.clone(), focus.clone(), stored_marks.clone())),
            DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                ..
            } => {
                let pos = self.first_pos_in_table_cell(*table_id, *anchor_cell)?;
                Some((pos.clone(), pos, None))
            }
        }
    }

    pub(super) fn first_pos_in_table_cell(
        &self,
        table_id: BlockId,
        cell_coords: (usize, usize),
    ) -> Option<DocPos> {
        let table = self.document.find_block(table_id)?.kind.table_block()?;
        let (_, placements, occupancy) = table.resolve_grid();
        let placement_idx = occupancy
            .get(cell_coords.0)
            .and_then(|row| row.get(cell_coords.1))
            .and_then(|slot| *slot)?;
        let placement = placements.get(placement_idx)?;
        let first_block = table
            .rows
            .get(placement.row_index)?
            .cells
            .get(placement.cell_index)?
            .blocks
            .first()?;
        self.document.doc_pos_for_block(first_block.id, 0)
    }

    fn ordered_positions(&self, first: &DocPos, second: &DocPos) -> (DocPos, DocPos) {
        let leaves = self.document.collect_leaf_blocks();
        let first_index = first
            .leaf_block_id()
            .and_then(|id| leaves.iter().position(|block| block.id == id))
            .unwrap_or(0);
        let second_index = second
            .leaf_block_id()
            .and_then(|id| leaves.iter().position(|block| block.id == id))
            .unwrap_or(0);
        if (first_index, first.offset) <= (second_index, second.offset) {
            (first.clone(), second.clone())
        } else {
            (second.clone(), first.clone())
        }
    }

    fn step_horizontal(
        &self,
        pos: &DocPos,
        direction: MoveDirection,
        by_word: bool,
    ) -> Option<DocPos> {
        let leaf_id = pos.leaf_block_id()?;
        let inline = self.document.find_block(leaf_id)?.kind.inline_content()?;
        let offset = pos.offset.min(inline.len());

        match direction {
            MoveDirection::Backward | MoveDirection::Up => {
                if offset > 0 {
                    let next_offset = if by_word {
                        previous_word_boundary(&inline.text, offset)
                    } else {
                        previous_grapheme_boundary(&inline.text, offset)
                    };
                    let mut next = pos.clone();
                    next.offset = next_offset;
                    Some(next)
                } else {
                    let visible_leaves = self.document.collect_visible_leaf_blocks();
                    let current_index = visible_leaves
                        .iter()
                        .position(|block| block.id == leaf_id)?;
                    let prev_leaf = visible_leaves.get(current_index.checked_sub(1)?)?;
                    let prev_len = prev_leaf
                        .kind
                        .inline_content()
                        .map(|content| content.len())
                        .unwrap_or(0);
                    self.document.doc_pos_for_block(prev_leaf.id, prev_len)
                }
            }
            MoveDirection::Forward | MoveDirection::Down => {
                if offset < inline.len() {
                    let next_offset = if by_word {
                        next_word_boundary(&inline.text, offset)
                    } else {
                        next_grapheme_boundary(&inline.text, offset)
                    };
                    let mut next = pos.clone();
                    next.offset = next_offset;
                    Some(next)
                } else {
                    let visible_leaves = self.document.collect_visible_leaf_blocks();
                    let current_index = visible_leaves
                        .iter()
                        .position(|block| block.id == leaf_id)?;
                    let next_leaf = visible_leaves.get(current_index + 1)?;
                    self.document.doc_pos_for_block(next_leaf.id, 0)
                }
            }
        }
    }

    pub(super) fn ensure_caret_visible(&mut self) {
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
        if viewport_width <= 1.0 || viewport_height <= 1.0 {
            return;
        }
        let Some(DocumentSelection::Text {
            focus, affinity, ..
        }) = self.selection.as_ref()
        else {
            return;
        };

        let (caret_top, caret_bottom) = self
            .flow
            .cached_caret_rect(focus, *affinity, &self.document, viewport_width, self.zoom)
            .map(|rect| (rect.y, rect.bottom()))
            .or_else(|| {
                self.flow
                    .block_canvas_y(focus, &self.document)
                    .map(|y| (y, y + 24.0))
            })
            .unwrap_or((0.0, 0.0));

        let max_scroll_y = (self.flow.total_height() - viewport_height).max(0.0);
        if caret_top < self.scroll_y + CARET_SCROLL_MARGIN_PX {
            self.scroll_y = (caret_top - CARET_SCROLL_MARGIN_PX).clamp(0.0, max_scroll_y);
        } else if caret_bottom > self.scroll_y + viewport_height - CARET_SCROLL_MARGIN_PX {
            self.scroll_y =
                (caret_bottom - viewport_height + CARET_SCROLL_MARGIN_PX).clamp(0.0, max_scroll_y);
        }
    }

    pub(super) fn update_selection(
        &mut self,
        next_selection: Option<DocumentSelection>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.selection == next_selection {
            return false;
        }
        self.pending_reveal = None;
        self.selection = next_selection;
        self.dismiss_overlays_where(|spec| spec.dismiss.on_selection_change, cx);
        cx.emit(RichDocumentEvent::SelectionChanged {
            selection: self.selection.clone().map(Box::new),
        });
        cx.notify();
        true
    }

    fn notify_transaction_applied(&mut self, change_set: ChangeSet, cx: &mut Context<Self>) {
        self.document_revision = self.document_revision.wrapping_add(1);
        self.refresh_search_matches();
        self.dismiss_overlays_where(|spec| spec.dismiss.on_document_edit, cx);
        for (_, middleware) in &mut self.middleware {
            middleware.after_transaction(&change_set, &self.document, self.selection.as_ref());
        }
        cx.emit(RichDocumentEvent::DocumentMutated { change_set });
    }

    /// Convenience wrapper dispatching [`RichCommand::SetZoom`].
    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) -> CommandOutcome {
        self.dispatch_command(RichCommand::SetZoom(zoom), cx)
    }

    /// Convenience wrapper dispatching [`RichCommand::SetHorizontalLayout`].
    pub fn set_horizontal_layout(
        &mut self,
        layout: HorizontalLayout,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.dispatch_command(RichCommand::SetHorizontalLayout(layout), cx)
    }

    /// Convenience wrapper dispatching [`RichCommand::ToggleDebugOverlay`].
    pub fn toggle_debug_overlay(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.dispatch_command(RichCommand::ToggleDebugOverlay, cx)
    }

    /// Extracts the rich [`ClipboardFragment`] covered by the current selection.
    pub fn selected_fragment(&self) -> Option<ClipboardFragment> {
        let selection = self.selection.as_ref()?;
        self.document.extract_selection_fragment(selection)
    }

    /// Extracts the plain UTF-8 text covered by the current selection (TSV for multi-cell table selections).
    pub fn selected_plain_text(&self) -> Option<String> {
        let fragment = self.selected_fragment()?;
        Some(fragment.to_plain_text_with(&clipboard::ComponentClipboardFormatter::new(self)))
    }

    /// Extracts normalized semantic HTML (`text/html`) covering the current selection.
    pub fn selected_html(&self) -> Option<String> {
        let fragment = self.selected_fragment()?;
        Some(fragment.to_html_with(&clipboard::ComponentClipboardFormatter::new(self)))
    }
}
