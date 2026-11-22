//! Interactive GPUI view hosting [`RichDocumentElement`] with scrolling, zoom, selection, and copy.

use crate::element::RichDocumentElement;
use crate::shaper::{GpuiGlyphBackend, GpuiShaper};
use gpui::{
    Bounds, ClipboardItem, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point as GpuiPoint, Render, ScrollDelta, ScrollWheelEvent, Styled, Window, div,
};
use rich_layout::{FlowLayout, Point, resolve_horizontal_metrics};
use rich_model::{
    BlockId, BlockKind, DocPos, Document, DocumentSelection, HorizontalLayout, PathSegment, Step,
    Transaction,
};
use unicode_segmentation::UnicodeSegmentation;

/// Interactive viewer and future editor host for a non-paginated rich text document.
pub struct RichDocumentView {
    pub document: Document,
    pub flow: FlowLayout<GpuiGlyphBackend>,
    pub selection: Option<DocumentSelection>,
    pub focus_handle: FocusHandle,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub zoom: f32,
    pub debug_overlay: bool,
    pub(crate) last_viewport_bounds: Bounds<Pixels>,
    pub(crate) hovered_pointer: bool,
    dragging: bool,
    drag_moved: bool,
    drag_origin: Option<Point>,
    drag_anchor_pos: Option<DocPos>,
    pending_link: Option<String>,
}

impl RichDocumentView {
    /// Creates a new view wrapping `document`.
    pub fn new(document: Document, cx: &mut Context<Self>) -> Self {
        Self {
            document,
            flow: FlowLayout::default(),
            selection: None,
            focus_handle: cx.focus_handle(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            zoom: 1.0,
            debug_overlay: false,
            last_viewport_bounds: Bounds::default(),
            hovered_pointer: false,
            dragging: false,
            drag_moved: false,
            drag_origin: None,
            drag_anchor_pos: None,
            pending_link: None,
        }
    }

    /// Adjusts zoom within `[0.5, 2.0]` while preserving the top visible scroll anchor.
    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let clamped = zoom.clamp(0.5, 2.0);
        if (clamped - self.zoom).abs() < 0.001 {
            return;
        }
        let anchor = self.flow.capture_anchor(self.scroll_y);
        self.zoom = clamped;
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
        if viewport_width > 1.0 {
            self.flow
                .sync_document(&self.document, viewport_width, self.zoom);
            if let Some(captured) = anchor {
                self.scroll_y = self.flow.restore_anchor(captured, viewport_height);
            }
            let horizontal =
                resolve_horizontal_metrics(viewport_width, &self.document.config, self.zoom);
            let max_scroll_x = (horizontal.canvas_width - viewport_width).max(0.0);
            self.scroll_x = self.scroll_x.clamp(0.0, max_scroll_x);
        }
        cx.notify();
    }

    /// Switches between viewport-filling reflow (`Fill`) and infinite-length `Paper` layout.
    pub fn set_horizontal_layout(&mut self, layout: HorizontalLayout, cx: &mut Context<Self>) {
        if self.document.config.horizontal_layout == layout {
            return;
        }
        let anchor = self.flow.capture_anchor(self.scroll_y);
        self.document.config.horizontal_layout = layout;
        self.scroll_x = 0.0;
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
        if viewport_width > 1.0 {
            self.flow
                .sync_document(&self.document, viewport_width, self.zoom);
            if let Some(captured) = anchor {
                self.scroll_y = self.flow.restore_anchor(captured, viewport_height);
            }
        }
        cx.notify();
    }

    /// Toggles the layout debug overlay showing visual line bounds and baselines.
    pub fn toggle_debug_overlay(&mut self, cx: &mut Context<Self>) {
        self.debug_overlay = !self.debug_overlay;
        cx.notify();
    }

    /// Extracts the plain UTF-8 text covered by the current selection (TSV for multi-cell table selections).
    pub fn selected_plain_text(&self) -> Option<String> {
        match self.selection.as_ref()? {
            DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                focus_cell,
            } => {
                let BlockKind::Table(table) = &self.document.find_block(*table_id)?.kind else {
                    return None;
                };
                let row_min = anchor_cell.0.min(focus_cell.0);
                let row_max = anchor_cell.0.max(focus_cell.0);
                let col_min = anchor_cell.1.min(focus_cell.1);
                let col_max = anchor_cell.1.max(focus_cell.1);
                let mut rows_output = Vec::new();
                for row_index in row_min..=row_max {
                    let Some(row) = table.rows.get(row_index) else {
                        continue;
                    };
                    let mut cols_output = Vec::new();
                    let mut col_cursor = 0usize;
                    for cell in &row.cells {
                        let span = cell.colspan.max(1) as usize;
                        if col_cursor >= col_min && col_cursor <= col_max {
                            let cell_text: Vec<&str> = cell
                                .blocks
                                .iter()
                                .filter_map(|block| block.kind.inline_content())
                                .map(|inline| inline.text.as_str())
                                .collect();
                            cols_output.push(cell_text.join(" "));
                        }
                        col_cursor += span;
                    }
                    rows_output.push(cols_output.join("\t"));
                }
                Some(rows_output.join("\n"))
            }
            DocumentSelection::Text { anchor, focus, .. } => {
                if anchor == focus {
                    return None;
                }
                let anchor_leaf_id = anchor.leaf_block_id()?;
                let focus_leaf_id = focus.leaf_block_id()?;
                let leaves = self.document.collect_leaf_blocks();
                let anchor_index = leaves.iter().position(|block| block.id == anchor_leaf_id)?;
                let focus_index = leaves.iter().position(|block| block.id == focus_leaf_id)?;

                let ((start_index, start_offset), (end_index, end_offset)) =
                    if (anchor_index, anchor.offset) <= (focus_index, focus.offset) {
                        ((anchor_index, anchor.offset), (focus_index, focus.offset))
                    } else {
                        ((focus_index, focus.offset), (anchor_index, anchor.offset))
                    };

                let mut output = String::new();
                for index in start_index..=end_index {
                    let Some(block) = leaves.get(index) else {
                        continue;
                    };
                    let Some(inline) = block.kind.inline_content() else {
                        continue;
                    };
                    let slice_start = if index == start_index {
                        start_offset.min(inline.text.len())
                    } else {
                        0
                    };
                    let slice_end = if index == end_index {
                        end_offset.min(inline.text.len())
                    } else {
                        inline.text.len()
                    };
                    if let Some(segment) = inline.text.get(slice_start..slice_end) {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(segment);
                    }
                }
                Some(output)
            }
        }
    }

    fn window_point_to_canvas(&self, position: GpuiPoint<Pixels>) -> Point {
        let origin_x: f32 = self.last_viewport_bounds.origin.x.into();
        let origin_y: f32 = self.last_viewport_bounds.origin.y.into();
        let px_x: f32 = position.x.into();
        let px_y: f32 = position.y.into();
        Point::new(
            px_x - origin_x + self.scroll_x,
            px_y - origin_y + self.scroll_y,
        )
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
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

        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
        let horizontal =
            resolve_horizontal_metrics(viewport_width, &self.document.config, self.zoom);
        let max_scroll_x = (horizontal.canvas_width - viewport_width).max(0.0);
        let max_scroll_y = (self.flow.total_height() - viewport_height).max(0.0);

        self.scroll_x = (self.scroll_x - delta_x).clamp(0.0, max_scroll_x);
        self.scroll_y = (self.scroll_y - delta_y).clamp(0.0, max_scroll_y);
        cx.notify();
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        let canvas_point = self.window_point_to_canvas(event.position);
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let mut shaper = GpuiShaper::new(window);
        let Some(hit) = self.flow.hit_test_canvas(
            canvas_point,
            &self.document,
            viewport_width,
            self.zoom,
            &mut shaper,
        ) else {
            return;
        };

        if let Some(heading_id) = hit.toggled_heading {
            let anchor = self.flow.capture_anchor(self.scroll_y);
            let _applied = self
                .document
                .apply(Transaction::new(vec![Step::ToggleHeadingCollapsed {
                    block_id: heading_id,
                }]));
            self.flow
                .sync_document(&self.document, viewport_width, self.zoom);
            if let Some(captured) = anchor {
                let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
                self.scroll_y = self.flow.restore_anchor(captured, viewport_height);
            }
            cx.notify();
            return;
        }

        if event.click_count >= 2 {
            self.pending_link = None;
            self.drag_origin = None;
            self.drag_moved = false;
            if let Some((word_start, word_end)) = self.word_bounds_at(&hit.pos) {
                let mut anchor_pos = hit.pos.clone();
                anchor_pos.offset = word_start;
                let mut focus_pos = hit.pos;
                focus_pos.offset = word_end;
                self.selection = Some(DocumentSelection::Text {
                    anchor: anchor_pos,
                    focus: focus_pos,
                    affinity: hit.affinity,
                    goal_x: None,
                    stored_marks: None,
                });
                self.dragging = false;
                cx.notify();
                return;
            }
        }

        self.pending_link = hit.hit.link.map(|link| link.href);
        self.drag_origin = Some(canvas_point);
        self.drag_moved = false;

        if event.modifiers.shift
            && let Some(DocumentSelection::Text { anchor, .. }) = self.selection.clone()
        {
            self.drag_anchor_pos = Some(anchor.clone());
            self.selection = Some(selection_from_drag_positions(anchor, hit.pos, hit.affinity));
        } else {
            self.drag_anchor_pos = Some(hit.pos.clone());
            self.selection = Some(DocumentSelection::caret(hit.pos, hit.affinity));
        }
        self.dragging = true;
        cx.notify();
    }

    fn handle_mouse_move(
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
        let mut shaper = GpuiShaper::new(window);
        let hit = self.flow.hit_test_canvas(
            canvas_point,
            &self.document,
            viewport_width,
            self.zoom,
            &mut shaper,
        );

        if !self.dragging || !event.dragging() {
            let pointer = hit
                .as_ref()
                .is_some_and(|result| result.toggled_heading.is_some() || result.hit.link.is_some());
            if pointer != self.hovered_pointer {
                self.hovered_pointer = pointer;
                cx.notify();
            }
            return;
        }

        if let Some(origin) = self.drag_origin {
            let delta_x = canvas_point.x - origin.x;
            let delta_y = canvas_point.y - origin.y;
            if delta_x * delta_x + delta_y * delta_y > 9.0 {
                self.drag_moved = true;
            }
        }

        let Some(hit) = hit else {
            return;
        };

        if let Some(anchor) = self.drag_anchor_pos.clone() {
            self.selection = Some(selection_from_drag_positions(anchor, hit.pos, hit.affinity));
            cx.notify();
        }
    }

    fn handle_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragging = false;
        self.drag_origin = None;
        self.drag_anchor_pos = None;
        if !self.drag_moved
            && let Some(href) = self.pending_link.take()
        {
            cx.open_url(&href);
        } else {
            self.pending_link = None;
        }
        self.drag_moved = false;
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let primary_modifier = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if !primary_modifier {
            return;
        }
        match event.keystroke.key.as_str() {
            "c" => {
                if let Some(text) = self.selected_plain_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            "=" | "+" => self.set_zoom(self.zoom + 0.1, cx),
            "-" => self.set_zoom(self.zoom - 0.1, cx),
            "0" => self.set_zoom(1.0, cx),
            _ => {}
        }
    }

    fn word_bounds_at(&self, pos: &DocPos) -> Option<(usize, usize)> {
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
}

fn table_cell_of(pos: &DocPos) -> Option<(BlockId, usize, usize)> {
    pos.path.iter().find_map(|segment| match *segment {
        PathSegment::TableCell { table_id, row, col } => Some((table_id, row, col)),
        PathSegment::Block(_) => None,
    })
}

/// Promotes drags across distinct cells of the same table into rectangular `TableCells` selection,
/// while keeping single-cell drags and cross-block prose drags as `Text` selections.
fn selection_from_drag_positions(
    anchor: DocPos,
    focus: DocPos,
    affinity: rich_model::Affinity,
) -> DocumentSelection {
    if let (Some((anchor_table, anchor_row, anchor_col)), Some((focus_table, focus_row, focus_col))) =
        (table_cell_of(&anchor), table_cell_of(&focus))
        && anchor_table == focus_table
        && (anchor_row, anchor_col) != (focus_row, focus_col)
    {
        return DocumentSelection::TableCells {
            table_id: anchor_table,
            anchor_cell: (anchor_row, anchor_col),
            focus_cell: (focus_row, focus_col),
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

impl Focusable for RichDocumentView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RichDocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(RichDocumentElement::new(cx.entity()))
    }
}
