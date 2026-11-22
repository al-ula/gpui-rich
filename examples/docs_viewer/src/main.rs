//! Demo GPUI application showcasing the non-paginated `gpui-rich` renderer.

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, Styled, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use rich_gpui::RichDocumentView;
use rich_model::{
    BaselineShift, BlockKind, BlockNode, BlockStyle, BlockWidth, ChipKind, ColumnSpec, Document,
    FontWeight, HorizontalLayout, InlineContent, InlineObject, InlineStyle, InlineVAlign, ListId,
    ListKind, NumberStyle, PaperSpec, RgbaColor, StyleId, TableBlock, TableBorderStyle, TableCell,
    TableRow, TextAlign, UnderlineKind,
};

struct DocsViewerApp {
    document_view: Entity<RichDocumentView>,
    last_paper_spec: PaperSpec,
}

impl DocsViewerApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let document = build_sample_document();
        let document_view = cx.new(|cx| RichDocumentView::new(document, cx));
        Self {
            document_view,
            last_paper_spec: PaperSpec::LETTER,
        }
    }

    fn toolbar_button(
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let background = if active { rgb(0xd3e3fd) } else { rgb(0xf1f3f4) };
        let text_color = if active { rgb(0x0b57d0) } else { rgb(0x1f2328) };
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (zoom, horizontal_layout, debug_active) = {
            let view = self.document_view.read(cx);
            (
                view.zoom,
                view.document.config.horizontal_layout,
                view.debug_overlay,
            )
        };
        let is_fill = matches!(horizontal_layout, HorizontalLayout::Fill);
        let active_paper = match horizontal_layout {
            HorizontalLayout::Paper(spec) => Some(spec),
            HorizontalLayout::Fill => None,
        };
        let zoom_label = format!("{}%", (zoom * 100.0).round() as i32);

        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(8.0))
            .bg(rgb(0xf8f9fa))
            .border_b_1()
            .border_color(rgb(0xd0d7de))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x1f2328))
                    .mr(px(8.0))
                    .child("gpui-rich"),
            )
            .child(
                Self::toolbar_button("mode-paper", "Paper", !is_fill).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        let spec = this.last_paper_spec;
                        this.document_view.update(cx, |view, cx| {
                            view.set_horizontal_layout(HorizontalLayout::Paper(spec), cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("mode-fill", "Fill", is_fill).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.set_horizontal_layout(HorizontalLayout::Fill, cx);
                        });
                    }),
                ),
            );

        if let Some(paper_spec) = active_paper {
            toolbar = toolbar
                .child(div().w(px(1.0)).h(px(16.0)).bg(rgb(0xd0d7de)).mx(px(4.0)))
                .child(
                    Self::toolbar_button(
                        "paper-narrow",
                        "Narrow",
                        paper_spec == PaperSpec::NARROW,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.last_paper_spec = PaperSpec::NARROW;
                            this.document_view.update(cx, |view, cx| {
                                view.set_horizontal_layout(
                                    HorizontalLayout::Paper(PaperSpec::NARROW),
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
                        paper_spec == PaperSpec::LETTER,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.last_paper_spec = PaperSpec::LETTER;
                            this.document_view.update(cx, |view, cx| {
                                view.set_horizontal_layout(
                                    HorizontalLayout::Paper(PaperSpec::LETTER),
                                    cx,
                                );
                            });
                        }),
                    ),
                )
                .child(
                    Self::toolbar_button("paper-wide", "Wide", paper_spec == PaperSpec::WIDE)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.last_paper_spec = PaperSpec::WIDE;
                                this.document_view.update(cx, |view, cx| {
                                    view.set_horizontal_layout(
                                        HorizontalLayout::Paper(PaperSpec::WIDE),
                                        cx,
                                    );
                                });
                            }),
                        ),
                );
        }

        toolbar = toolbar
            .child(div().w(px(1.0)).h(px(16.0)).bg(rgb(0xd0d7de)).mx(px(4.0)))
            .child(
                Self::toolbar_button("zoom-out", "-", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.set_zoom(view.zoom - 0.1, cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("zoom-reset", zoom_label, false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.set_zoom(1.0, cx);
                        });
                    }),
                ),
            )
            .child(
                Self::toolbar_button("zoom-in", "+", false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            view.set_zoom(view.zoom + 0.1, cx);
                        });
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                Self::toolbar_button("toggle-debug", "Debug Baselines", debug_active)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.toggle_debug_overlay(cx);
                            });
                        }),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xffffff))
            .child(toolbar)
            .child(div().flex_1().size_full().child(self.document_view.clone()))
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
    mixed.push_text("11pt body text", InlineStyle::bold());
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
        InlineStyle::default().with_background(RgbaColor::rgb(0xff, 0xf3, 0xb0)),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text(
        "wavy spellcheck",
        InlineStyle::default().with_underline(UnderlineKind::Wavy(RgbaColor::rgb(0xcf, 0x22, 0x2e))),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text("strikethrough", InlineStyle::default().with_strikethrough(true));
    chips.push_text(", and ", InlineStyle::default());
    chips.push_text("clickable links", InlineStyle::default().with_link("https://gpui.rs"));
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
        Some(false),
        InlineContent::plain("Phase 2: EntityInputHandler IME bridge and rich text editing."),
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
                make_cell(&mut document, "gpui 0.2.2", false),
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
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1080.0), px(780.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        };
        if let Err(error) = cx.open_window(options, |_window, cx| cx.new(DocsViewerApp::new)) {
            eprintln!("docs_viewer: failed to open window: {error}");
        }
    });
}
