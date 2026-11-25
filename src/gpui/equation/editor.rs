//! Interactive inline editors for LaTeX equations:
//! - [`EquationBlockSourceEditor`]: live-syncing monospace source editor for display equation blocks.
//! - [`InlineEquationPopoverEditor`]: compact anchored popover editor for inline equations.

use crate::gpui::theme::RichTheme;
use crate::gpui::view::{RichDocumentEvent, RichDocumentView};
use crate::layout::pt_to_px;
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, CloseInlineEquationEditor, Document, Edges,
    EquationViewMode, HorizontalLayout, InlineContent, InlineObjectId, InlineStyle,
    InteractionPolicy, LineHeight, NamedStyle, SetEquationViewMode, StyleId, UpdateEquationBlock,
    UpdateInlineEquation,
};
use gpui_kit::{
    AppContext, BoxShadow, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, point, px,
};

fn build_source_document(source: &str, single_line: bool) -> Document {
    let mut document = Document::new();
    document.config.horizontal_layout = HorizontalLayout::Fill;
    document.config.fill_margins_pt = if single_line {
        Edges::new(4.0, 8.0, 4.0, 8.0)
    } else {
        Edges::new(6.0, 10.0, 8.0, 10.0)
    };
    document.config.default_style.font_family = "monospace".into();
    document.config.default_style.font_size_pt = 10.0;
    document.stylesheet.styles.insert(
        StyleId::NORMAL,
        NamedStyle {
            id: StyleId::NORMAL,
            name: "LaTeX".into(),
            text: InlineStyle::default()
                .with_font_family("monospace")
                .with_font_size(10.0),
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            line_height: LineHeight::Multiplier(1.4),
        },
    );
    if single_line {
        let normalized = source.replace("\r\n", " ").replace(['\n', '\r'], " ");
        let block_id = document.alloc_block_id();
        document.push_block(BlockNode::new(
            block_id,
            BlockKind::paragraph(InlineContent::plain(normalized)),
            BlockStyle::default(),
        ));
    } else {
        for line in source.split('\n') {
            let block_id = document.alloc_block_id();
            document.push_block(BlockNode::new(
                block_id,
                BlockKind::paragraph(InlineContent::plain(line)),
                BlockStyle::default(),
            ));
        }
    }
    document
}

fn plain_text_editor_policy(editable: bool) -> InteractionPolicy {
    if editable {
        let mut policy = InteractionPolicy::editable();
        policy.editing.formatting.enabled = false;
        policy.editing.tables.enabled = false;
        policy.editing.rich_paste = false;
        policy.editing.input_rules = false;
        policy.reading.heading_collapse = false;
        policy.reading.code_copy = false;
        policy
    } else {
        let mut policy = InteractionPolicy::viewer();
        policy.reading.heading_collapse = false;
        policy.reading.code_copy = false;
        policy
    }
}

fn source_editor_theme(theme: &RichTheme) -> RichTheme {
    let mut editor_theme = theme.clone();
    editor_theme.surface_background = theme.code_background;
    editor_theme
}

fn read_plain_source(view: &RichDocumentView) -> String {
    let mut lines = Vec::with_capacity(view.document.blocks.len());
    for block in &view.document.blocks {
        if let Some(content) = block.kind.inline_content() {
            lines.push(content.text.as_str());
        }
    }
    lines.join("\n")
}

/// Live-syncing plain-text LaTeX editor embedded in an expanded display equation block card.
pub(crate) struct EquationBlockSourceEditor {
    block_id: BlockId,
    parent_view: WeakEntity<RichDocumentView>,
    editor_view: Entity<RichDocumentView>,
    editable: bool,
    theme: RichTheme,
    zoom: f32,
    last_synced_source: String,
    syncing_from_parent: bool,
    _mutation_subscription: Subscription,
}

impl EquationBlockSourceEditor {
    pub(crate) fn new(
        block_id: BlockId,
        source: &str,
        theme: &RichTheme,
        editable: bool,
        zoom: f32,
        parent_view: WeakEntity<RichDocumentView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = build_source_document(source, false);
        let policy = plain_text_editor_policy(editable);
        let editor_theme = source_editor_theme(theme);
        let editor_view = cx.new(|cx| {
            let mut view = RichDocumentView::new(document, cx)
                .with_policy(policy)
                .with_theme(editor_theme);
            view.zoom = zoom;
            view
        });
        let mutation_subscription = cx.subscribe(
            &editor_view,
            |this, _editor, event: &RichDocumentEvent, cx| {
                if this.syncing_from_parent || !this.editable {
                    return;
                }
                if matches!(event, RichDocumentEvent::DocumentMutated { .. }) {
                    this.sync_to_parent(cx);
                }
            },
        );
        Self {
            block_id,
            parent_view,
            editor_view,
            editable,
            theme: theme.clone(),
            zoom,
            last_synced_source: source.to_owned(),
            syncing_from_parent: false,
            _mutation_subscription: mutation_subscription,
        }
    }

    pub(crate) fn sync(
        &mut self,
        source: &str,
        theme: &RichTheme,
        editable: bool,
        zoom: f32,
        cx: &mut Context<Self>,
    ) {
        let source_changed = self.last_synced_source != source;
        let theme_changed = self.theme != *theme;
        let editable_changed = self.editable != editable;
        let zoom_changed = (self.zoom - zoom).abs() > 0.001;
        if !source_changed && !theme_changed && !editable_changed && !zoom_changed {
            return;
        }
        self.theme = theme.clone();
        self.editable = editable;
        self.zoom = zoom;
        let next_theme = source_editor_theme(theme);

        if source_changed {
            self.syncing_from_parent = true;
            self.last_synced_source = source.to_owned();
            let replacement = build_source_document(source, false);
            self.editor_view.update(cx, |view, cx| {
                view.replace_document(replacement, cx);
            });
            self.syncing_from_parent = false;
        }

        if theme_changed || editable_changed || zoom_changed {
            self.editor_view.update(cx, |view, cx| {
                if theme_changed {
                    view.set_theme(next_theme, cx);
                }
                if editable_changed {
                    view.set_policy(plain_text_editor_policy(editable), cx);
                }
                if zoom_changed {
                    view.zoom = zoom;
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = self.editor_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
    }

    fn sync_to_parent(&mut self, cx: &mut Context<Self>) {
        let next_source = read_plain_source(self.editor_view.read(cx));
        if next_source == self.last_synced_source {
            return;
        }
        self.last_synced_source = next_source.clone();
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    UpdateEquationBlock {
                        block_id,
                        source: next_source,
                    },
                    cx,
                );
            });
        }
    }

    fn close_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    SetEquationViewMode {
                        block_id,
                        mode: EquationViewMode::Rendered,
                    },
                    cx,
                );
                view.invalidate_block_layout(block_id, cx);
                window.focus(&view.focus_handle, cx);
            });
        }
    }
}

impl Render for EquationBlockSourceEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_editor(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.editor_view.clone())
    }
}

/// Compact anchored popover editor for editing an inline LaTeX equation in place.
pub(crate) struct InlineEquationPopoverEditor {
    block_id: BlockId,
    object_id: InlineObjectId,
    initial_source: String,
    last_synced_source: String,
    parent_view: WeakEntity<RichDocumentView>,
    editor_view: Entity<RichDocumentView>,
    theme: RichTheme,
    zoom: f32,
    error_message: Option<String>,
    syncing_from_parent: bool,
    _mutation_subscription: Subscription,
}

impl InlineEquationPopoverEditor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        block_id: BlockId,
        object_id: InlineObjectId,
        source: &str,
        theme: &RichTheme,
        zoom: f32,
        parent_view: WeakEntity<RichDocumentView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = build_source_document(source, true);
        let policy = plain_text_editor_policy(true);
        let editor_theme = source_editor_theme(theme);
        let editor_view = cx.new(|cx| {
            let mut view = RichDocumentView::new(document, cx)
                .with_policy(policy)
                .with_theme(editor_theme);
            view.zoom = zoom;
            view
        });
        let focus_handle = editor_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);

        let mutation_subscription = cx.subscribe(
            &editor_view,
            |this, _editor, event: &RichDocumentEvent, cx| {
                if this.syncing_from_parent {
                    return;
                }
                if matches!(event, RichDocumentEvent::DocumentMutated { .. }) {
                    this.sync_to_parent(cx);
                }
            },
        );
        Self {
            block_id,
            object_id,
            initial_source: source.to_owned(),
            last_synced_source: source.to_owned(),
            parent_view,
            editor_view,
            theme: theme.clone(),
            zoom,
            error_message: None,
            syncing_from_parent: false,
            _mutation_subscription: mutation_subscription,
        }
    }

    pub(crate) fn sync(
        &mut self,
        source: &str,
        error_message: Option<String>,
        theme: &RichTheme,
        zoom: f32,
        cx: &mut Context<Self>,
    ) {
        let source_changed = self.last_synced_source != source;
        let error_changed = self.error_message != error_message;
        let theme_changed = self.theme != *theme;
        let zoom_changed = (self.zoom - zoom).abs() > 0.001;
        if !source_changed && !error_changed && !theme_changed && !zoom_changed {
            return;
        }
        self.error_message = error_message;
        self.theme = theme.clone();
        self.zoom = zoom;

        if source_changed {
            self.syncing_from_parent = true;
            self.last_synced_source = source.to_owned();
            let replacement = build_source_document(source, true);
            self.editor_view.update(cx, |view, cx| {
                view.replace_document(replacement, cx);
            });
            self.syncing_from_parent = false;
        }

        if theme_changed || zoom_changed {
            let next_theme = source_editor_theme(theme);
            self.editor_view.update(cx, |view, cx| {
                if theme_changed {
                    view.set_theme(next_theme, cx);
                }
                if zoom_changed {
                    view.zoom = zoom;
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = self.editor_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
    }

    fn sync_to_parent(&mut self, cx: &mut Context<Self>) {
        let next_source = read_plain_source(self.editor_view.read(cx))
            .replace("\r\n", " ")
            .replace(['\n', '\r'], " ");
        if next_source == self.last_synced_source {
            return;
        }
        self.last_synced_source = next_source.clone();
        let block_id = self.block_id;
        let object_id = self.object_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    UpdateInlineEquation {
                        block_id,
                        object_id,
                        source: next_source,
                    },
                    cx,
                );
            });
        }
    }

    fn commit_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_to_parent(cx);
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(CloseInlineEquationEditor, cx);
                window.focus(&view.focus_handle, cx);
            });
        }
    }

    fn cancel_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_synced_source != self.initial_source {
            let revert_source = self.initial_source.clone();
            self.last_synced_source = revert_source.clone();
            if let Some(parent) = self.parent_view.upgrade() {
                parent.update(cx, |view, cx| {
                    view.dispatch_extension_command(
                        UpdateInlineEquation {
                            block_id: self.block_id,
                            object_id: self.object_id,
                            source: revert_source,
                        },
                        cx,
                    );
                });
            }
        }
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(CloseInlineEquationEditor, cx);
                window.focus(&view.focus_handle, cx);
            });
        }
    }
}

impl Render for InlineEquationPopoverEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let width_px = pt_to_px(256.0, self.zoom).clamp(240.0, 420.0);
        let input_h = pt_to_px(26.0, self.zoom).max(26.0);

        let mut container = div()
            .id(("inline-equation-popover", self.object_id.0))
            .w(px(width_px))
            .p(px(8.0))
            .rounded(px(8.0))
            .bg(theme.surface_background)
            .border_1()
            .border_color(theme.paper_border)
            .shadow(vec![BoxShadow {
                color: theme.paper_shadow,
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(12.0),
                spread_radius: px(0.0),
                inset: false,
            }])
            .flex()
            .flex_col()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel_and_close(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "enter" {
                    this.commit_and_close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_text)
                            .child("LaTeX Equation"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id(("inline-eq-cancel", self.object_id.0))
                                    .px(px(6.0))
                                    .py(px(1.5))
                                    .rounded(px(4.0))
                                    .text_size(px(10.0))
                                    .text_color(theme.muted_text)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        this.cancel_and_close(window, cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .id(("inline-eq-done", self.object_id.0))
                                    .px(px(7.0))
                                    .py(px(1.5))
                                    .rounded(px(4.0))
                                    .bg(theme.accent)
                                    .text_size(px(10.0))
                                    .text_color(theme.accent_foreground)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        this.commit_and_close(window, cx);
                                    }))
                                    .child("Done"),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(input_h))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme.code_border)
                    .bg(theme.code_background)
                    .overflow_hidden()
                    .child(self.editor_view.clone()),
            );

        if let Some(error) = &self.error_message {
            let error_color = if theme.is_dark() {
                crate::gpui::shaper::rgba_to_hsla(crate::model::RgbaColor::rgb(0xff, 0x7b, 0x72))
            } else {
                crate::gpui::shaper::rgba_to_hsla(crate::model::RgbaColor::rgb(0xcf, 0x22, 0x2e))
            };
            container = container.child(
                div()
                    .text_size(px(10.0))
                    .text_color(error_color)
                    .child(error.clone()),
            );
        }

        container
    }
}
