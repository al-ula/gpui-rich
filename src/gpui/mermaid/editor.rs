//! Ephemeral inline Mermaid source editor hosted inside a [`crate::gpui::MermaidBlockRenderer`]
//! frame when a block is switched to [`crate::model::MermaidViewMode::Source`].

use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, Document, Edges, HorizontalLayout, InlineContent,
    InlineStyle, InteractionPolicy, LineHeight, MermaidViewMode, NamedStyle, SetMermaidViewMode,
    StyleId, UpdateMermaidSource,
};
use gpui_kit::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, Styled, WeakEntity, Window, div,
};

fn build_source_document(source: &str) -> Document {
    let mut document = Document::new();
    document.config.horizontal_layout = HorizontalLayout::Fill;
    document.config.fill_margins_pt = Edges::new(30.0, 12.0, 10.0, 12.0);
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

fn source_editor_theme(theme: &RichTheme) -> RichTheme {
    let mut editor_theme = theme.clone();
    editor_theme.surface_background = theme.code_background;
    editor_theme
}

fn source_editor_policy(editable: bool) -> InteractionPolicy {
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

/// Interactive draft editor wrapping an embedded plain-text [`RichDocumentView`] for one Mermaid
/// diagram block.
pub(crate) struct MermaidSourceEditor {
    block_id: BlockId,
    parent_view: WeakEntity<RichDocumentView>,
    editor_view: Entity<RichDocumentView>,
    editable: bool,
    theme: RichTheme,
    zoom: f32,
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
        let policy = source_editor_policy(editable);
        let editor_theme = source_editor_theme(theme);
        let editor_view = cx.new(|cx| {
            let mut view = RichDocumentView::new(document, cx)
                .with_policy(policy)
                .with_theme(editor_theme);
            view.zoom = zoom;
            view
        });
        Self {
            block_id,
            parent_view,
            editor_view,
            editable,
            theme: theme.clone(),
            zoom,
        }
    }

    pub(crate) fn sync(
        &mut self,
        theme: &RichTheme,
        editable: bool,
        zoom: f32,
        cx: &mut Context<Self>,
    ) {
        let theme_changed = self.theme != *theme;
        let editable_changed = self.editable != editable;
        let zoom_changed = (self.zoom - zoom).abs() > 0.001;
        if !theme_changed && !editable_changed && !zoom_changed {
            return;
        }
        self.theme = theme.clone();
        self.editable = editable;
        self.zoom = zoom;
        let next_theme = source_editor_theme(theme);
        self.editor_view.update(cx, |view, cx| {
            if theme_changed {
                view.set_theme(next_theme, cx);
            }
            if editable_changed {
                view.set_policy(source_editor_policy(editable), cx);
            }
            if zoom_changed {
                view.zoom = zoom;
                cx.notify();
            }
        });
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

    pub(crate) fn apply_draft(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable {
            self.close_source_view(window, cx);
            return;
        }
        let next_source = self.current_source(cx);
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                let _ = view.dispatch_extension_command(
                    UpdateMermaidSource {
                        block_id,
                        source: next_source,
                    },
                    cx,
                );
                let _ = view.dispatch_extension_command(
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

    pub(crate) fn close_source_view(&self, window: &mut Window, cx: &mut Context<Self>) {
        let block_id = self.block_id;
        if let Some(parent) = self.parent_view.upgrade() {
            parent.update(cx, |view, cx| {
                let _ = view.dispatch_extension_command(
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_source_view(window, cx);
                    cx.stop_propagation();
                } else if this.editable
                    && event.keystroke.key == "enter"
                    && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                {
                    this.apply_draft(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.editor_view.clone())
    }
}
