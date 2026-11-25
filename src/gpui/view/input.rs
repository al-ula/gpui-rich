//! Mouse/scroll pointer interactions, GPUI IME (`EntityInputHandler`), and boundary helpers.

use super::*;

impl RichDocumentView {
    pub(super) fn handle_scroll_wheel(
        &mut self,
        event: &ScrollEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (raw_x, raw_y): (f32, f32) = match event.delta {
            ScrollDelta::Pixels(pixels) => (pixels.x.into(), pixels.y.into()),
            ScrollDelta::Lines(lines) => (lines.x * 36.0, lines.y * 36.0),
        };
        let (delta_x, delta_y) = if event.modifiers.shift && raw_x.abs() < 0.01 {
            (raw_y, 0.0)
        } else {
            (raw_x, raw_y)
        };
        self.dismiss_overlays_where(|spec| spec.dismiss.on_scroll, cx);
        self.dispatch_command(RichCommand::ScrollBy { delta_x, delta_y }, cx);
    }

    pub(super) fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.dismiss_overlays_where(|spec| spec.dismiss.on_outside_click, cx);
        self.ime_preedit = None;
        self.history.break_coalescing();
        let canvas_point = self.window_point_to_canvas(event.position);
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();

        // 1. Check corner drag-resize handles on the currently selected inline image.
        if self.policy.can_resize_images()
            && let Some(selected_img) = self.selected_image
            && let Some(image_rect) = self.flow.inline_object_rect_cached(
                selected_img.block_id,
                selected_img.object_id,
                &self.document,
                viewport_width,
                self.zoom,
            )
            && let Some(corner) = hit_test_image_resize_handle(image_rect, canvas_point)
            && let Some(InlineObject::Image {
                width_pt,
                height_pt,
                ..
            }) = self
                .document
                .find_block(selected_img.block_id)
                .and_then(|block| block.kind.inline_content())
                .and_then(|inline| inline.object_by_id(selected_img.object_id))
                .map(|entry| &entry.object)
        {
            self.active_chip_popover = None;
            self.image_resize_drag = Some(ImageResizeDrag {
                block_id: selected_img.block_id,
                object_id: selected_img.object_id,
                corner,
                start_canvas_point: canvas_point,
                initial_width_pt: *width_pt,
                initial_height_pt: *height_pt,
                current_width_pt: *width_pt,
                current_height_pt: *height_pt,
            });
            cx.notify();
            return;
        }

        // 2. Check interactive table column divider dragging.
        if self.policy.can_resize_table_columns()
            && let Some(divider) = self.flow.hit_test_table_column_border(
                canvas_point,
                &self.document,
                viewport_width,
                self.zoom,
                5.0,
            )
        {
            self.active_chip_popover = None;
            self.selected_image = None;
            self.table_column_drag = Some(TableColumnDrag {
                table_id: divider.table_id,
                column_index: divider.column_index,
                start_canvas_x: canvas_point.x,
                initial_divider_x: divider.divider_x,
                current_divider_x: divider.divider_x,
                table_y: divider.table_y,
                table_height: divider.table_height,
                initial_widths_px: divider.column_widths_px,
            });
            cx.notify();
            return;
        }

        let mut shaper = GpuiShaper::with_extensions(window, &self.extensions)
            .with_components(&self.document.components)
            .with_optional_font_resolver(self.font_resolver.as_deref());
        let Some(hit) = self.flow.hit_test_canvas(
            canvas_point,
            &self.document,
            viewport_width,
            self.zoom,
            &mut shaper,
        ) else {
            self.selected_image = None;
            self.active_chip_popover = None;
            cx.notify();
            return;
        };

        if self.policy.can_collapse_headings()
            && let Some(heading_id) = hit.toggled_heading
        {
            self.active_chip_popover = None;
            self.selected_image = None;
            self.dispatch_command(RichCommand::ToggleHeadingCollapsed(heading_id), cx);
            return;
        }

        if self.policy.can_toggle_checklists()
            && let Some(checklist_id) = hit.toggled_checklist
        {
            self.active_chip_popover = None;
            self.selected_image = None;
            self.dispatch_command(RichCommand::ToggleChecklistItem(checklist_id), cx);
            return;
        }

        if self.policy.can_copy_code()
            && let Some(code_block_id) = hit.code_block_copy
        {
            self.active_chip_popover = None;
            self.selected_image = None;
            self.dispatch_command(RichCommand::CopyCodeBlock(code_block_id), cx);
            return;
        }

        // Inspect clicked inline atomic objects (images for resize handles, smart chips for popovers).
        let clicked_object = hit.hit.inline_object.and_then(|object_id| {
            let block_id = hit.pos.leaf_block_id()?;
            let entry = self
                .document
                .find_block(block_id)?
                .kind
                .inline_content()?
                .object_by_id(object_id)?;
            Some((block_id, object_id, entry.object.clone()))
        });

        self.pending_footnote = clicked_object
            .as_ref()
            .and_then(|(_, _, object)| crate::model::footnote_reference_id(object));
        self.pending_chip = None;
        match clicked_object {
            Some((block_id, object_id, InlineObject::Image { .. }))
                if self.policy.can_resize_images() =>
            {
                self.selected_image = Some(SelectedInlineImage {
                    block_id,
                    object_id,
                });
                self.active_chip_popover = None;
            }
            Some((
                block_id,
                object_id,
                InlineObject::SmartChip {
                    kind,
                    label,
                    accent,
                },
            )) if self.policy.can_activate_smart_chips() => {
                self.selected_image = None;
                self.pending_chip = Some(ActiveSmartChipPopover {
                    block_id,
                    object_id,
                    kind,
                    label,
                    accent,
                });
            }
            Some((block_id, object_id, InlineObject::Custom { kind, data }))
                if self.policy.can_activate_smart_chips()
                    && kind.as_ref() == crate::model::DROPDOWN_KIND =>
            {
                self.selected_image = None;
                self.pending_chip = Some(ActiveSmartChipPopover {
                    block_id,
                    object_id,
                    kind: ChipKind::Tag,
                    label: data
                        .to_json()
                        .ok()
                        .map(|json| self.document.components.inline_fallback(&kind, &json))
                        .unwrap_or_default(),
                    accent: None,
                });
            }
            _ => {
                self.selected_image = None;
                self.active_chip_popover = None;
            }
        }

        if self.policy.can_select() && event.click_count >= 2 {
            self.pending_link = None;
            self.pending_chip = None;
            self.drag_origin = None;
            self.drag_moved = false;
            self.dragging = false;
            if self.dispatch_command(RichCommand::SelectWordAt(hit.pos.clone()), cx)
                == CommandOutcome::Executed
            {
                return;
            }
        }

        self.pending_link = if self.policy.can_open_links() {
            hit.hit.link.map(|link| link.href)
        } else {
            None
        };
        self.drag_origin = Some(canvas_point);
        self.drag_moved = false;

        if !self.policy.can_select() {
            self.drag_anchor_pos = None;
            self.dragging = false;
            cx.notify();
            return;
        }

        let next_selection = if event.modifiers.shift
            && let Some(DocumentSelection::Text { anchor, .. }) = self.selection.clone()
        {
            self.drag_anchor_pos = Some(anchor.clone());
            selection_from_drag_positions(&self.document, anchor, hit.pos, hit.affinity)
        } else {
            self.drag_anchor_pos = Some(hit.pos.clone());
            DocumentSelection::caret(hit.pos, hit.affinity)
        };
        self.dragging = true;
        self.dispatch_command(
            RichCommand::SetSelection(Some(Box::new(next_selection))),
            cx,
        );
        cx.notify();
    }

    pub(super) fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let canvas_point = self.window_point_to_canvas(event.position);
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        if viewport_width <= 1.0 {
            return;
        }

        // Active image corner resize drag.
        if event.dragging()
            && let Some(drag) = self.image_resize_drag.as_mut()
        {
            let px_per_pt = pt_to_px(1.0, self.zoom).max(0.1);
            let dx_pt = (canvas_point.x - drag.start_canvas_point.x) / px_per_pt;
            let dy_pt = (canvas_point.y - drag.start_canvas_point.y) / px_per_pt;
            let raw_w = match drag.corner {
                ImageResizeCorner::TopLeft | ImageResizeCorner::BottomLeft => {
                    drag.initial_width_pt - dx_pt
                }
                ImageResizeCorner::TopRight | ImageResizeCorner::BottomRight => {
                    drag.initial_width_pt + dx_pt
                }
            };
            let raw_h = match drag.corner {
                ImageResizeCorner::TopLeft | ImageResizeCorner::TopRight => {
                    drag.initial_height_pt - dy_pt
                }
                ImageResizeCorner::BottomLeft | ImageResizeCorner::BottomRight => {
                    drag.initial_height_pt + dy_pt
                }
            };
            let horizontal =
                resolve_horizontal_metrics(viewport_width, &self.document.config, self.zoom);
            let max_width_pt = (horizontal.column_width / px_per_pt).max(60.0);
            let clamped_w = raw_w.clamp(24.0, max_width_pt);
            let (next_w, next_h) = if event.modifiers.shift {
                (clamped_w, raw_h.clamp(24.0, 1200.0))
            } else {
                let aspect = drag.initial_width_pt / drag.initial_height_pt.max(1.0);
                let locked_h = (clamped_w / aspect.max(0.05)).clamp(24.0, 1200.0);
                (locked_h * aspect, locked_h)
            };
            drag.current_width_pt = next_w.clamp(24.0, max_width_pt);
            drag.current_height_pt = next_h.clamp(24.0, 1200.0);
            cx.notify();
            return;
        }

        // Active table column border drag.
        if event.dragging()
            && let Some(drag) = self.table_column_drag.as_mut()
        {
            let col = drag.column_index;
            if let (Some(&left_w), Some(&right_w)) = (
                drag.initial_widths_px.get(col),
                drag.initial_widths_px.get(col + 1),
            ) {
                let min_col_px = pt_to_px(28.0, self.zoom);
                let min_delta = -(left_w - min_col_px).max(0.0);
                let max_delta = (right_w - min_col_px).max(0.0);
                let delta = (canvas_point.x - drag.start_canvas_x).clamp(min_delta, max_delta);
                drag.current_divider_x = drag.initial_divider_x + delta;
                cx.notify();
            }
            return;
        }

        let mut shaper = GpuiShaper::with_extensions(window, &self.extensions)
            .with_components(&self.document.components)
            .with_optional_font_resolver(self.font_resolver.as_deref());
        let hit = self.flow.hit_test_canvas(
            canvas_point,
            &self.document,
            viewport_width,
            self.zoom,
            &mut shaper,
        );

        if let Some(origin) = self.drag_origin
            && event.dragging()
        {
            let delta_x = canvas_point.x - origin.x;
            let delta_y = canvas_point.y - origin.y;
            if delta_x * delta_x + delta_y * delta_y > 9.0 {
                self.drag_moved = true;
            }
        }

        if !self.dragging || !event.dragging() || !self.policy.can_select() {
            let next_image_handle = if self.policy.can_resize_images()
                && let Some(selected_img) = self.selected_image
                && let Some(image_rect) = self.flow.inline_object_rect_cached(
                    selected_img.block_id,
                    selected_img.object_id,
                    &self.document,
                    viewport_width,
                    self.zoom,
                ) {
                hit_test_image_resize_handle(image_rect, canvas_point)
            } else {
                None
            };

            let next_divider =
                if self.policy.can_resize_table_columns() && next_image_handle.is_none() {
                    self.flow.hit_test_table_column_border(
                        canvas_point,
                        &self.document,
                        viewport_width,
                        self.zoom,
                        5.0,
                    )
                } else {
                    None
                };

            let hovered_chip = hit.as_ref().is_some_and(|result| {
                if !self.policy.can_activate_smart_chips() {
                    return false;
                }
                let Some(object_id) = result.hit.inline_object else {
                    return false;
                };
                let Some(block_id) = result.pos.leaf_block_id() else {
                    return false;
                };
                self.document
                    .find_block(block_id)
                    .and_then(|block| block.kind.inline_content())
                    .and_then(|inline| inline.object_by_id(object_id))
                    .is_some_and(|entry| match &entry.object {
                        InlineObject::SmartChip { .. } => true,
                        InlineObject::Custom { kind, .. } => {
                            kind.as_ref() == crate::model::DROPDOWN_KIND
                        }
                        _ => false,
                    })
            });

            let pointer = hit.as_ref().is_some_and(|result| {
                (self.policy.can_collapse_headings() && result.toggled_heading.is_some())
                    || (self.policy.can_toggle_checklists() && result.toggled_checklist.is_some())
                    || (self.policy.can_copy_code() && result.code_block_copy.is_some())
                    || (self.policy.can_open_links() && result.hit.link.is_some())
            }) || hovered_chip;

            let hover_changed = next_image_handle != self.hovered_image_handle
                || next_divider != self.hovered_column_divider
                || pointer != self.hovered_pointer;
            if hover_changed {
                self.hovered_image_handle = next_image_handle;
                self.hovered_column_divider = next_divider;
                self.hovered_pointer = pointer;
                cx.notify();
            }
            return;
        }

        let Some(hit) = hit else {
            return;
        };

        if let Some(anchor) = self.drag_anchor_pos.clone() {
            let next_selection =
                selection_from_drag_positions(&self.document, anchor, hit.pos, hit.affinity);
            self.dispatch_command(
                RichCommand::SetSelection(Some(Box::new(next_selection))),
                cx,
            );
        }
    }

    pub(super) fn handle_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.image_resize_drag.take() {
            self.dispatch_command(
                RichCommand::ResizeInlineImage {
                    block_id: drag.block_id,
                    object_id: drag.object_id,
                    width_pt: drag.current_width_pt,
                    height_pt: drag.current_height_pt,
                },
                cx,
            );
            cx.notify();
            return;
        }

        if let Some(drag) = self.table_column_drag.take() {
            let delta = drag.current_divider_x - drag.initial_divider_x;
            if delta.abs() >= 1.0 && drag.column_index + 1 < drag.initial_widths_px.len() {
                let mut next_widths = drag.initial_widths_px;
                next_widths[drag.column_index] += delta;
                next_widths[drag.column_index + 1] -= delta;
                let total: f32 = next_widths.iter().sum::<f32>().max(1.0);
                let col_count = next_widths.len() as f32;
                let columns: Vec<ColumnSpec> = next_widths
                    .into_iter()
                    .map(|width| ColumnSpec::Ratio(((width / total) * col_count).max(0.05)))
                    .collect();
                self.dispatch_command(
                    RichCommand::SetTableColumns {
                        table_id: drag.table_id,
                        columns,
                    },
                    cx,
                );
            }
            cx.notify();
            return;
        }

        self.dragging = false;
        self.drag_origin = None;
        self.drag_anchor_pos = None;
        if !self.drag_moved
            && let Some(chip) = self.pending_chip.take()
        {
            let same_open = self.active_chip_popover.as_ref().is_some_and(|open| {
                open.block_id == chip.block_id && open.object_id == chip.object_id
            });
            if same_open {
                self.active_chip_popover = None;
            } else {
                cx.emit(RichDocumentEvent::SmartChipActivated {
                    block_id: chip.block_id,
                    object_id: chip.object_id,
                    kind: chip.kind,
                    label: chip.label.clone(),
                });
                self.active_chip_popover = Some(chip);
            }
            cx.notify();
        } else {
            self.pending_chip = None;
        }

        if !self.drag_moved
            && let Some(href) = self.pending_link.take()
        {
            self.dispatch_command(RichCommand::OpenLink(href), cx);
        } else {
            self.pending_link = None;
        }
        if let Some(id) = self.pending_footnote.take()
            && !self.drag_moved
        {
            self.dispatch_command(
                RichCommand::Anchor(crate::model::AnchorCommand::RevealFootnote { id }),
                cx,
            );
        }
        self.drag_moved = false;
    }

    pub(super) fn word_bounds_at(&self, pos: &DocPos) -> Option<(usize, usize)> {
        let block_id = pos.leaf_block_id()?;
        let inline = self.document.find_block(block_id)?.kind.inline_content()?;
        for (start, word) in inline.text.split_word_bound_indices() {
            let end = start + word.len();
            if pos.offset >= start && pos.offset <= end {
                return Some((start, end));
            }
        }
        None
    }

    /// Returns `(active_block_id, committed_text, active_effective_text)` for active-block IME scoping.
    fn ime_active_block_text(&self) -> Option<(BlockId, String, String)> {
        let block_id = self
            .ime_preedit
            .as_ref()
            .map(|preedit| preedit.block_id)
            .or_else(|| {
                self.active_text_selection()
                    .and_then(|(_, focus, _)| focus.leaf_block_id())
            })?;
        let committed = self
            .document
            .find_block(block_id)?
            .kind
            .inline_content()?
            .text
            .clone();
        let effective = match self.ime_preedit.as_ref() {
            Some(preedit) if preedit.block_id == block_id => {
                let start = preedit.replace_range.start.min(committed.len());
                let end = preedit.replace_range.end.min(committed.len()).max(start);
                let mut combined =
                    String::with_capacity(committed.len() - (end - start) + preedit.text.len());
                combined.push_str(committed.get(..start).unwrap_or(""));
                combined.push_str(&preedit.text);
                combined.push_str(committed.get(end..).unwrap_or(""));
                combined
            }
            _ => committed.clone(),
        };
        Some((block_id, committed, effective))
    }

    fn map_effective_range_to_committed(
        effective_range: Range<usize>,
        preedit: Option<&ImePreedit>,
        committed_len: usize,
    ) -> Range<usize> {
        let Some(preedit) = preedit else {
            let start = effective_range.start.min(committed_len);
            let end = effective_range.end.min(committed_len).max(start);
            return start..end;
        };
        let preedit_start = preedit.replace_range.start.min(committed_len);
        let preedit_end_committed = preedit
            .replace_range
            .end
            .min(committed_len)
            .max(preedit_start);
        let preedit_end_effective = preedit_start + preedit.text.len();

        let map_point = |offset: usize, is_end: bool| -> usize {
            if offset <= preedit_start {
                offset
            } else if offset >= preedit_end_effective {
                (preedit_end_committed + (offset - preedit_end_effective)).min(committed_len)
            } else if is_end {
                preedit_end_committed
            } else {
                preedit_start
            }
        };

        let start = map_point(effective_range.start, false);
        let end = map_point(effective_range.end, true).max(start);
        start..end
    }
}

impl EntityInputHandler for RichDocumentView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (_, _, effective_text) = self.ime_active_block_text()?;
        let start = utf16_to_utf8_offset(&effective_text, range_utf16.start);
        let end = utf16_to_utf8_offset(&effective_text, range_utf16.end).max(start);
        adjusted_range.replace(
            utf8_to_utf16_offset(&effective_text, start)
                ..utf8_to_utf16_offset(&effective_text, end),
        );
        effective_text.get(start..end).map(ToOwned::to_owned)
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.policy.editing.enabled && !ignore_disabled_input {
            return None;
        }
        let (block_id, committed, effective_text) = self.ime_active_block_text()?;
        if let Some(preedit) = self.ime_preedit.as_ref()
            && preedit.block_id == block_id
        {
            let start = preedit.replace_range.start + preedit.selected_range.start;
            let end = preedit.replace_range.start + preedit.selected_range.end;
            return Some(UTF16Selection {
                range: utf8_to_utf16_offset(&effective_text, start)
                    ..utf8_to_utf16_offset(&effective_text, end),
                reversed: false,
            });
        }

        let (anchor, focus, _) = self.active_text_selection()?;
        let (start, end, reversed) = if anchor.leaf_block_id() == Some(block_id) {
            if anchor.offset <= focus.offset {
                (
                    anchor.offset.min(committed.len()),
                    focus.offset.min(committed.len()),
                    false,
                )
            } else {
                (
                    focus.offset.min(committed.len()),
                    anchor.offset.min(committed.len()),
                    true,
                )
            }
        } else {
            let clamped = focus.offset.min(committed.len());
            (clamped, clamped, false)
        };
        Some(UTF16Selection {
            range: utf8_to_utf16_offset(&committed, start)..utf8_to_utf16_offset(&committed, end),
            reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let preedit = self.ime_preedit.as_ref()?;
        let (block_id, _, effective_text) = self.ime_active_block_text()?;
        if preedit.block_id != block_id {
            return None;
        }
        let start = preedit.replace_range.start.min(effective_text.len());
        let end = (start + preedit.text.len()).min(effective_text.len());
        Some(
            utf8_to_utf16_offset(&effective_text, start)
                ..utf8_to_utf16_offset(&effective_text, end),
        )
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(preedit) = self.ime_preedit.take() else {
            return;
        };
        if preedit.text.is_empty() {
            cx.notify();
            return;
        }
        let preserved_marks = self.active_text_selection().and_then(|(_, _, marks)| marks);
        if let (Some(anchor), Some(focus)) = (
            self.document
                .doc_pos_for_block(preedit.block_id, preedit.replace_range.start),
            self.document
                .doc_pos_for_block(preedit.block_id, preedit.replace_range.end),
        ) {
            self.selection = Some(DocumentSelection::Text {
                anchor,
                focus,
                affinity: Affinity::Downstream,
                goal_x: None,
                stored_marks: preserved_marks,
            });
        }
        self.history.break_coalescing();
        self.dispatch_command(RichCommand::InsertText(preedit.text), cx);
    }

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.policy.editing.enabled {
            return;
        }
        self.ime_preedit = None;
        self.paste_clipboard_item(&item, false, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.policy.editing.enabled {
            return;
        }
        let preserved_marks = self.active_text_selection().and_then(|(_, _, marks)| marks);
        let active_info = self.ime_active_block_text();
        let existing_preedit = self.ime_preedit.take();
        let had_preedit = existing_preedit.is_some();

        if let Some((block_id, committed, effective_text)) = active_info {
            let explicit_committed_range = if let Some(u16_range) = range_utf16 {
                let eff_start = utf16_to_utf8_offset(&effective_text, u16_range.start);
                let eff_end = utf16_to_utf8_offset(&effective_text, u16_range.end).max(eff_start);
                Some(Self::map_effective_range_to_committed(
                    eff_start..eff_end,
                    existing_preedit.as_ref().filter(|p| p.block_id == block_id),
                    committed.len(),
                ))
            } else {
                existing_preedit
                    .as_ref()
                    .filter(|p| p.block_id == block_id)
                    .map(|p| p.replace_range.clone())
            };

            if let Some(committed_range) = explicit_committed_range
                && let (Some(anchor), Some(focus)) = (
                    self.document
                        .doc_pos_for_block(block_id, committed_range.start),
                    self.document
                        .doc_pos_for_block(block_id, committed_range.end),
                )
            {
                self.selection = Some(DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: preserved_marks,
                });
            }
        }

        if had_preedit {
            self.history.break_coalescing();
        }
        if new_text.is_empty()
            && self
                .selection
                .as_ref()
                .is_none_or(DocumentSelection::is_collapsed)
        {
            cx.notify();
            return;
        }
        self.dispatch_command(RichCommand::InsertText(new_text.to_owned()), cx);
        if had_preedit {
            self.history.break_coalescing();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.policy.editing.enabled {
            return;
        }
        self.history.break_coalescing();
        let Some((block_id, committed, effective_text)) = self.ime_active_block_text() else {
            return;
        };
        let existing_preedit = self
            .ime_preedit
            .as_ref()
            .filter(|preedit| preedit.block_id == block_id)
            .cloned();

        let replace_range = if let Some(u16_range) = range_utf16 {
            let eff_start = utf16_to_utf8_offset(&effective_text, u16_range.start);
            let eff_end = utf16_to_utf8_offset(&effective_text, u16_range.end).max(eff_start);
            Self::map_effective_range_to_committed(
                eff_start..eff_end,
                existing_preedit.as_ref(),
                committed.len(),
            )
        } else if let Some(preedit) = existing_preedit {
            preedit.replace_range
        } else if let Some((anchor, focus, _)) = self.active_text_selection() {
            if anchor.leaf_block_id() == Some(block_id) {
                let start = anchor.offset.min(focus.offset).min(committed.len());
                let end = anchor.offset.max(focus.offset).min(committed.len());
                start..end
            } else {
                let clamped = focus.offset.min(committed.len());
                clamped..clamped
            }
        } else {
            return;
        };

        if new_text.is_empty() {
            self.ime_preedit = None;
            self.reset_caret_blink(cx);
            cx.notify();
            return;
        }

        let selected_range = match new_selected_range_utf16 {
            Some(sel_u16) => {
                let start = utf16_to_utf8_offset(new_text, sel_u16.start);
                let end = utf16_to_utf8_offset(new_text, sel_u16.end).max(start);
                start..end
            }
            None => new_text.len()..new_text.len(),
        };

        self.ime_preedit = Some(ImePreedit {
            block_id,
            replace_range,
            text: new_text.to_owned(),
            selected_range,
        });
        self.reset_caret_blink(cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (block_id, committed, effective_text) = self.ime_active_block_text()?;
        let eff_start = utf16_to_utf8_offset(&effective_text, range_utf16.start);
        let eff_end = utf16_to_utf8_offset(&effective_text, range_utf16.end).max(eff_start);
        let committed_range = Self::map_effective_range_to_committed(
            eff_start..eff_end,
            self.ime_preedit
                .as_ref()
                .filter(|preedit| preedit.block_id == block_id),
            committed.len(),
        );

        let viewport_width: f32 = bounds.size.width.into();
        let mut shaper = GpuiShaper::with_extensions(window, &self.extensions)
            .with_components(&self.document.components)
            .with_optional_font_resolver(self.font_resolver.as_deref());
        let canvas_rect = self.flow.range_bounds_in_block(
            block_id,
            committed_range,
            &self.document,
            viewport_width.max(200.0),
            self.zoom,
            &mut shaper,
        )?;
        let origin_x: f32 = bounds.origin.x.into();
        let origin_y: f32 = bounds.origin.y.into();
        Some(Bounds::new(
            point(
                px(origin_x + canvas_rect.x - self.scroll_x),
                px(origin_y + canvas_rect.y - self.scroll_y),
            ),
            size(
                px(canvas_rect.width.max(2.0)),
                px(canvas_rect.height.max(14.0)),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: GpuiPoint<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (active_block_id, committed, _) = self.ime_active_block_text()?;
        let canvas_point = self.window_point_to_canvas(position);
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let mut shaper = GpuiShaper::with_extensions(window, &self.extensions)
            .with_components(&self.document.components)
            .with_optional_font_resolver(self.font_resolver.as_deref());
        let hit = self.flow.hit_test_canvas(
            canvas_point,
            &self.document,
            viewport_width.max(200.0),
            self.zoom,
            &mut shaper,
        )?;
        if hit.pos.leaf_block_id() != Some(active_block_id) {
            return None;
        }
        Some(utf8_to_utf16_offset(&committed, hit.pos.offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((block_id, committed, effective_text)) = self.ime_active_block_text() else {
            return;
        };
        let eff_start = utf16_to_utf8_offset(&effective_text, range_utf16.start);
        let eff_end = utf16_to_utf8_offset(&effective_text, range_utf16.end).max(eff_start);
        let committed_range = Self::map_effective_range_to_committed(
            eff_start..eff_end,
            self.ime_preedit
                .as_ref()
                .filter(|preedit| preedit.block_id == block_id),
            committed.len(),
        );
        if let (Some(anchor), Some(focus)) = (
            self.document
                .doc_pos_for_block(block_id, committed_range.start),
            self.document
                .doc_pos_for_block(block_id, committed_range.end),
        ) {
            self.dispatch_command(
                RichCommand::SetSelection(Some(Box::new(DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: None,
                }))),
                cx,
            );
        }
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (_, _, effective_text) = self.ime_active_block_text()?;
        Some(effective_text.encode_utf16().count())
    }
}

fn utf8_to_utf16_offset(text: &str, byte_offset: usize) -> usize {
    let mut clamped = byte_offset.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    text.get(..clamped)
        .map(|prefix| prefix.encode_utf16().count())
        .unwrap_or(0)
}

fn utf16_to_utf8_offset(text: &str, utf16_offset: usize) -> usize {
    let mut units = 0usize;
    for (byte_index, character) in text.char_indices() {
        if units >= utf16_offset {
            return byte_index;
        }
        units += character.len_utf16();
    }
    text.len()
}

pub(super) fn previous_grapheme_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    text.get(..clamped)
        .and_then(|prefix| prefix.grapheme_indices(true).next_back())
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(super) fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    text.get(clamped..)
        .and_then(|suffix| suffix.graphemes(true).next())
        .map(|grapheme| clamped + grapheme.len())
        .unwrap_or(text.len())
}

pub(super) fn previous_word_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    let Some(prefix) = text.get(..clamped) else {
        return 0;
    };
    let mut candidates = prefix.split_word_bound_indices().rev();
    for (start, segment) in &mut candidates {
        if !segment.chars().all(char::is_whitespace) {
            return start;
        }
    }
    0
}

pub(super) fn next_word_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    let Some(suffix) = text.get(clamped..) else {
        return text.len();
    };
    for (start, segment) in suffix.split_word_bound_indices() {
        if !segment.chars().all(char::is_whitespace) {
            return clamped + start + segment.len();
        }
    }
    text.len()
}

pub(super) fn is_external_uri(href: &str) -> bool {
    href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:")
}

pub(super) fn table_cell_of(pos: &DocPos) -> Option<(BlockId, usize, usize)> {
    pos.path.iter().find_map(|segment| match *segment {
        PathSegment::TableCell { table_id, row, col } => Some((table_id, row, col)),
        PathSegment::Block(_) => None,
    })
}

/// Returns the 4 corner resize handle rectangles (centered on each corner) for an inline image.
pub(crate) fn image_resize_handle_rects(image_rect: Rect) -> [(ImageResizeCorner, Rect); 4] {
    let handle_size = 9.0;
    let half = handle_size * 0.5;
    [
        (
            ImageResizeCorner::TopLeft,
            Rect::new(
                image_rect.x - half,
                image_rect.y - half,
                handle_size,
                handle_size,
            ),
        ),
        (
            ImageResizeCorner::TopRight,
            Rect::new(
                image_rect.right() - half,
                image_rect.y - half,
                handle_size,
                handle_size,
            ),
        ),
        (
            ImageResizeCorner::BottomLeft,
            Rect::new(
                image_rect.x - half,
                image_rect.bottom() - half,
                handle_size,
                handle_size,
            ),
        ),
        (
            ImageResizeCorner::BottomRight,
            Rect::new(
                image_rect.right() - half,
                image_rect.bottom() - half,
                handle_size,
                handle_size,
            ),
        ),
    ]
}

fn hit_test_image_resize_handle(image_rect: Rect, point: Point) -> Option<ImageResizeCorner> {
    for (corner, rect) in image_resize_handle_rects(image_rect) {
        let hit_rect = Rect::new(
            rect.x - 3.0,
            rect.y - 3.0,
            rect.width + 6.0,
            rect.height + 6.0,
        );
        if hit_rect.contains(point) {
            return Some(corner);
        }
    }
    None
}

/// Promotes drags across distinct cells of the same table into rectangular `TableCells` selection
/// (using visual grid coordinates so spanned cells align cleanly), while keeping single-cell drags
/// and cross-block prose drags as `Text` selections.
fn selection_from_drag_positions(
    document: &Document,
    anchor: DocPos,
    focus: DocPos,
    affinity: Affinity,
) -> DocumentSelection {
    if let (
        Some((anchor_table, anchor_row, anchor_cell_idx)),
        Some((focus_table, focus_row, focus_cell_idx)),
    ) = (table_cell_of(&anchor), table_cell_of(&focus))
        && anchor_table == focus_table
        && (anchor_row, anchor_cell_idx) != (focus_row, focus_cell_idx)
    {
        let (anchor_visual, focus_visual) = match document
            .find_block(anchor_table)
            .and_then(|block| block.kind.table_block())
        {
            Some(table) => {
                let a_col = table
                    .placement_for_stored_cell(anchor_row, anchor_cell_idx)
                    .map(|p| p.visual_col)
                    .unwrap_or(anchor_cell_idx);
                let f_col = table
                    .placement_for_stored_cell(focus_row, focus_cell_idx)
                    .map(|p| p.visual_col)
                    .unwrap_or(focus_cell_idx);
                ((anchor_row, a_col), (focus_row, f_col))
            }
            None => ((anchor_row, anchor_cell_idx), (focus_row, focus_cell_idx)),
        };
        return DocumentSelection::TableCells {
            table_id: anchor_table,
            anchor_cell: anchor_visual,
            focus_cell: focus_visual,
        };
    }

    DocumentSelection::Text {
        anchor,
        focus,
        affinity,
        goal_x: None,
        stored_marks: None,
    }
}

pub(super) fn extract_clipboard_html(item: &ClipboardItem) -> Option<String> {
    let raw_metadata = item.metadata()?;
    let parsed: serde_json::Value = serde_json::from_str(raw_metadata).ok()?;
    parsed
        .get(CLIPBOARD_HTML_METADATA_KEY)
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

pub(super) fn extract_clipboard_fragment(item: &ClipboardItem) -> Option<ClipboardFragment> {
    let raw_metadata = item.metadata()?;
    let parsed: serde_json::Value = serde_json::from_str(raw_metadata).ok()?;
    let encoded = parsed
        .get(CLIPBOARD_FRAGMENT_METADATA_KEY)
        .and_then(|value| value.as_str())?;
    ClipboardFragment::from_versioned_json(encoded).ok()
}

pub(super) fn last_text_pos_in_nodes(nodes: &[BlockNode]) -> Option<(BlockId, usize)> {
    for node in nodes.iter().rev() {
        if let Some(inline) = node.kind.inline_content() {
            return Some((node.id, inline.len()));
        }
        match &node.kind {
            BlockKind::Container { children, .. } => {
                if let Some(found) = last_text_pos_in_nodes(children) {
                    return Some(found);
                }
            }
            BlockKind::Grid { table, .. } => {
                for row in table.rows.iter().rev() {
                    for cell in row.cells.iter().rev() {
                        if let Some(found) = last_text_pos_in_nodes(&cell.blocks) {
                            return Some(found);
                        }
                    }
                }
            }
            BlockKind::Text { .. } | BlockKind::Leaf { .. } => {}
        }
    }
    None
}
