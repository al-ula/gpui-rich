//! Demo GPUI application showcasing the non-paginated `gpui-rich` renderer.

use gpui_kit::{
    App, AppContext, BoxShadow, ClipboardItem, Context, Entity, FontWeight as GpuiFontWeight,
    ImageFormat, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, point, px, rgb,
};
use gpui_rich::gpui::{
    BlockRenderContext, BlockRenderer, CompletionAccept, CompletionAcceptKeys,
    CompletionBlockScope, CompletionDetail, CompletionItem, CompletionProvider,
    CompletionQueryContext, CompletionQueryResult, CompletionTrigger, CustomRenderOutput,
    InlineRenderContext, InlineRenderer, MemoryAssetProvider, OverlayAnchor, OverlayDismissPolicy,
    OverlayId, OverlayPlacement, OverlaySpec, OverlayZLayer, RichDocumentEvent, RichDocumentView,
    RichTheme, ToolbarState, ViewState, fuzzy_match_label, inline_preview_suffix,
};
use gpui_rich::layout::{InlineBoxMetrics, Insets, pt_to_px};
use gpui_rich::model::{
    BaselineShift, BlockByteRange, BlockKind, BlockNode, BlockSpec, BlockStyle, BlockWidth,
    ChipKind, ColumnSpec, DecorationContext, DecorationProvider, DecorationWriter, Document, Edges,
    EditableSlotLayout, EquationBlockData, EquationBlockSpec, ExportDocument, FontWeight,
    HorizontalLayout, ImageAssetId, InlineContent, InlineDecorationStyle, InlineEquationData,
    InlineEquationSpec, InlineObject, InlineSpec, InlineStyle, InlineVAlign, InteractionPolicy,
    ListId, ListKind, MarkKind, MarkdownCodec, MermaidData, MermaidSpec, NumberStyle, PaperSpec,
    RgbaColor, RichCommand, SelectionValue, StyleId, TableBlock, TableBorderStyle, TableCell,
    TableRow, TextAlign, UnderlineKind, block_shape,
};
use gpui_rich::model::{SourceBuffer, SourceChangeSet, SourceLinePresentation, SourcePresentation};
use serde::{Deserialize, Serialize};

struct SourceDemoPresentation;

impl SourcePresentation for SourceDemoPresentation {
    fn project(
        &mut self,
        source: &SourceBuffer,
        _change: Option<&SourceChangeSet>,
    ) -> Vec<SourceLinePresentation> {
        source
            .lines()
            .iter()
            .enumerate()
            .filter_map(|(line, range)| {
                let text = &source.text()[range.clone()];
                if text.starts_with("# ") {
                    return Some(SourceLinePresentation {
                        line,
                        style: BlockStyle::named(StyleId::HEADING_1),
                        hidden: std::iter::once(0..2).collect(),
                    });
                }
                let (_, value) = text.split_once(" = ")?;
                let value_start = text.len() - value.len();
                let value_end = text.len();
                (value.len() >= 2 && value.starts_with('"') && value.ends_with('"')).then(|| {
                    SourceLinePresentation {
                        line,
                        style: BlockStyle::default(),
                        hidden: vec![value_start..value_start + 1, value_end - 1..value_end],
                    }
                })
            })
            .collect()
    }
}

struct RustSyntaxDecorationProvider;

impl DecorationProvider for RustSyntaxDecorationProvider {
    fn collect_decorations(
        &self,
        context: &DecorationContext<'_>,
        writer: &mut DecorationWriter<'_>,
    ) {
        let Some(code) = context.block.kind.code_block_data() else {
            return;
        };
        let Some(language) = code.language.as_deref() else {
            return;
        };
        let Some(content) = context.block.kind.inline_content() else {
            return;
        };
        if !language.eq_ignore_ascii_case("rust") {
            return;
        }

        let keyword_color = RgbaColor::rgb(0xcf, 0x22, 0x2e);
        let type_color = RgbaColor::rgb(0x82, 0x50, 0xdf);
        let text = content.text.as_str();
        let mut start: Option<usize> = None;

        for (index, character) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            let is_ident = character.is_ascii_alphanumeric() || character == '_';
            match (start, is_ident) {
                (None, true) => start = Some(index),
                (Some(word_start), false) => {
                    if let Some(word) = text.get(word_start..index) {
                        if matches!(
                            word,
                            "pub" | "trait" | "type" | "fn" | "mut" | "let" | "struct" | "impl"
                        ) {
                            if let Err(error) = writer.push_inline(
                                BlockByteRange::from_usize_range(word_start..index),
                                InlineDecorationStyle::new().with_foreground(keyword_color),
                            ) {
                                eprintln!("docs_viewer: syntax decoration error: {error}");
                            }
                        } else if word
                            .chars()
                            .next()
                            .is_some_and(|first| first.is_ascii_uppercase())
                            && let Err(error) = writer.push_inline(
                                BlockByteRange::from_usize_range(word_start..index),
                                InlineDecorationStyle::new().with_foreground(type_color),
                            )
                        {
                            eprintln!("docs_viewer: syntax decoration error: {error}");
                        }
                    }
                    start = None;
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CalloutData {
    title: String,
}

struct CalloutBlockSpec;

impl BlockSpec for CalloutBlockSpec {
    const KIND: &'static str = "callout";
    type Shape = block_shape::Container;
    type Data = CalloutData;
}

struct CalloutBlockRenderer;

impl BlockRenderer for CalloutBlockRenderer {
    type Spec = CalloutBlockSpec;

    fn measure_chrome(
        &self,
        _data: &CalloutData,
        _max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Insets {
        Insets::new(
            pt_to_px(26.0, zoom),
            pt_to_px(12.0, zoom),
            pt_to_px(10.0, zoom),
            pt_to_px(14.0, zoom),
        )
    }

    fn render(
        &self,
        context: BlockRenderContext<'_, CalloutData>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let is_dark = context.theme.is_dark();
        let card_bg = if is_dark {
            rgb(0x162235)
        } else {
            rgb(0xedf5ff)
        };
        let card_border = if is_dark {
            rgb(0x388bfd)
        } else {
            rgb(0x8ab4f8)
        };
        let title_color = if is_dark {
            rgb(0x79c0ff)
        } else {
            rgb(0x0b57d0)
        };

        let element = div()
            .size_full()
            .flex()
            .flex_col()
            .px(px(pt_to_px(12.0, context.zoom)))
            .pt(px(pt_to_px(7.0, context.zoom)))
            .rounded(px(pt_to_px(6.0, context.zoom)))
            .bg(card_bg)
            .border_1()
            .border_color(card_border)
            .child(
                div()
                    .text_size(px(pt_to_px(10.0, context.zoom)))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(title_color)
                    .child(context.data.title.clone()),
            )
            .into_any_element();

        Some(CustomRenderOutput::Element(element))
    }

    fn plain_text(&self, data: &CalloutData) -> Option<String> {
        Some(data.title.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StatusBadgeData {
    label: String,
}

struct StatusBadgeSpec;

impl InlineSpec for StatusBadgeSpec {
    const KIND: &'static str = "status";
    type Data = StatusBadgeData;
}

struct StatusBadgeRenderer;

impl InlineRenderer for StatusBadgeRenderer {
    type Spec = StatusBadgeSpec;

    fn measure(&self, data: &StatusBadgeData, zoom: f32, _window: &Window) -> InlineBoxMetrics {
        let font_px = pt_to_px(9.0, zoom);
        InlineBoxMetrics::new(
            (data.label.chars().count().max(3) as f32) * font_px * 0.62 + pt_to_px(14.0, zoom),
            pt_to_px(16.0, zoom),
            InlineVAlign::Middle,
        )
    }

    fn render(
        &self,
        context: InlineRenderContext<'_, StatusBadgeData>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let is_dark = context.theme.is_dark();
        let badge_bg = if is_dark {
            rgb(0x1b3a28)
        } else {
            rgb(0xdcfce7)
        };
        let badge_border = if is_dark {
            rgb(0x3fb950)
        } else {
            rgb(0x86efac)
        };
        let badge_text = if is_dark {
            rgb(0x7ee787)
        } else {
            rgb(0x166534)
        };

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
            .child(context.data.label.clone())
            .into_any_element();

        Some(CustomRenderOutput::Element(element))
    }

    fn plain_text(&self, data: &StatusBadgeData) -> Option<String> {
        Some(format!("[{}]", data.label))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayUnit {
    Pt,
    Px,
    In,
    Cm,
    Mm,
}

impl DisplayUnit {
    const ALL: [Self; 5] = [Self::Pt, Self::Px, Self::In, Self::Cm, Self::Mm];

    const fn suffix(self) -> &'static str {
        match self {
            Self::Pt => "pt",
            Self::Px => "px",
            Self::In => "in",
            Self::Cm => "cm",
            Self::Mm => "mm",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Pt => Self::Px,
            Self::Px => Self::In,
            Self::In => Self::Cm,
            Self::Cm => Self::Mm,
            Self::Mm => Self::Pt,
        }
    }

    fn convert_from_pt(self, pt: f32) -> f32 {
        match self {
            Self::Pt => pt,
            Self::Px => pt * (4.0 / 3.0),
            Self::In => pt / 72.0,
            Self::Cm => pt * 2.54 / 72.0,
            Self::Mm => pt * 25.4 / 72.0,
        }
    }

    fn to_pt(self, value: f32) -> f32 {
        match self {
            Self::Pt => value,
            Self::Px => value * 0.75,
            Self::In => value * 72.0,
            Self::Cm => value * 72.0 / 2.54,
            Self::Mm => value * 72.0 / 25.4,
        }
    }

    fn format_value(self, pt: f32) -> String {
        let converted = self.convert_from_pt(pt);
        match self {
            Self::Pt | Self::Px => format!("{converted:.0}{}", self.suffix()),
            Self::In | Self::Cm => format!("{converted:.2}{}", self.suffix()),
            Self::Mm => format!("{converted:.1}{}", self.suffix()),
        }
    }

    fn margin_step_pt(self) -> f32 {
        match self {
            Self::Pt => self.to_pt(6.0),
            Self::Px => self.to_pt(8.0),
            Self::In => self.to_pt(0.1),
            Self::Cm => self.to_pt(0.25),
            Self::Mm => self.to_pt(2.0),
        }
    }

    const fn margin_step_label(self) -> &'static str {
        match self {
            Self::Pt => "6pt",
            Self::Px => "8px",
            Self::In => "0.1in",
            Self::Cm => "0.25cm",
            Self::Mm => "2mm",
        }
    }

    fn font_size_step_pt(self) -> f32 {
        match self {
            Self::Px => self.to_pt(1.0),
            _ => 1.0,
        }
    }
}

pub struct DocsViewerApp {
    document_view: Entity<RichDocumentView>,
    _event_subscription: Subscription,
    last_paper_spec: PaperSpec,
    display_unit: DisplayUnit,
    dark_mode: bool,
    font_menu_open: bool,
    margin_menu_open: bool,
    available_fonts: Vec<SharedString>,
    status_message: SharedString,
}

struct DocsSlashCompletionProvider {
    triggers: [CompletionTrigger; 2],
}

impl DocsSlashCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::prefix("/"), CompletionTrigger::Manual],
        }
    }
}

impl CompletionProvider for DocsSlashCompletionProvider {
    fn id(&self) -> &'static str {
        "docs_viewer.slash"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::ProseOnly
    }

    fn priority(&self) -> i32 {
        10
    }

    fn header_title(&self) -> Option<&str> {
        Some("Slash Commands")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        _view: WeakEntity<RichDocumentView>,
        _cx: &mut App,
    ) -> CompletionQueryResult {
        struct SlashEntry {
            id: &'static str,
            label: &'static str,
            subtitle: &'static str,
            badge: &'static str,
            command: RichCommand,
            detail: Option<CompletionDetail>,
        }

        let entries = [
            SlashEntry {
                id: "heading-1",
                label: "Heading 1",
                subtitle: "Large section title",
                badge: "H1",
                command: RichCommand::SetHeading(Some(1)),
                detail: None,
            },
            SlashEntry {
                id: "heading-2",
                label: "Heading 2",
                subtitle: "Subsection heading",
                badge: "H2",
                command: RichCommand::SetHeading(Some(2)),
                detail: None,
            },
            SlashEntry {
                id: "heading-3",
                label: "Heading 3",
                subtitle: "Minor section heading",
                badge: "H3",
                command: RichCommand::SetHeading(Some(3)),
                detail: None,
            },
            SlashEntry {
                id: "bullet-list",
                label: "Bullet List",
                subtitle: "Start an unordered bullet list",
                badge: "list",
                command: RichCommand::ToggleList(ListKind::Bullet),
                detail: None,
            },
            SlashEntry {
                id: "checklist",
                label: "Checklist",
                subtitle: "Interactive task list with checkboxes",
                badge: "todo",
                command: RichCommand::ToggleList(ListKind::Checklist),
                detail: None,
            },
            SlashEntry {
                id: "blockquote",
                label: "Blockquote",
                subtitle: "Indented callout quotation",
                badge: "quote",
                command: RichCommand::ToggleBlockquote,
                detail: None,
            },
            SlashEntry {
                id: "code-block",
                label: "Code Block",
                subtitle: "Monospace block with syntax highlighting",
                badge: "```",
                command: RichCommand::ToggleCodeBlock,
                detail: Some(
                    CompletionDetail::new(
                        "Code Block",
                        "Creates a syntax-highlighted monospace block with built-in copy support.",
                    )
                    .with_preview("fn main() {\n    println!(\"hello\");\n}"),
                ),
            },
            SlashEntry {
                id: "table",
                label: "Table (3×3)",
                subtitle: "Insert a 3-column table with header row",
                badge: "table",
                command: RichCommand::InsertTable {
                    rows: 3,
                    columns: 3,
                    with_header: true,
                },
                detail: Some(
                    CompletionDetail::new(
                        "Structured Table",
                        "Inserts a resizable 3×3 grid table with a styled header row.",
                    )
                    .with_preview("| Column 1 | Column 2 | Column 3 |"),
                ),
            },
        ];

        let mut items = Vec::new();
        for entry in entries {
            if let Some((score, indices)) = fuzzy_match_label(context.query, entry.label) {
                let mut item = CompletionItem::new(
                    entry.id,
                    entry.label,
                    CompletionAccept::command(entry.command),
                )
                .with_subtitle(entry.subtitle)
                .with_badge(entry.badge)
                .with_match(score, indices);
                if let Some(detail) = entry.detail {
                    item = item.with_detail(detail);
                }
                items.push(item);
            }
        }
        CompletionQueryResult::Ready(items)
    }
}

struct DocsMentionCompletionProvider {
    triggers: [CompletionTrigger; 1],
}

impl DocsMentionCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::prefix_with_spaces("@")],
        }
    }
}

impl CompletionProvider for DocsMentionCompletionProvider {
    fn id(&self) -> &'static str {
        "docs_viewer.mentions"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::ProseOnly
    }

    fn header_title(&self) -> Option<&str> {
        Some("People & Chips")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        view: WeakEntity<RichDocumentView>,
        cx: &mut App,
    ) -> CompletionQueryResult {
        let local_people = [
            ("alex", "Alex Rivera", "Core Layout · Recent", "local"),
            ("sam", "Sam Patel", "Renderer · Recent", "local"),
        ];
        let mut initial = Vec::new();
        for (id, name, subtitle, badge) in local_people {
            if let Some((score, indices)) = fuzzy_match_label(context.query, name) {
                initial.push(
                    CompletionItem::new(
                        id,
                        name,
                        CompletionAccept::replace_inline_object(InlineObject::SmartChip {
                            kind: ChipKind::Person,
                            label: name.to_owned(),
                            accent: None,
                        }),
                    )
                    .with_subtitle(subtitle)
                    .with_badge(badge)
                    .with_match(score, indices),
                );
            }
        }

        let query_owned = context.query.to_owned();
        let session_id = context.session_id;
        let query_revision = context.query_revision;
        let provider_id = self.id();
        let mut remote_items = initial.clone();

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(90))
                .await;
            let directory = [
                (
                    "alice",
                    "Alice Chen",
                    "Design Systems · alice@acme.dev",
                    "dir",
                ),
                (
                    "diego",
                    "Diego Morales",
                    "Text Shaping · diego@acme.dev",
                    "dir",
                ),
                ("mina", "Mina Park", "Product · mina@acme.dev", "dir"),
            ];
            for (id, name, subtitle, badge) in directory {
                if let Some((score, indices)) = fuzzy_match_label(&query_owned, name) {
                    remote_items.push(
                        CompletionItem::new(
                            id,
                            name,
                            CompletionAccept::replace_inline_object(InlineObject::SmartChip {
                                kind: ChipKind::Person,
                                label: name.to_owned(),
                                accent: None,
                            }),
                        )
                        .with_subtitle(subtitle)
                        .with_badge(badge)
                        .with_match(score + 4, indices),
                    );
                }
            }
            if view
                .update(cx, |document_view, cx| {
                    document_view.update_completion_items(
                        session_id,
                        query_revision,
                        provider_id,
                        remote_items,
                        true,
                        cx,
                    );
                })
                .is_err()
            {
                // View was dropped before the simulated directory lookup completed.
            }
        })
        .detach();

        CompletionQueryResult::Pending { initial }
    }
}

struct RustCodeCompletionProvider {
    triggers: [CompletionTrigger; 2],
}

impl RustCodeCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::word_prefix(2), CompletionTrigger::Manual],
        }
    }
}

impl CompletionProvider for RustCodeCompletionProvider {
    fn id(&self) -> &'static str {
        "docs_viewer.rust_symbols"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::CodeBlocks {
            language: Some("rust".into()),
        }
    }

    fn accept_keys(&self) -> CompletionAcceptKeys {
        CompletionAcceptKeys::TabOrSelectedEnter
    }

    fn header_title(&self) -> Option<&str> {
        Some("Rust Symbols")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        _view: WeakEntity<RichDocumentView>,
        _cx: &mut App,
    ) -> CompletionQueryResult {
        let symbols: [(&str, &str, &str, Option<&str>); 8] = [
            ("return", "keyword", "kw", None),
            (
                "retain",
                "fn(&mut self, f: F)",
                "method",
                Some("Retains only the elements specified by the predicate."),
            ),
            (
                "Result",
                "enum Result<T, E>",
                "type",
                Some("Type used for returning and propagating recoverable errors."),
            ),
            (
                "Option",
                "enum Option<T>",
                "type",
                Some("Represents an optional value: Some(T) or None."),
            ),
            ("struct", "keyword", "kw", None),
            ("impl", "keyword", "kw", None),
            ("match", "keyword", "kw", None),
            ("pub fn", "snippet", "snip", None),
        ];

        let mut items = Vec::new();
        for (label, subtitle, badge, docs) in symbols {
            if let Some((score, indices)) = fuzzy_match_label(context.query, label) {
                let mut item =
                    CompletionItem::new(label, label, CompletionAccept::replace_text(label))
                        .with_subtitle(subtitle)
                        .with_badge(badge)
                        .with_match(score, indices);
                if let Some(preview) = inline_preview_suffix(context.query, label) {
                    item = item.with_inline_preview(preview);
                }
                if let Some(description) = docs {
                    item = item.with_detail(
                        CompletionDetail::new(label, description).with_preview(subtitle),
                    );
                }
                items.push(item);
            }
        }
        CompletionQueryResult::Ready(items)
    }
}

const SELECTION_BUBBLE_OVERLAY_ID: &str = "docs_viewer.selection_bubble";

fn sync_selection_bubble_overlay(view: &mut RichDocumentView, cx: &mut Context<RichDocumentView>) {
    let overlay_id = OverlayId::new(SELECTION_BUBBLE_OVERLAY_ID);
    let show = view.policy().editing.enabled
        && matches!(
            &view.selection,
            Some(gpui_rich::model::DocumentSelection::Text { anchor, focus, .. }) if anchor != focus
        );
    if !show {
        view.remove_overlay(&overlay_id, cx);
        return;
    }

    let spec = OverlaySpec::new(
        overlay_id,
        OverlayAnchor::Selection,
        |context, _window, _cx| {
            let theme = context.theme;
            let build_btn = |id: &'static str, label: &'static str, cmd: RichCommand| {
                let view = context.view.clone();
                div()
                    .id(id)
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded(px(5.0))
                    .text_size(px(11.5))
                    .font_weight(GpuiFontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.code_background))
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                        cx.stop_propagation();
                        let command = cmd.clone();
                        view.update(cx, |this, cx| {
                            this.dispatch_command(command, cx);
                        });
                    })
                    .child(label)
            };

            div()
                .px(px(5.0))
                .py(px(4.0))
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
                .items_center()
                .gap(px(2.0))
                .child(build_btn(
                    "bubble-bold",
                    "B",
                    RichCommand::ToggleMark(MarkKind::Bold),
                ))
                .child(build_btn(
                    "bubble-italic",
                    "I",
                    RichCommand::ToggleMark(MarkKind::Italic),
                ))
                .child(build_btn(
                    "bubble-underline",
                    "U",
                    RichCommand::ToggleMark(MarkKind::Underline),
                ))
                .child(build_btn(
                    "bubble-code",
                    "</>",
                    RichCommand::ToggleMark(MarkKind::InlineCode),
                ))
                .child(build_btn(
                    "bubble-highlight",
                    "Highlight",
                    RichCommand::SetInlineStyle(InlineStyle {
                        background: Some(RgbaColor::rgb(254, 240, 138)),
                        ..InlineStyle::default()
                    }),
                ))
                .into_any_element()
        },
    )
    .with_placement(OverlayPlacement::above_center())
    .with_z_layer(OverlayZLayer::Interactive)
    .with_dismiss_policy(OverlayDismissPolicy::selection_toolbar());

    view.set_overlay(spec, cx);
}

impl DocsViewerApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let assets = MemoryAssetProvider::new();
        let document = build_sample_document(&assets);
        let document_view = cx.new(|cx| {
            let mut view = RichDocumentView::new(document, cx)
                .with_asset_provider(assets)
                .with_decoration_provider(RustSyntaxDecorationProvider);
            if let Err(error) = view.register_block_renderer(CalloutBlockRenderer, cx) {
                eprintln!("docs_viewer: failed to register callout renderer: {error}");
            }
            if let Err(error) = view.register_inline_renderer(StatusBadgeRenderer, cx) {
                eprintln!("docs_viewer: failed to register status renderer: {error}");
            }
            #[cfg(feature = "mermaid")]
            if let Err(error) = view.install_mermaid(cx) {
                eprintln!("docs_viewer: failed to install mermaid extension: {error}");
            }
            #[cfg(feature = "equation")]
            if let Err(error) = view.install_equation(cx) {
                eprintln!("docs_viewer: failed to install equation extension: {error}");
            }
            if let Err(error) = view.install_emoji(cx) {
                eprintln!("docs_viewer: failed to install emoji extension: {error}");
            }
            if let Err(error) =
                view.register_completion_provider(DocsSlashCompletionProvider::new())
            {
                eprintln!("docs_viewer: failed to register slash completions: {error}");
            }
            if let Err(error) =
                view.register_completion_provider(DocsMentionCompletionProvider::new())
            {
                eprintln!("docs_viewer: failed to register mention completions: {error}");
            }
            if let Err(error) = view.register_completion_provider(RustCodeCompletionProvider::new())
            {
                eprintln!("docs_viewer: failed to register rust completions: {error}");
            }
            view
        });
        let event_subscription = cx.subscribe(
            &document_view,
            |this, view, event: &RichDocumentEvent, cx| match event {
                RichDocumentEvent::LinkActivated { href } => {
                    this.status_message = format!("Link: {href}").into();
                    cx.notify();
                }
                RichDocumentEvent::SmartChipActivated { label, .. } => {
                    this.status_message = format!("Chip: {label}").into();
                    cx.notify();
                }
                RichDocumentEvent::CodeBlockCopied { .. } => {
                    this.status_message = SharedString::from("Code block copied");
                    cx.notify();
                }
                RichDocumentEvent::DocumentMutated { change_set } => {
                    this.status_message =
                        format!("Mutated {} block(s)", change_set.dirty_blocks.len()).into();
                    cx.notify();
                }
                RichDocumentEvent::SourceEdited { change } => {
                    this.status_message = format!("Source revision {}", change.revision).into();
                    cx.notify();
                }
                RichDocumentEvent::SelectionChanged { .. } => {
                    view.update(cx, |document_view, cx| {
                        sync_selection_bubble_overlay(document_view, cx);
                    });
                    cx.notify();
                }
                RichDocumentEvent::CommandExecuted { .. } => {}
            },
        );
        Self {
            document_view,
            _event_subscription: event_subscription,
            last_paper_spec: PaperSpec::LETTER,
            display_unit: DisplayUnit::Pt,
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
            self.document_view
                .read(cx)
                .document
                .config
                .horizontal_layout,
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
        let view_state = self.document_view.read(cx).query(ViewState);
        let zoom = view_state.zoom;
        let horizontal_layout = view_state.horizontal_layout;
        let active_margins = view_state.margins_pt;
        let debug_active = view_state.debug_overlay;
        let policy = view_state.policy;
        let reveal_source = self.document_view.read(cx).source_syntax_revealed();
        if !policy.editing.enabled {
            self.font_menu_open = false;
        }
        let dark = self.dark_mode;
        let is_fill = matches!(horizontal_layout, HorizontalLayout::Fill);
        let active_paper = match horizontal_layout {
            HorizontalLayout::Paper(spec) => Some(spec),
            HorizontalLayout::Fill => None,
        };
        let margins_button_label =
            format!("Margins: {} ▾", Self::margin_preset_label(active_margins));
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
                Self::toolbar_button(
                    "source-demo",
                    match reveal_source {
                        None => "Source Demo",
                        Some(false) => "Show Syntax",
                        Some(true) => "Hide Syntax",
                    },
                    reveal_source == Some(true),
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.document_view.update(cx, |view, cx| {
                            if let Some(revealed) = view.source_syntax_revealed() {
                                view.reveal_source(!revealed, cx);
                            } else {
                                let source = "# Source-backed editing\r\n\
                                    Click before the heading and press Backspace to delete its hidden space.\n\
                                    \n\
                                    title = \"A TOML-style value\"\n\
                                    Show Syntax reveals the original delimiters without editing the file.";
                                view.set_source_buffer(SourceBuffer::new(source.into()), cx);
                                view.set_source_presentation(
                                    Some(Box::new(SourceDemoPresentation)),
                                    cx,
                                );
                                view.set_policy(InteractionPolicy::editable(), cx);
                            }
                        });
                        cx.notify();
                    }),
                ),
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

        let unit = self.display_unit;
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
            )
            .child(
                Self::toolbar_button(
                    "unit-cycle",
                    format!("Unit: {}", unit.suffix()),
                    false,
                    dark,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.display_unit = this.display_unit.next();
                        this.status_message =
                            format!("Unit: {}", this.display_unit.suffix()).into();
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
                            sync_selection_bubble_overlay(view, cx);
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
                            sync_selection_bubble_overlay(view, cx);
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
                            sync_selection_bubble_overlay(view, cx);
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
                Self::toolbar_button(
                    "toggle-theme",
                    if dark { "Light" } else { "Dark" },
                    dark,
                    dark,
                )
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

        let mut active_font_selection: Option<SelectionValue<gpui_rich::model::SharedStr>> = None;

        if policy.editing.enabled {
            let toolbar_state = self.document_view.read(cx).query(ToolbarState);
            let formatting = &toolbar_state.formatting;
            let bold_active = formatting.is_mark_active(MarkKind::Bold);
            let italic_active = formatting.is_mark_active(MarkKind::Italic);
            let underline_active = formatting.is_mark_active(MarkKind::Underline);
            let strike_active = formatting.is_mark_active(MarkKind::Strikethrough);
            let code_active = formatting.is_mark_active(MarkKind::InlineCode);
            let font_family = formatting.font_family.clone();
            let font_size_pt = formatting.font_size_pt;
            let heading_level = formatting.heading_level;
            let list_kind = formatting.active_list_kind;
            let text_align = formatting.text_align;

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
                Some(SelectionValue::Uniform(pt)) => unit.format_value(pt),
                Some(SelectionValue::Mixed) => "Mixed".to_owned(),
                None => "--".to_owned(),
            };
            let font_step_pt = unit.font_size_step_pt();
            let uniform_heading = heading_level.and_then(SelectionValue::uniform);
            let uniform_align = text_align.and_then(SelectionValue::uniform);

            let mut format_bar = div()
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
                        cx.listener(move |this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::AdjustFontSize(-font_step_pt),
                                    cx,
                                );
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
                        cx.listener(move |this, _event, _window, cx| {
                            this.document_view.update(cx, |view, cx| {
                                view.dispatch_command(
                                    RichCommand::AdjustFontSize(font_step_pt),
                                    cx,
                                );
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
                )
                .child(
                    Self::toolbar_button("fmt-export-md", "Copy MD", false, dark).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            let exported = this
                                .document_view
                                .read(cx)
                                .query(ExportDocument(&MarkdownCodec));
                            match exported {
                                Ok(markdown) => {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        markdown.into_string(),
                                    ));
                                    this.status_message =
                                        SharedString::from("Exported Markdown to clipboard");
                                }
                                Err(error) => {
                                    this.status_message = format!("Export failed: {error}").into();
                                }
                            }
                            cx.notify();
                        }),
                    ),
                );
            if let Some(table_ctx) = toolbar_state.table {
                format_bar = format_bar
                    .child(div().w(px(1.0)).h(px(16.0)).bg(toolbar_border).mx(px(2.0)))
                    .child(
                        Self::toolbar_button("tbl-add-row", "+Row", false, dark).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(
                                        RichCommand::InsertTableRow { after: true },
                                        cx,
                                    );
                                });
                            }),
                        ),
                    )
                    .child(
                        Self::toolbar_button("tbl-del-row", "-Row", false, dark).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(RichCommand::DeleteTableRow, cx);
                                });
                            }),
                        ),
                    )
                    .child(
                        Self::toolbar_button("tbl-add-col", "+Col", false, dark).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(
                                        RichCommand::InsertTableColumn { after: true },
                                        cx,
                                    );
                                });
                            }),
                        ),
                    )
                    .child(
                        Self::toolbar_button("tbl-del-col", "-Col", false, dark).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.document_view.update(cx, |view, cx| {
                                    view.dispatch_command(RichCommand::DeleteTableColumn, cx);
                                });
                            }),
                        ),
                    )
                    .child(
                        Self::toolbar_button("tbl-merge", "Merge", table_ctx.can_merge, dark)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.document_view.update(cx, |view, cx| {
                                        view.dispatch_command(RichCommand::MergeTableCells, cx);
                                    });
                                }),
                            ),
                    )
                    .child(
                        Self::toolbar_button("tbl-split", "Split", table_ctx.can_split, dark)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.document_view.update(cx, |view, cx| {
                                        view.dispatch_command(RichCommand::SplitTableCell, cx);
                                    });
                                }),
                            ),
                    );
            }
            root = root.child(format_bar);
        }

        let mut root = root.child(div().flex_1().size_full().child(self.document_view.clone()));

        if self.font_menu_open && policy.editing.enabled {
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
                let preview_family = gpui_rich::gpui::resolve_font_family(value, window);
                list = list.child(
                    div()
                        .id(("font-generic", index))
                        .flex_shrink_0()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(if is_selected { selected_bg } else { menu_bg })
                        .hover(move |style| {
                            style.bg(if is_selected { selected_bg } else { hover_bg })
                        })
                        .text_color(if is_selected {
                            selected_text
                        } else {
                            title_color
                        })
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
                        .hover(move |style| {
                            style.bg(if is_selected { selected_bg } else { hover_bg })
                        })
                        .text_color(if is_selected {
                            selected_text
                        } else {
                            title_color
                        })
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

            let mut unit_row = div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(6.0))
                .pb(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(muted_color)
                        .child("Unit"),
                );
            let mut unit_pills = div().flex().flex_row().items_center().gap(px(3.0));
            for (index, candidate) in DisplayUnit::ALL.into_iter().enumerate() {
                let is_active = candidate == unit;
                unit_pills = unit_pills.child(
                    div()
                        .id(("margin-unit-option", index))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(if is_active {
                            selected_bg
                        } else {
                            step_button_bg
                        })
                        .text_color(if is_active {
                            selected_text
                        } else {
                            title_color
                        })
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.display_unit = candidate;
                                this.status_message =
                                    format!("Unit: {}", candidate.suffix()).into();
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(candidate.suffix()),
                );
            }
            unit_row = unit_row.child(unit_pills);

            let mut panel_content = div()
                .flex()
                .flex_col()
                .p(px(8.0))
                .gap(px(4.0))
                .child(unit_row)
                .child(div().h(px(1.0)).w_full().bg(toolbar_border).my(px(2.0)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(muted_color)
                        .px(px(6.0))
                        .pb(px(2.0))
                        .child("Margin Presets"),
                );

            let presets: [(&str, String, Edges); 4] = [
                (
                    "Narrow",
                    unit.format_value(Edges::MARGINS_NARROW.left),
                    Edges::MARGINS_NARROW,
                ),
                (
                    "Moderate",
                    unit.format_value(Edges::MARGINS_MODERATE.left),
                    Edges::MARGINS_MODERATE,
                ),
                (
                    "Normal",
                    unit.format_value(Edges::MARGINS_NORMAL.left),
                    Edges::MARGINS_NORMAL,
                ),
                (
                    "Wide",
                    format!(
                        "{} / {}",
                        unit.format_value(Edges::MARGINS_WIDE.left),
                        unit.format_value(Edges::MARGINS_WIDE.top),
                    ),
                    Edges::MARGINS_WIDE,
                ),
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
                        .hover(move |style| {
                            style.bg(if is_selected { selected_bg } else { hover_bg })
                        })
                        .text_color(if is_selected {
                            selected_text
                        } else {
                            title_color
                        })
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
                                .text_color(if is_selected {
                                    selected_text
                                } else {
                                    muted_color
                                })
                                .child(detail),
                        ),
                );
            }

            let step_pt = unit.margin_step_pt();
            panel_content = panel_content
                .child(div().h(px(1.0)).w_full().bg(toolbar_border).my(px(4.0)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(muted_color)
                        .px(px(6.0))
                        .pb(px(2.0))
                        .child(format!("Custom Edges ({} step)", unit.margin_step_label())),
                );

            let edge_rows: [(&str, f32, Edges, Edges); 4] = [
                (
                    "Top",
                    active_margins.top,
                    Edges::new(
                        active_margins.top - step_pt,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top + step_pt,
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
                        active_margins.bottom - step_pt,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom + step_pt,
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
                        active_margins.left - step_pt,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right,
                        active_margins.bottom,
                        active_margins.left + step_pt,
                    ),
                ),
                (
                    "Right",
                    active_margins.right,
                    Edges::new(
                        active_margins.top,
                        active_margins.right - step_pt,
                        active_margins.bottom,
                        active_margins.left,
                    ),
                    Edges::new(
                        active_margins.top,
                        active_margins.right + step_pt,
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
                                        .w(px(54.0))
                                        .text_size(px(11.0))
                                        .text_color(muted_color)
                                        .child(unit.format_value(current_pt)),
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

const SAMPLE_DIAGRAM_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="72" viewBox="0 0 320 72">
  <defs>
    <linearGradient id="bg" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#e8f0fe"/>
      <stop offset="100%" stop-color="#dcfce7"/>
    </linearGradient>
  </defs>
  <rect x="2" y="2" width="316" height="68" rx="10" fill="url(#bg)" stroke="#8ab4f8" stroke-width="2"/>
  <rect x="16" y="18" width="76" height="36" rx="6" fill="#0b57d0"/>
  <rect x="122" y="18" width="76" height="36" rx="6" fill="#166534"/>
  <rect x="228" y="18" width="76" height="36" rx="6" fill="#6b21a8"/>
  <path d="M94 36 L118 36 M200 36 L224 36" stroke="#1f2328" stroke-width="2.5" stroke-linecap="round"/>
</svg>"##;

fn build_sample_document(assets: &MemoryAssetProvider) -> Document {
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
    document.add_heading(
        1,
        InlineContent::plain("1. Mixed-Baseline Typography & Inline Objects"),
    );

    let mut mixed = InlineContent::empty();
    mixed.push_text("A single visual line aligns ", InlineStyle::default());
    mixed.push_text("11pt bold", InlineStyle::bold());
    mixed.push_text(", ", InlineStyle::default());
    mixed.push_text("italic", InlineStyle::italic());
    mixed.push_text(", and ", InlineStyle::default());
    mixed.push_text("serif", InlineStyle::default().with_font_family("serif"));
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
    mixed.push_text(
        "O on one shared typographic baseline.",
        InlineStyle::default(),
    );
    document.add_paragraph(mixed, BlockStyle::default());

    let mut chips = InlineContent::empty();
    chips.push_text(
        "Inline atomic embeds use U+FFFC sentinels: ",
        InlineStyle::default(),
    );
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
        InlineStyle::default()
            .with_underline(UnderlineKind::Wavy(RgbaColor::rgb(0xcf, 0x22, 0x2e))),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text(
        "strikethrough",
        InlineStyle::default().with_strikethrough(true),
    );
    chips.push_text(", ", InlineStyle::default());
    chips.push_text(
        "clickable links",
        InlineStyle::default().with_link("https://gpui.rs"),
    );
    chips.push_text(", and ", InlineStyle::default());
    chips.push_text(
        "internal doc URIs",
        InlineStyle::default().with_link("doc://architecture/phase-2"),
    );
    chips.push_object(
        InlineObject::FootnoteRef { index: 1 },
        InlineStyle::default(),
    );
    chips.push_text(".", InlineStyle::default());
    document.add_paragraph(chips, BlockStyle::default());

    let mut inline_img = InlineContent::empty();
    inline_img.push_text("Host-provided image: ", InlineStyle::default());
    match ImageAssetId::new("asset://docs-viewer/diagram.svg") {
        Ok(asset_id) => {
            assets.insert_encoded(
                asset_id.clone(),
                ImageFormat::Svg,
                SAMPLE_DIAGRAM_SVG.as_bytes(),
            );
            inline_img.push_object(
                InlineObject::Image {
                    asset_id,
                    alt: "Diagram 160x36".into(),
                    width_pt: 120.0,
                    height_pt: 28.0,
                    valign: InlineVAlign::Middle,
                },
                InlineStyle::default(),
            );
        }
        Err(error) => eprintln!("docs_viewer: invalid diagram source: {error}"),
    }
    inline_img.push_text(" and bundled PNG: ", InlineStyle::default());
    match ImageAssetId::new(concat!(
        "data:image/png;base64,",
        "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAAD0lEQVR4nGO4ppLC0fAEAAlLAsu5bKHnAAAAAElFTkSuQmCC"
    )) {
        Ok(asset_id) => {
            inline_img.push_object(
                InlineObject::Image {
                    asset_id,
                    alt: "Two-color swatch".into(),
                    width_pt: 56.0,
                    height_pt: 28.0,
                    valign: InlineVAlign::Middle,
                },
                InlineStyle::default(),
            );
        }
        Err(error) => eprintln!("docs_viewer: invalid swatch source: {error}"),
    }
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
        InlineContent::plain(
            "Click the triangle chevron next to any heading to collapse or expand its section.",
        ),
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
    document.add_list_item(
        ListId(3),
        ListKind::Checklist,
        0,
        Some(false),
        InlineContent::plain("Phase 3: Click checklists in Edit mode, drag image handles, drag table column borders, click smart chips, and copy code blocks."),
    );

    let mut bidi_sample = InlineContent::empty();
    bidi_sample.push_text(
        "Unicode BiDi (UAX #9) splits runs and reorders mixed scripts inline: ",
        InlineStyle::default(),
    );
    bidi_sample.push_text("שלום עולם", InlineStyle::bold());
    bidi_sample.push_text(" and ", InlineStyle::default());
    bidi_sample.push_text("مرحبا بالعالم", InlineStyle::italic());
    bidi_sample.push_text(" alongside English prose.", InlineStyle::default());
    document.add_paragraph(bidi_sample, BlockStyle::default());

    let mut ext_intro = InlineContent::empty();
    ext_intro.push_text(
        "Typed inline extensions render hosted GPUI widgets inline ",
        InlineStyle::default(),
    );
    ext_intro.push_object(
        InlineObject::custom::<StatusBadgeSpec>(StatusBadgeData {
            label: "TYPED EXTENSIONS".into(),
        }),
        InlineStyle::default(),
    );
    ext_intro.push_text(
        " alongside editable custom container blocks:",
        InlineStyle::default(),
    );
    document.add_paragraph(ext_intro, BlockStyle::default());

    let mut title = InlineContent::empty();
    title.push_text("Editable card title", InlineStyle::bold());
    let callout_title = BlockNode::new(
        document.alloc_block_id(),
        BlockKind::paragraph(title),
        BlockStyle::default(),
    );
    let callout_body_id = document.alloc_block_id();
    let callout_body = BlockNode::new(
        callout_body_id,
        BlockKind::paragraph(InlineContent::plain(
            "This body and the title are native text slots. Select across them, or press Enter to add a line without creating a new slot.",
        )),
        BlockStyle::default(),
    );
    if let Some(kind) = BlockKind::slotted_container::<CalloutBlockSpec>(
        CalloutData {
            title: "EDITABLE CARD".into(),
        },
        EditableSlotLayout::SideBySide,
        vec![
            ("title".into(), callout_title),
            ("body".into(), callout_body),
        ],
    ) {
        let card = BlockNode::new(document.alloc_block_id(), kind, BlockStyle::default());
        document.push_block(card);
    } else {
        eprintln!("docs_viewer: invalid editable card slots");
    }

    document.add_leaf_block::<MermaidSpec>(
        MermaidData::new(
            "flowchart LR\n    Doc[Document Tree] --> Flow[FlowLayout]\n    Flow --> Cache[Two-Tier SVG Cache]\n    Cache --> Canvas[GPUI Layer 4 Paint]",
        )
        .with_caption("Built-in Mermaid diagram extension (click a label to edit directly, or switch to Split / Source)"),
    );

    let mut math_prose = InlineContent::empty();
    math_prose.push_text(
        "Built-in LaTeX equations powered by RaTeX align inline formulas such as ",
        InlineStyle::default(),
    );
    math_prose.push_object(
        InlineObject::custom::<InlineEquationSpec>(InlineEquationData::new(r"e^{i\pi} + 1 = 0")),
        InlineStyle::default(),
    );
    math_prose.push_text(" and ", InlineStyle::default());
    math_prose.push_object(
        InlineObject::custom::<InlineEquationSpec>(InlineEquationData::new(r"\frac{a}{b + c_i}")),
        InlineStyle::default(),
    );
    math_prose.push_text(
        " directly on the prose baseline (click any equation to edit, or type $...$ / $$ ):",
        InlineStyle::default(),
    );
    document.add_paragraph(math_prose, BlockStyle::default());

    document.add_leaf_block::<EquationBlockSpec>(
        EquationBlockData::new(
            r"\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}, \qquad \sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
        )
        .with_numbering("1"),
    );

    let quote_child_id = document.alloc_block_id();
    let quote_child = BlockNode::new(
        quote_child_id,
        BlockKind::paragraph(InlineContent::plain(
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
        let para = BlockNode::new(
            para_id,
            BlockKind::paragraph(content),
            BlockStyle::default(),
        );
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
        column_kinds: Default::default(),
        border_style: TableBorderStyle::default(),
    };
    document.push_block(
        BlockNode::new(
            table_id,
            BlockKind::table(table_block),
            BlockStyle::default(),
        )
        .with_width_mode(BlockWidth::Wide),
    );

    document
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, size};
    use gpui_rich::gpui::register_default_keybindings;

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
