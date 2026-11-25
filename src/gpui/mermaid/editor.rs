//! Interactive inline editors for Mermaid blocks:
//! - [`MermaidSourceEditor`]: live-syncing plain-text source editor used in `Source` and `Split` modes.
//! - [`MermaidLabelEditor`]: compact single-line overlay editor for editing text labels directly on
//!   a rendered Mermaid diagram.

use crate::gpui::theme::RichTheme;
use crate::gpui::view::{RichDocumentEvent, RichDocumentView};
use crate::layout::pt_to_px;
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, Document, Edges, HorizontalLayout, InlineContent,
    InlineStyle, InteractionPolicy, LineHeight, MermaidSpec, MermaidViewMode, NamedStyle,
    RichCommand, SetMermaidViewMode, StyleId, UpdateMermaidSource, replace_mermaid_label,
};
use gpui_kit::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Render, Styled, Subscription, WeakEntity, Window, div, px,
};

fn build_source_document(source: &str) -> Document {
    let mut document = Document::new();
    document.config.horizontal_layout = HorizontalLayout::Fill;
    document.config.fill_margins_pt = Edges::new(8.0, 12.0, 10.0, 12.0);
    document.config.default_style.font_family = "monospace".into();
    document.config.default_style.font_size_pt = 10.0;
    document.stylesheet.styles.insert(
        StyleId::NORMAL,
        NamedStyle {
            id: StyleId::NORMAL,
            name: "Code".into(),
            text: InlineStyle::default()
                .with_font_family("monospace")
                .with_font_size(10.0),
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            line_height: LineHeight::Multiplier(1.45),
        },
    );
    for line in source.split('\n') {
        let block_id = document.alloc_block_id();
        document.push_block(BlockNode::new(
            block_id,
            BlockKind::paragraph(InlineContent::plain(line)),
            BlockStyle::default(),
        ));
    }
    document
}

fn build_label_document(text: &str) -> Document {
    let mut document = Document::new();
    document.config.horizontal_layout = HorizontalLayout::Fill;
    document.config.fill_margins_pt = Edges::new(2.5, 6.0, 2.5, 6.0);
    document.config.default_style.font_size_pt = 9.5;
    document.stylesheet.styles.insert(
        StyleId::NORMAL,
        NamedStyle {
            id: StyleId::NORMAL,
            name: "Label".into(),
            text: InlineStyle::default().with_font_size(9.5),
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            line_height: LineHeight::Multiplier(1.25),
        },
    );
    let single_line = text.replace("\r\n", "\n").replace('\n', " ");
    let block_id = document.alloc_block_id();
    document.push_block(BlockNode::new(
        block_id,
        BlockKind::paragraph(InlineContent::plain(single_line)),
        BlockStyle::default(),
    ));
    document
}

fn source_editor_theme(theme: &RichTheme) -> RichTheme {
    let mut editor_theme = theme.clone();
    editor_theme.surface_background = theme.code_background;
    editor_theme
}

fn label_editor_theme(theme: &RichTheme) -> RichTheme {
    theme.clone()
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

/// Live-syncing plain-text source editor wrapping an embedded [`RichDocumentView`] for one Mermaid
/// diagram block.
pub(crate) struct MermaidSourceEditor {
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

impl MermaidSourceEditor {
    pub(crate) fn new(
        block_id: BlockId,
        source: &str,
        theme: &RichTheme,
        editable: bool,
        zoom: f32,
        parent_view: WeakEntity<RichDocumentView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = build_source_document(source);
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
            // Reflect external updates (undo/redo or direct diagram label edits) without echoing
            // another UpdateMermaidSource command back to the parent document.
            self.syncing_from_parent = true;
            self.last_synced_source = source.to_owned();
            let replacement = build_source_document(source);
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

    pub(crate) fn current_source(&self, cx: &App) -> String {
        let document = &self.editor_view.read(cx).document;
        let mut lines = Vec::new();
        for block in &document.blocks {
            if let Some(content) = block.kind.inline_content() {
                lines.push(content.text.as_str());
            }
        }
        lines.join("\n")
    }

    fn sync_to_parent(&mut self, cx: &mut Context<Self>) {
        let next_source = self.current_source(cx);
        if next_source == self.last_synced_source {
            return;
        }
        self.last_synced_source = next_source.clone();
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    UpdateMermaidSource {
                        block_id,
                        source: next_source,
                    },
                    cx,
                );
            });
        }
    }

    pub(crate) fn close_source_view(&self, window: &mut Window, cx: &mut Context<Self>) {
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    SetMermaidViewMode {
                        block_id,
                        mode: MermaidViewMode::Diagram,
                    },
                    cx,
                );
                view.invalidate_block_layout(block_id, cx);
                window.focus(&view.focus_handle, cx);
            });
        }
    }
}

impl Render for MermaidSourceEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_source_view(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.editor_view.clone())
    }
}

/// Compact inline single-line overlay editor positioned directly over a clicked diagram text label.
pub(crate) struct MermaidLabelEditor {
    block_id: BlockId,
    original_label: String,
    occurrence: usize,
    parent_view: WeakEntity<RichDocumentView>,
    editor_view: Entity<RichDocumentView>,
    on_dismiss: Box<dyn Fn(&mut App)>,
    theme: RichTheme,
    zoom: f32,
    finished: bool,
    _blur_subscription: Subscription,
}

impl MermaidLabelEditor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        block_id: BlockId,
        original_label: String,
        initial_text: String,
        occurrence: usize,
        theme: &RichTheme,
        zoom: f32,
        parent_view: WeakEntity<RichDocumentView>,
        on_dismiss: Box<dyn Fn(&mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = build_label_document(&initial_text);
        let policy = plain_text_editor_policy(true);
        let editor_theme = label_editor_theme(theme);
        let editor_view = cx.new(|cx| {
            let mut view = RichDocumentView::new(document, cx)
                .with_policy(policy)
                .with_theme(editor_theme);
            view.zoom = zoom;
            view
        });
        let focus_handle = editor_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        editor_view.update(cx, |view, cx| {
            view.dispatch_command(RichCommand::SelectAll, cx);
        });
        let blur_subscription = cx.on_blur(&focus_handle, window, |this, window, cx| {
            this.commit(false, window, cx);
        });
        Self {
            block_id,
            original_label,
            occurrence,
            parent_view,
            editor_view,
            on_dismiss,
            theme: theme.clone(),
            zoom,
            finished: false,
            _blur_subscription: blur_subscription,
        }
    }

    pub(crate) fn sync(&mut self, theme: &RichTheme, zoom: f32, cx: &mut Context<Self>) {
        let theme_changed = self.theme != *theme;
        let zoom_changed = (self.zoom - zoom).abs() > 0.001;
        if !theme_changed && !zoom_changed {
            return;
        }
        self.theme = theme.clone();
        self.zoom = zoom;
        let next_theme = label_editor_theme(theme);
        self.editor_view.update(cx, |view, cx| {
            if theme_changed {
                view.set_theme(next_theme, cx);
            }
            if zoom_changed {
                view.zoom = zoom;
                cx.notify();
            }
        });
        cx.notify();
    }

    fn current_text(&self, cx: &App) -> String {
        let document = &self.editor_view.read(cx).document;
        let mut lines = Vec::new();
        for block in &document.blocks {
            if let Some(content) = block.kind.inline_content() {
                lines.push(content.text.as_str());
            }
        }
        lines.join("\n")
    }

    pub(crate) fn commit(
        &mut self,
        return_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.finished {
            return;
        }
        self.finished = true;
        let next_text = self.current_text(cx);
        let block_id = self.block_id;
        let original_label = self.original_label.clone();
        let occurrence = self.occurrence;
        (self.on_dismiss)(cx);

        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                let updated = view
                    .document
                    .find_block(block_id)
                    .and_then(|block| block.kind.read_data::<MermaidSpec>().ok())
                    .and_then(|data| {
                        replace_mermaid_label(&data.source, &original_label, occurrence, &next_text)
                    });
                if let Some(source) = updated {
                    view.dispatch_extension_command(UpdateMermaidSource { block_id, source }, cx);
                }
                if return_focus {
                    window.focus(&view.focus_handle, cx);
                }
                cx.notify();
            });
        }
    }

    pub(crate) fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        (self.on_dismiss)(cx);
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                window.focus(&view.focus_handle, cx);
                cx.notify();
            });
        }
    }
}

impl Render for MermaidLabelEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_hidden()
            .rounded(px(pt_to_px(3.5, self.zoom)))
            .border_1()
            .border_color(self.theme.accent)
            .bg(self.theme.surface_background)
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "enter" || event.keystroke.key == "tab" {
                    this.commit(true, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.editor_view.clone())
    }
}
