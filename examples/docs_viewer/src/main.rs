//! Demo GPUI application showcasing the non-paginated `gpui-rich` renderer.

use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use rich_gpui::{
    CustomBlockRenderContext, CustomInlineRenderContext, CustomRenderOutput, NodeExtension,
    RichDocumentEvent, RichDocumentView, RichTheme, register_default_keybindings,
};
use rich_layout::{InlineBoxMetrics, Size, pt_to_px};
use rich_model::{
    BaselineShift, BlockDecorations, BlockKind, BlockNode, BlockStyle, BlockWidth, ChipKind,
    ColumnSpec, DecorationContext, DecorationProvider, Document, Edges, FontWeight,
    HorizontalLayout, InlineContent, InlineDecoration, InlineObject, InlineStyle, InlineVAlign,
    InteractionPolicy, ListId, ListKind, MarkKind, NumberStyle, PaperSpec, RgbaColor, RichCommand,
    SelectionValue, StyleId, TableBlock, TableBorderStyle, TableCell, TableRow, TextAlign,
    UnderlineKind,
};

struct RustSyntaxDecorationProvider;

impl DecorationProvider for RustSyntaxDecorationProvider {
    fn decorations_for_block(&self, context: &DecorationContext<'_>) -> BlockDecorations {
        let BlockKind::CodeBlock {
            language: Some(language),
            content,
        } = &context.block.kind
        else {
            return BlockDecorations::default();
        };
        if !language.eq_ignore_ascii_case("rust") {
            return BlockDecorations::default();
        }

        let keyword_color = RgbaColor::rgb(0xcf, 0x22, 0x2e);
        let type_color = RgbaColor::rgb(0x82, 0x50, 0xdf);
        let mut decorations = BlockDecorations::default();
        let text = content.text.as_str();
        let mut start: Option<usize> = None;

        for (index, character) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
            let is_ident = character.is_ascii_alphanumeric() || character == '_';
            match (start, is_ident) {
                (None, true) => start = Some(index),
                (Some(word_start), false) => {
                    if let Some(word) = text.get(word_start..index) {
                        if matches!(
                            word,
                            "pub" | "trait" | "type" | "fn" | "mut" | "let" | "struct" | "impl"
                        ) {
                            decorations.inline.push(
                                InlineDecoration::new(word_start..index)
                                    .with_foreground(keyword_color),
                            );
                        } else if word
                            .chars()
                            .next()
                            .is_some_and(|first| first.is_ascii_uppercase())
                        {
                            decorations.inline.push(
                                InlineDecoration::new(word_start..index)
                                    .with_foreground(type_color),
                            );
                        }
                    }
                    start = None;
                }
                _ => {}
            }
        }
        decorations
    }
}

struct CalloutBlockExtension;

impl NodeExtension for CalloutBlockExtension {
    fn measure_block(
        &self,
        _data: &serde_json::Value,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width, pt_to_px(58.0, zoom))
    }

    fn render_block(
        &self,
        context: CustomBlockRenderContext<'_>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let title = context
            .data
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("Callout")
            .to_owned();
        let body = context
            .data
            .get("body")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();
        let is_dark = context.theme.is_dark();
        let card_bg = if is_dark { rgb(0x162235) } else { rgb(0xedf5ff) };
        let card_border = if is_dark { rgb(0x388bfd) } else { rgb(0x8ab4f8) };
        let title_color = if is_dark { rgb(0x79c0ff) } else { rgb(0x0b57d0) };
        let body_color = context.theme.text;

        let element = div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(pt_to_px(2.0, context.zoom)))
            .px(px(pt_to_px(12.0, context.zoom)))
            .py(px(pt_to_px(8.0, context.zoom)))
            .rounded(px(pt_to_px(6.0, context.zoom)))
            .bg(card_bg)
            .border_1()
            .border_color(card_border)
            .child(
                div()
                    .text_size(px(pt_to_px(10.0, context.zoom)))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(title_color)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(pt_to_px(9.5, context.zoom)))
                    .text_color(body_color)
                    .child(body),
            )
            .into_any_element();

        Some(CustomRenderOutput::Element(element))
    }

    fn plain_text_block(&self, data: &serde_json::Value) -> Option<String> {
        let title = data.get("title").and_then(|v| v.as_str()).unwrap_or("Callout");
        let body = data.get("body").and_then(|v| v.as_str()).unwrap_or("");
        Some(format!("{title}: {body}"))
    }
}

struct StatusBadgeExtension;

impl NodeExtension for StatusBadgeExtension {
    fn measure_inline(
        &self,
        data: &serde_json::Value,
        zoom: f32,
        _window: &Window,
    ) -> InlineBoxMetrics {
        let label = data
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or("STATUS");
        let font_px = pt_to_px(9.0, zoom);
        InlineBoxMetrics {
            width: (label.chars().count().max(3) as f32) * font_px * 0.62 + pt_to_px(14.0, zoom),
            height: pt_to_px(16.0, zoom),
            valign: InlineVAlign::Middle,
        }
    }

    fn render_inline(
        &self,
        context: CustomInlineRenderContext<'_>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let label = context
            .data
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or("STATUS")
            .to_owned();
        let is_dark = context.theme.is_dark();
        let badge_bg = if is_dark { rgb(0x1b3a28) } else { rgb(0xdcfce7) };
        let badge_border = if is_dark { rgb(0x3fb950) } else { rgb(0x86efac) };
        let badge_text = if is_dark { rgb(0x7ee787) } else { rgb(0x166534) };

        let element = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(pt_to_px(8.0, context.zoom)))
            .bg(badge_bg)
            .border_1()
            .border_color(badge_border)
            .text_size(px(pt_to_px(8.5, context.zoom)))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(badge_text)
            .child(label)
            .into_any_element();

        Some(CustomRenderOutput::Element(element))
    }

    fn plain_text_inline(&self, data: &serde_json::Value) -> Option<String> {
        data.get("label")
            .and_then(|value| value.as_str())
            .map(|label| format!("[{label}]"))
    }
}

struct DocsViewerApp {
    document_view: Entity<RichDocumentView>,
    _event_subscription: Subscription,
    last_paper_spec: PaperSpec,
    dark_mode: bool,
    font_menu_open: bool,
    margin_menu_open: bool,
    available_fonts: Vec<SharedString>,
    status_message: SharedString,
}

impl DocsViewerApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let document = build_sample_document();
        let document_view = cx.new(|cx| {
            RichDocumentView::new(document, cx)
                .with_decoration_provider(RustSyntaxDecorationProvider)
                .with_node_extension("callout", CalloutBlockExtension)
                .with_node_extension("status", StatusBadgeExtension)
        });
        let event_subscription = cx.subscribe(
            &document_view,
            |this, _view, event: &RichDocumentEvent, cx| match event {
                RichDocumentEvent::LinkActivated { href } => {
                    this.status_message = format!("Link: {href}").into();
                    cx.notify();
                }
                RichDocumentEvent::DocumentMutated { change_set } => {
                    this.status_message =
                        format!("Mutated {} block(s)", change_set.dirty_blocks.len()).into();
                    cx.notify();
                }
                RichDocumentEvent::SelectionChanged { .. } => {
                    cx.notify();
                }
                RichDocumentEvent::CommandExecuted { .. } => {}
            },
        );
        Self {
            document_view,
            _event_subscription: event_subscription,
            last_paper_spec: PaperSpec::LETTER,
            dark_mode: false,
            font_menu_open: false,
            margin_menu_open: false,
            available_fonts: Vec::new(),
            status_message: SharedString::from("Viewer"),
        }
    }

    fn edges_approx_eq(left: Edges, right: Edges) -> bool {
        (left.top - right.top).abs() < 0.5
            && (left.right - right.right).abs() < 0.5
            && (left.bottom - right.bottom).abs() < 0.5
            && (left.left - right.left).abs() < 0.5
    }

    fn margin_preset_label(margins: Edges) -> &'static str {
        if Self::edges_approx_eq(margins, Edges::MARGINS_NARROW) {
            "Narrow"
        } else if Self::edges_approx_eq(margins, Edges::MARGINS_MODERATE) {
            "Moderate"
        } else if Self::edges_approx_eq(margins, Edges::MARGINS_NORMAL) {
            "Normal"
        } else if Self::edges_approx_eq(margins, Edges::MARGINS_WIDE) {
            "Wide"
        } else {
            "Custom"
        }
    }

    fn apply_margins(&mut self, next_margins: Edges, cx: &mut Context<Self>) {
        let clamped = next_margins.clamped_non_negative();
        if matches!(
            self.document_view.read(cx).document.config.horizontal_layout,
            HorizontalLayout::Paper(_)
        ) {
            self.last_paper_spec.margins_pt = clamped;
        }
        self.document_view.update(cx, |view, cx| {
            view.dispatch_command(RichCommand::SetMargins(clamped), cx);
        });
        cx.notify();
    }

    fn ensure_fonts_loaded(&mut self, window: &Window) {
        if !self.available_fonts.is_empty() {
            return;
        }
        let mut names: Vec<String> = window
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names.dedup();
        self.available_fonts = names.into_iter().map(SharedString::from).collect();
    }

    fn toolbar_button(
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        dark_mode: bool,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (background, text_color) = match (dark_mode, active) {
            (false, true) => (rgb(0xd3e3fd), rgb(0x0b57d0)),
            (false, false) => (rgb(0xf1f3f4), rgb(0x1f2328)),
            (true, true) => (rgb(0x1f3a5f), rgb(0x79c0ff)),
            (true, false) => (rgb(0x21262d), rgb(0xe6edf3)),
        };
        div()
            .id(id)
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(background)
            .text_color(text_color)
            .text_size(px(12.0))
            .cursor_pointer()
            .child(label.into())
    }
}

impl Render for DocsViewerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_fonts_loaded(window);
        let (zoom, horizontal_layout, active_margins, debug_active, policy) = {
            let view = self.document_view.read(cx);
            (
                view.zoom,
                view.document.config.horizontal_layout,
                view.document.config.active_margins_pt(),
                view.debug_overlay,
                view.policy,
            )
        };
        if !policy.editable {
            self.font_menu_open = false;
        }
        let dark = self.dark_mode;
        let is_fill = matches!(horizontal_layout, HorizontalLayout::Fill);
        let active_paper = match horizontal_layout {
            HorizontalLayout::Paper(spec) => Some(spec),
            HorizontalLayout::Fill => None,
        };
        let margins_button_label = format!("Margins: {} ▾", Self::margin_preset_label(active_margins));
        let zoom_label = format!("{}%", (zoom * 100.0).round() as i32);
        let toolbar_bg = if dark { rgb(0x161b22) } else { rgb(0xf8f9fa) };
        let toolbar_border = if dark { rgb(0x30363d) } else { rgb(0xd0d7de) };
        let title_color = if dark { rgb(0xe6edf3) } else { rgb(0x1f2328) };
        let muted_color = if dark { rgb(0x8b949e) } else { rgb(0x59636e) };
        let root_bg = if dark { rgb(0x0d1117) } else { rgb(0xffffff) };

        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(14.0))
            .py(px(8.0))
            .bg(toolbar_bg)
            .border_b_1()
            .border_color(toolbar_border)
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(title_color)
                    .mr(px(6.0))
                    .child("gpui-rich"),
            )
            .child(
                Self::toolbar_button("mode-paper", "Paper", !is_fill, dark).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        let spec = this.last_paper_spec;
                        this.document_view.update(cx, |view, cx| {
                            view.dispatch_command(
                                RichCommand::SetHorizontalLayout(HorizontalLayout::Paper(spec)),
                                cx,
                            );
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("mode-fill", "Fill", is_fill, dark).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.dispatch_command(
                                RichCommand::SetHorizontalLayout(HorizontalLayout::Fill),
                                cx,
                            );
                        });
                    }),
                ),
            );

        if let Some(paper_spec) = active_paper {
            let current_paper_margins = paper_spec.margins_pt;
            toolbar = toolbar
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button(
                        "paper-narrow",
                        "Narrow",
                        (paper_spec.width_pt - PaperSpec::NARROW.width_pt).abs() < 0.5,
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            let next_spec = PaperSpec::NARROW.with_margins(current_paper_margins);
                            this.last_paper_spec = next_spec;
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetHorizontalLayout(HorizontalLayout::Paper(
                                        next_spec,
                                    )),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "paper-letter",
                        "Letter",
                        (paper_spec.width_pt - PaperSpec::LETTER.width_pt).abs() < 0.5,
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            let next_spec = PaperSpec::LETTER.with_margins(current_paper_margins);
                            this.last_paper_spec = next_spec;
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetHorizontalLayout(HorizontalLayout::Paper(
                                        next_spec,
                                    )),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "paper-wide",
                        "Wide",
                        (paper_spec.width_pt - PaperSpec::WIDE.width_pt).abs() < 0.5,
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            let next_spec = PaperSpec::WIDE.with_margins(current_paper_margins);
                            this.last_paper_spec = next_spec;
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetHorizontalLayout(HorizontalLayout::Paper(
                                        next_spec,
                                    )),
                                    cx,
                                );
                            });
                        }),
                    ),
                );
        }

        toolbar = toolbar
            .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
            .child(
                Self::toolbar_button(
                    "margins-picker",
                    margins_button_label,
                    self.margin_menu_open,
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.margin_menu_open = !this.margin_menu_open;
                        if this.margin_menu_open {
                            this.font_menu_open = false;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            );

        toolbar = toolbar
            .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
            .child(
                Self::toolbar_button("zoom-out", "-", false, dark).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.dispatch_command(RichCommand::AdjustZoom(-0.1), cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("zoom-reset", zoom_label, false, dark).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.dispatch_command(RichCommand::ResetZoom, cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("zoom-in", "+", false, dark).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.dispatch_command(RichCommand::AdjustZoom(0.1), cx);
                        });
                    }),
                ),
            )
            .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
            .child(
                Self::toolbar_button(
                    "policy-viewer",
                    "Viewer",
                    policy == InteractionPolicy::viewer(),
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.status_message = SharedString::from("Viewer");
                        this.document_view.update(cx, |view, cx| {
                            view.set_policy(InteractionPolicy::viewer(), cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button(
                    "policy-editable",
                    "Editable",
                    policy == InteractionPolicy::editable(),
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.status_message = SharedString::from("Editable");
                        this.document_view.update(cx, |view, cx| {
                            view.set_policy(InteractionPolicy::editable(), cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button(
                    "policy-preview",
                    "Preview",
                    policy == InteractionPolicy::static_preview(),
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.status_message = SharedString::from("Static Preview");
                        this.document_view.update(cx, |view, cx| {
                            view.set_policy(InteractionPolicy::static_preview(), cx);
                        });
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(muted_color)
                    .mr(px(6.0))
                    .child(self.status_message.clone()),
            )
            .child(
                Self::toolbar_button("toggle-theme", if dark { "Light" } else { "Dark" }, dark, dark)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.dark_mode = !this.dark_mode;
                            let next_theme = if this.dark_mode {
                                RichTheme::dark()
                            } else {
                                RichTheme::light()
                            };
                            this.document_view.update(cx, |view, cx| {
                                view.set_theme(next_theme, cx);
                            });
                            cx.notify();
                        }),
                    ),
            )
            .child(
                Self::toolbar_button("toggle-debug", "Debug Baselines", debug_active, dark)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(RichCommand::ToggleDebugOverlay, cx);
                            });
                        }),
                    ),
            );

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(root_bg)
            .child(toolbar);

        let mut active_font_selection: Option<SelectionValue<rich_model::SharedStr>> = None;

        if policy.editable {
            let (
                bold_active,
                italic_active,
                underline_active,
                strike_active,
                code_active,
                font_family,
                font_size_pt,
                heading_level,
                list_kind,
                text_align,
            ) = {
                let view = self.document_view.read(cx);
                (
                    view.is_mark_active(MarkKind::Bold),
                    view.is_mark_active(MarkKind::Italic),
                    view.is_mark_active(MarkKind::Underline),
                    view.is_mark_active(MarkKind::Strikethrough),
                    view.is_mark_active(MarkKind::InlineCode),
                    view.selected_font_family(),
                    view.selected_font_size_pt(),
                    view.selected_heading_level(),
                    view.active_list_kind(),
                    view.selected_text_align(),
                )
            };

            let font_label = match &font_family {
                Some(SelectionValue::Uniform(name)) => {
                    let display = match name.as_ref() {
                        ".SystemUIFont" | "sans-serif" => "Default (Sans)",
                        "serif" => "Serif",
                        "monospace" => "Monospace",
                        other => other,
                    };
                    format!("{display} ▾")
                }
                Some(SelectionValue::Mixed) => "Mixed ▾".to_owned(),
                None => "Font ▾".to_owned(),
            };
            active_font_selection = font_family;

            let size_label = match font_size_pt {
                Some(SelectionValue::Uniform(pt)) => format!("{:.0}pt", pt),
                Some(SelectionValue::Mixed) => "Mixed".to_owned(),
                None => "--".to_owned(),
            };
            let uniform_heading = heading_level.and_then(SelectionValue::uniform);
            let uniform_align = text_align.and_then(SelectionValue::uniform);

            let format_bar = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .px(px(14.0))
                .py(px(6.0))
                .bg(toolbar_bg)
                .border_b_1()
                .border_color(toolbar_border)
                .child(
                    Self::toolbar_button("fmt-font-picker", font_label, self.font_menu_open, dark)
                        .min_w(px(120.0))
                        .max_w(px(180.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.font_menu_open = !this.font_menu_open;
                                if this.font_menu_open {
                                    this.margin_menu_open = false;
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button("fmt-size-dec", "A-", false, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(RichCommand::AdjustFontSize(-1.0), cx);
                            });
                        }),
                    ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(muted_color)
                        .px(px(4.0))
                        .child(size_label),
                )
                .child(
                    Self::toolbar_button("fmt-size-inc", "A+", false, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(RichCommand::AdjustFontSize(1.0), cx);
                            });
                        }),
                    ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button("fmt-bold", "B", bold_active, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(RichCommand::ToggleMark(MarkKind::Bold), cx);
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button("fmt-italic", "I", italic_active, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleMark(MarkKind::Italic),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button("fmt-underline", "U", underline_active, dark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(
                                        RichCommand::ToggleMark(MarkKind::Underline),
                                        cx,
                                    );
                                });
                            }),
                        ),
                )
                .child(
                    Self::toolbar_button("fmt-strike", "S", strike_active, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleMark(MarkKind::Strikethrough),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button("fmt-code", "</>", code_active, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleMark(MarkKind::InlineCode),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button("fmt-p", "P", uniform_heading == Some(None), dark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(RichCommand::SetHeading(None), cx);
                                });
                            }),
                        ),
                )
                .child(
                    Self::toolbar_button("fmt-h1", "H1", uniform_heading == Some(Some(1)), dark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(RichCommand::SetHeading(Some(1)), cx);
                                });
                            }),
                        ),
                )
                .child(
                    Self::toolbar_button("fmt-h2", "H2", uniform_heading == Some(Some(2)), dark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(RichCommand::SetHeading(Some(2)), cx);
                                });
                            }),
                        ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button(
                        "fmt-bullet",
                        "Bullet",
                        list_kind == Some(ListKind::Bullet),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleList(ListKind::Bullet),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "fmt-ordered",
                        "1.",
                        matches!(list_kind, Some(ListKind::Ordered(_))),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleList(ListKind::Ordered(
                                        NumberStyle::Decimal,
                                    )),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "fmt-check",
                        "Task",
                        list_kind == Some(ListKind::Checklist),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::ToggleList(ListKind::Checklist),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button(
                        "fmt-align-l",
                        "Left",
                        uniform_align == Some(TextAlign::Left),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetTextAlign(TextAlign::Left),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "fmt-align-c",
                        "Center",
                        uniform_align == Some(TextAlign::Center),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetTextAlign(TextAlign::Center),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button(
                        "fmt-align-r",
                        "Right",
                        uniform_align == Some(TextAlign::Right),
                        dark,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::SetTextAlign(TextAlign::Right),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                .child(
                    Self::toolbar_button("fmt-clear", "Clear", false, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(RichCommand::ClearFormatting, cx);
                            });
                        }),
                    ),
                );
            root = root.child(format_bar);
        }

        let mut root = root.child(div().flex_1().size_full().child(self.document_view.clone()));

        if self.font_menu_open && policy.editable {
            let menu_bg = if dark { rgb(0x161b22) } else { rgb(0xffffff) };
            let hover_bg = if dark { rgb(0x21262d) } else { rgb(0xf1f3f4) };
            let selected_bg = if dark { rgb(0x1f3a5f) } else { rgb(0xd3e3fd) };
            let selected_text = if dark { rgb(0x79c0ff) } else { rgb(0x0b57d0) };
            let current_uniform = active_font_selection
                .as_ref()
                .and_then(|value| match value {
                    SelectionValue::Uniform(name) => Some(name.as_ref()),
                    SelectionValue::Mixed => None,
                });

            let mut list = div()
                .id("font-picker-list")
                .flex()
                .flex_col()
                .h(px(320.0))
                .overflow_y_scroll()
                .p(px(4.0))
                .gap(px(2.0));

            let generic_entries: [(&str, &str); 3] = [
                ("Default (Sans)", ".SystemUIFont"),
                ("Serif", "serif"),
                ("Monospace", "monospace"),
            ];
            for (index, (label, value)) in generic_entries.into_iter().enumerate() {
                let is_selected = current_uniform.is_some_and(|active| {
                    active.eq_ignore_ascii_case(value)
                        || (value == ".SystemUIFont" && active.eq_ignore_ascii_case("sans-serif"))
                });
                let font_value = value.to_owned();
                let preview_family = rich_gpui::resolve_font_family(value, window);
                list = list.child(
                    div()
                        .id(("font-generic", index))
                        .flex_shrink_0()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(if is_selected { selected_bg } else { menu_bg })
                        .hover(move |style| style.bg(if is_selected { selected_bg } else { hover_bg }))
                        .text_color(if is_selected { selected_text } else { title_color })
                        .text_size(px(12.0))
                        .font_family(preview_family)
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                let focus_handle = this.document_view.read(cx).focus_handle.clone();
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(
                                        RichCommand::SetInlineStyle(
                                            InlineStyle::default()
                                                .with_font_family(font_value.clone()),
                                        ),
                                        cx,
                                    );
                                });
                                this.font_menu_open = false;
                                window.focus(&focus_handle, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(label),
                );
            }

            list = list.child(
                div()
                    .flex_shrink_0()
                    .h(px(1.0))
                    .w_full()
                    .bg(toolbar_border)
                    .my(px(3.0)),
            );

            for (index, font_name) in self.available_fonts.iter().enumerate() {
                let is_selected = current_uniform
                    .is_some_and(|active| active.eq_ignore_ascii_case(font_name.as_ref()));
                let font_value = font_name.to_string();
                let preview_family = font_name.clone();
                let display_label = font_name.clone();
                list = list.child(
                    div()
                        .id(("font-installed", index))
                        .flex_shrink_0()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(if is_selected { selected_bg } else { menu_bg })
                        .hover(move |style| style.bg(if is_selected { selected_bg } else { hover_bg }))
                        .text_color(if is_selected { selected_text } else { title_color })
                        .text_size(px(12.0))
                        .font_family(preview_family)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                let focus_handle = this.document_view.read(cx).focus_handle.clone();
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(
                                        RichCommand::SetInlineStyle(
                                            InlineStyle::default()
                                                .with_font_family(font_value.clone()),
                                        ),
                                        cx,
                                    );
                                });
                                this.font_menu_open = false;
                                window.focus(&focus_handle, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(display_label),
                );
            }

            let backdrop = div()
                .id("font-picker-backdrop")
                .occlude()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_scroll(|_event, _window, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.font_menu_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .occlude()
                        .absolute()
                        .top(px(82.0))
                        .left(px(14.0))
                        .w(px(240.0))
                        .rounded(px(8.0))
                        .bg(menu_bg)
                        .border_1()
                        .border_color(toolbar_border)
                        .shadow_md()
                        .on_scroll(|_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .child(list),
                );

            root = root.child(backdrop);
        }

        if self.margin_menu_open {
            let menu_bg = if dark { rgb(0x161b22) } else { rgb(0xffffff) };
            let hover_bg = if dark { rgb(0x21262d) } else { rgb(0xf1f3f4) };
            let selected_bg = if dark { rgb(0x1f3a5f) } else { rgb(0xd3e3fd) };
            let selected_text = if dark { rgb(0x79c0ff) } else { rgb(0x0b57d0) };
            let step_button_bg = if dark { rgb(0x21262d) } else { rgb(0xf1f3f4) };

            let mut panel_content = div()
                .flex()
                .flex_col()
                .p(px(8.0))
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(muted_color)
                        .px(px(6.0))
                        .pb(px(2.0))
                        .child("Margin Presets"),
                );

            let presets: [(&str, &str, Edges); 4] = [
                ("Narrow", "0.5 in (36pt)", Edges::MARGINS_NARROW),
                ("Moderate", "0.75 in (54pt)", Edges::MARGINS_MODERATE),
                ("Normal", "1.0 in (72pt)", Edges::MARGINS_NORMAL),
                ("Wide", "1.5 in / 1.0 in", Edges::MARGINS_WIDE),
            ];
            for (index, (name, detail, preset_edges)) in presets.into_iter().enumerate() {
                let is_selected = Self::edges_approx_eq(active_margins, preset_edges);
                panel_content = panel_content.child(
                    div()
                        .id(("margin-preset", index))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(if is_selected { selected_bg } else { menu_bg })
                        .hover(move |style| style.bg(if is_selected { selected_bg } else { hover_bg }))
                        .text_color(if is_selected { selected_text } else { title_color })
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.apply_margins(preset_edges, cx);
                                this.margin_menu_open = false;
                                cx.stop_propagation();
                            }),
                        )
                        .child(name)
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(if is_selected { selected_text } else { muted_color })
                                .child(detail),
                        ),
                );
            }

            panel_content = panel_content
                .child(div().h(px(1.0)).w_full().bg(toolbar_border).my(px(4.0)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(muted_color)
                        .px(px(6.0))
                        .pb(px(2.0))
                        .child("Custom Edges (6pt step)"),
                );

            let edge_rows: [(&str, f32, Edges, Edges); 4] = [
                (
                    "Top",
                    active_margins.top,
                    Edges::new(
                        active_margins.top - 6.0,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top + 6.0,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                ),
                (
                    "Bottom",
                    active_margins.bottom,
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom - 6.0,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom + 6.0,
                        active_margins.left,
                    ),
                ),
                (
                    "Left",
                    active_margins.left,
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left - 6.0,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left + 6.0,
                    ),
                ),
                (
                    "Right",
                    active_margins.right,
                    Edges::new(
                        active_margins.top,
                        active_margins.right - 6.0,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right + 6.0,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                ),
            ];

            for (index, (label, current_pt, decreased, increased)) in
                edge_rows.into_iter().enumerate()
            {
                panel_content = panel_content.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(px(6.0))
                        .py(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(title_color)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id(("margin-edge-dec", index))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(step_button_bg)
                                        .text_color(title_color)
                                        .text_size(px(12.0))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _event, _window, cx| {
                                                this.apply_margins(decreased, cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child("-"),
                                )
                                .child(
                                    div()
                                        .w(px(44.0))
                                        .text_size(px(11.0))
                                        .text_color(muted_color)
                                        .child(format!("{:.0}pt", current_pt)),
                                )
                                .child(
                                    div()
                                        .id(("margin-edge-inc", index))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(step_button_bg)
                                        .text_color(title_color)
                                        .text_size(px(12.0))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _event, _window, cx| {
                                                this.apply_margins(increased, cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child("+"),
                                ),
                        ),
                );
            }

            let menu_left = if active_paper.is_some() { 360.0 } else { 195.0 };
            let margin_backdrop = div()
                .id("margin-picker-backdrop")
                .occlude()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_scroll(|_event, _window, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.margin_menu_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .occlude()
                        .absolute()
                        .top(px(42.0))
                        .left(px(menu_left))
                        .w(px(250.0))
                        .rounded(px(8.0))
                        .bg(menu_bg)
                        .border_1()
                        .border_color(toolbar_border)
                        .shadow_md()
                        .on_scroll(|_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .child(panel_content),
                );

            root = root.child(margin_backdrop);
        }

        root
    }
}

fn build_sample_document() -> Document {
    let mut document = Document::new();

    document.add_paragraph(
        InlineContent::plain("GPUI Rich Text Architecture"),
        BlockStyle::named(StyleId::TITLE),
    );
    document.add_paragraph(
        InlineContent::plain(
            "Non-paginated continuous document renderer designed for a future rich text editor.",
        ),
        BlockStyle::named(StyleId::SUBTITLE),
    );

    // Section 1: Mixed typography, baseline alignment, inline objects, and decorations.
    document.add_heading(1, InlineContent::plain("1. Mixed-Baseline Typography & Inline Objects"));

    let mut mixed = InlineContent::empty();
    mixed.push_text("A single visual line aligns ", InlineStyle::default());
    mixed.push_text("11pt bold", InlineStyle::bold());
    mixed.push_text(", ", InlineStyle::default());
    mixed.push_text("italic", InlineStyle::italic());
    mixed.push_text(", and ", InlineStyle::default());
    mixed.push_text(
        "serif",
        InlineStyle::default().with_font_family("serif"),
    );
    mixed.push_text(" next to ", InlineStyle::default());
    mixed.push_text(
        "22pt display text",
        InlineStyle::default()
            .with_font_size(22.0)
            .with_weight(FontWeight::SEMIBOLD)
            .with_color(RgbaColor::rgb(0x0b, 0x57, 0xd0)),
    );
    mixed.push_text(", superscripts like E = mc", InlineStyle::default());
    mixed.push_text(
        "2",
        InlineStyle::default().with_baseline_shift(BaselineShift::Superscript),
    );
    mixed.push_text(", and subscripts like H", InlineStyle::default());
    mixed.push_text(
        "2",
        InlineStyle::default().with_baseline_shift(BaselineShift::Subscript),
    );
    mixed.push_text("O on one shared typographic baseline.", InlineStyle::default());
    document.add_paragraph(mixed, BlockStyle::default());

    let mut chips = InlineContent::empty();
    chips.push_text("Inline atomic embeds use U+FFFC sentinels: ", InlineStyle::default());
    chips.push_object(
        InlineObject::SmartChip {
            kind: ChipKind::Person,
            label: "@Alice Chen".into(),
            accent: None,
        },
        InlineStyle::default(),
    );
    chips.push_text(" scheduled ", InlineStyle::default());
    chips.push_object(
        InlineObject::SmartChip {
            kind: ChipKind::Date,
            label: "Nov 22, 2026".into(),
            accent: None,
        },
        InlineStyle::default(),
    );
    chips.push_text(" for ", InlineStyle::default());
    chips.push_object(
        InlineObject::SmartChip {
            kind: ChipKind::Document,
            label: "Editor Spec".into(),
            accent: None,
        },
        InlineStyle::default(),
    );
    chips.push_text(" with inline ", InlineStyle::default());
    chips.push_text("code_spans()", InlineStyle::code());
    chips.push_text(", ", InlineStyle::default());
    chips.push_text(
        "highlighted passages",
        InlineStyle::default().with_background(RgbaColor::rgba(0xff, 0xd4, 0x3b, 0x6b)),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text(
        "wavy spellcheck",
        InlineStyle::default().with_underline(UnderlineKind::Wavy(RgbaColor::rgb(0xcf, 0x22, 0x2e))),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text("strikethrough", InlineStyle::default().with_strikethrough(true));
    chips.push_text(", ", InlineStyle::default());
    chips.push_text("clickable links", InlineStyle::default().with_link("https://gpui.rs"));
    chips.push_text(", and ", InlineStyle::default());
    chips.push_text(
        "internal doc URIs",
        InlineStyle::default().with_link("doc://architecture/phase-2"),
    );
    chips.push_object(InlineObject::FootnoteRef { index: 1 }, InlineStyle::default());
    chips.push_text(".", InlineStyle::default());
    document.add_paragraph(chips, BlockStyle::default());

    let mut inline_img = InlineContent::empty();
    inline_img.push_text("Inline image placeholder anchored to baseline: ", InlineStyle::default());
    inline_img.push_object(
        InlineObject::Image {
            asset_id: "diagram-1".into(),
            alt: "Diagram 160x36".into(),
            width_pt: 120.0,
            height_pt: 28.0,
            valign: InlineVAlign::Middle,
        },
        InlineStyle::default(),
    );
    inline_img.push_text(" flowing cleanly with surrounding text.", InlineStyle::default());
    document.add_paragraph(inline_img, BlockStyle::default());

    // Section 2: Alignment, lists, blockquotes, and code blocks.
    document.add_heading(1, InlineContent::plain("2. Alignment, Lists & Code Blocks"));

    document.add_paragraph(
        InlineContent::plain(
            "Justified paragraphs distribute residual whitespace evenly across word gaps on every soft-wrapped line while keeping hit-testing and caret positions synchronized with the painted glyphs. Try dragging across words or toggling Debug Baselines in the toolbar above.",
        ),
        BlockStyle::default().with_align(TextAlign::Justify),
    );
    document.add_paragraph(
        InlineContent::plain("Centered callout line"),
        BlockStyle::default().with_align(TextAlign::Center),
    );
    document.add_paragraph(
        InlineContent::plain("Right-aligned attribution"),
        BlockStyle::default().with_align(TextAlign::Right),
    );

    let bullet_list = ListId(1);
    document.add_list_item(
        bullet_list,
        ListKind::Bullet,
        0,
        None,
        InlineContent::plain("Fenwick tree HeightIndex provides O(log N) viewport queries."),
    );
    document.add_list_item(
        bullet_list,
        ListKind::Bullet,
        1,
        None,
        InlineContent::plain("Click the triangle chevron next to any heading to collapse or expand its section."),
    );

    let ordered_list = ListId(2);
    document.add_list_item(
        ordered_list,
        ListKind::Ordered(NumberStyle::Decimal),
        0,
        None,
        InlineContent::plain("Pure-Rust rich-model crate with Undo/Redo transactions."),
    );
    document.add_list_item(
        ordered_list,
        ListKind::Ordered(NumberStyle::Decimal),
        0,
        None,
        InlineContent::plain("Pure-Rust rich-layout crate tested headlessly via MockShaper."),
    );
    document.add_list_item(
        ListId(3),
        ListKind::Checklist,
        0,
        Some(true),
        InlineContent::plain("Phase 1: Non-paginated 6-layer GPUI renderer with selection & copy."),
    );
    document.add_list_item(
        ListId(3),
        ListKind::Checklist,
        0,
        Some(true),
        InlineContent::plain("Phase 2: Rich text editing, formatting, rich HTML clipboard, decorations, and custom extensions."),
    );

    let mut ext_intro = InlineContent::empty();
    ext_intro.push_text("Custom inline extensions render hosted GPUI widgets inline ", InlineStyle::default());
    ext_intro.push_object(
        InlineObject::Custom {
            kind: "status".into(),
            data: serde_json::json!({ "label": "PHASE 2 COMPLETE" }),
        },
        InlineStyle::default(),
    );
    ext_intro.push_text(" alongside custom block nodes:", InlineStyle::default());
    document.add_paragraph(ext_intro, BlockStyle::default());

    document.add_custom_block(
        "callout",
        serde_json::json!({
            "title": "Hosted GPUI Extension Block",
            "body": "Measured via TextShaper::measure_custom_block and rendered via NodeExtension + AnyElement::prepaint_as_root."
        }),
    );

    let quote_child_id = document.alloc_block_id();
    let quote_child = BlockNode::new(
        quote_child_id,
        BlockKind::Paragraph(InlineContent::plain(
            "Click and drag across paragraphs to select text, double-click a word, or press Ctrl+C / Cmd+C to copy.",
        )),
        BlockStyle::default(),
    );
    document.add_blockquote(vec![quote_child]);

    document.add_code_block(
        Some("rust".into()),
        InlineContent::plain(
            "pub trait TextShaper {\n    type Backend: GlyphBackend;\n    fn shape_segment(&mut self, text: &str, font_size_px: f32, runs: &[ShaperRunSpec]) -> ShapedSegment<Self::Backend>;\n}",
        ),
    );

    document.add_horizontal_rule();

    // Section 3: Wide breakout recursive table.
    document.add_heading(1, InlineContent::plain("3. Wide Breakout Recursive Table"));

    let table_id = document.alloc_block_id();
    let make_cell = |doc: &mut Document, text: &str, bold: bool| -> TableCell {
        let cell_id = doc.alloc_block_id();
        let para_id = doc.alloc_block_id();
        let mut content = InlineContent::empty();
        content.push_text(
            text,
            if bold {
                InlineStyle::bold()
            } else {
                InlineStyle::default()
            },
        );
        let para = BlockNode::new(para_id, BlockKind::Paragraph(content), BlockStyle::default());
        TableCell::simple(cell_id, vec![para])
    };

    let rows = vec![
        TableRow {
            is_header: true,
            cells: vec![
                make_cell(&mut document, "Crate", true),
                make_cell(&mut document, "Responsibility", true),
                make_cell(&mut document, "GPUI Dependency", true),
            ],
        },
        TableRow {
            is_header: false,
            cells: vec![
                make_cell(&mut document, "rich-model", false),
                make_cell(
                    &mut document,
                    "Block tree, normalized inline spans, U+FFFC embeds, DocPos, Transactions",
                    false,
                ),
                make_cell(&mut document, "None (pure Rust)", false),
            ],
        },
        TableRow {
            is_header: false,
            cells: vec![
                make_cell(&mut document, "rich-layout", false),
                make_cell(
                    &mut document,
                    "Mixed-size line composer, UAX #14 wrapping, Fenwick HeightIndex, hit-testing",
                    false,
                ),
                make_cell(&mut document, "None (TextShaper trait)", false),
            ],
        },
        TableRow {
            is_header: false,
            cells: vec![
                make_cell(&mut document, "rich-gpui", false),
                make_cell(
                    &mut document,
                    "GpuiShaper, 6-layer RichDocumentElement, zoom, scroll anchoring, selection",
                    false,
                ),
                make_cell(&mut document, "gpui-kit 0.10", false),
            ],
        },
    ];

    let table_block = TableBlock {
        columns: vec![
            ColumnSpec::Ratio(1.0),
            ColumnSpec::Ratio(2.6),
            ColumnSpec::Ratio(1.2),
        ],
        rows,
        border_style: TableBorderStyle::default(),
    };
    document.push_block(
        BlockNode::new(table_id, BlockKind::Table(table_block), BlockStyle::default())
            .with_width_mode(BlockWidth::Wide),
    );

    document
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        register_default_keybindings(cx);
        let bounds = Bounds::centered(None, size(px(1080.0), px(780.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        };
        if let Err(error) =
            gpui_kit::open_window(options, cx, |_window, cx| cx.new(DocsViewerApp::new))
        {
            eprintln!("docs_viewer: failed to open window: {error}");
        }
    });
}
