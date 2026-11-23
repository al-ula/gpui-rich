//! Interactive GPUI view hosting [`RichDocumentElement`] with command-driven actions,
//! policy gating, theming, host events, keyboard navigation, IME input, and core editing.

use crate::asset::{AssetProvider, LoadedAsset, decode_image_asset};
use crate::element::RichDocumentElement;
use crate::extension::{
    BlockRenderer, CommandExecutionResult, EditCommandContext, ExtensionDiagnostic,
    ExtensionRegistrationError, ExtensionRegistry, InlineRenderer, ReadCommandContext,
    ReadCommandOutcome,
};
use crate::shaper::{FontResolver, GpuiGlyphBackend, GpuiShaper};
use crate::theme::RichTheme;
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    Font, InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point as GpuiPoint, Render, ScrollDelta, ScrollEvent,
    Styled, Task, UTF16Selection, Window, actions, div, point, px, size,
};
use rich_layout::{
    FlowLayout, Point, Rect, ShaperRunSpec, TableColumnDividerHit, pt_to_px,
    resolve_horizontal_metrics,
};
use rich_model::{
    Affinity, BlockId, BlockKind, BlockNode, BlockSpec, BlockStyle, ChangeSet, ChipKind,
    ClipboardBlock, ClipboardFragment, CodecError, ColumnSpec, CommandDecision, CommandEffectKind,
    CommandMiddleware, CommandOutcome, CommandResolution, CommandSource, CrdtSyncBridge,
    DecorationProvider, DocPos, Document, DocumentCodec, DocumentSelection, DynamicDocumentCodec,
    Edit, Edges, EncodedDocument, ExtensionCommand, ExtensionInvocation, History,
    HistoryMergeGroup, HorizontalLayout, ImageAssetId, InlineContent, InlineObject, InlineObjectId,
    InlineSpec, InlineStyle, InteractionPolicy, LinkTarget, ListKind, MarkKind, MoveDirection,
    MoveUnit, NamedStyle, NumberStyle, OBJECT_REPLACEMENT_UTF8_LEN, PathSegment, ReadOnly,
    ResolvedCommand, ResolvedTextStyle, RgbaColor, RichCommand, SelectionValue, SharedStr, Step,
    StyleId, SyncDelta, TextAlign, Transaction,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

#[path = "editor_anchors.rs"]
mod editor_anchors;
#[path = "editor_clipboard.rs"]
mod editor_clipboard;
#[path = "editor_components.rs"]
pub(crate) mod editor_components;
#[path = "editor_grid.rs"]
mod editor_grid;
#[path = "editor_navigation.rs"]
mod editor_navigation;
#[path = "editor_structure.rs"]
mod editor_structure;

/// GPUI keymap context attached to [`RichDocumentView`].
pub const RICH_DOCUMENT_KEY_CONTEXT: &str = "RichDocument";

const CARET_BLINK_PAUSE_DELAY: Duration = Duration::from_millis(500);
const CARET_SCROLL_MARGIN_PX: f32 = 28.0;
const MAX_LIST_INDENT_LEVEL: u8 = 6;
const CLIPBOARD_HTML_METADATA_KEY: &str = "gpui_rich_html";
const CLIPBOARD_FRAGMENT_METADATA_KEY: &str = "gpui_rich_fragment_v1";

actions!(
    rich_document,
    [
        Copy,
        Cut,
        Paste,
        PastePlainText,
        SelectAll,
        ClearSelection,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleDebugOverlay,
        Undo,
        Redo,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveLineStart,
        MoveLineEnd,
        SelectLineStart,
        SelectLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectDocStart,
        SelectDocEnd,
        Backspace,
        Delete,
        BackspaceWord,
        DeleteWord,
        Enter,
        SoftBreak,
        Indent,
        Outdent,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline,
        ToggleStrikethrough,
        ToggleInlineCode,
        ToggleSuperscript,
        ToggleSubscript,
        SetHeading1,
        SetHeading2,
        SetHeading3,
        SetParagraph,
        ToggleBulletList,
        ToggleOrderedList,
        ToggleChecklist,
        AlignLeft,
        AlignCenter,
        AlignRight,
        AlignJustify,
        ClearFormatting,
        IncreaseFontSize,
        DecreaseFontSize,
        InsertTableRowAbove,
        InsertTableRowBelow,
        DeleteTableRow,
        InsertTableColumnBefore,
        InsertTableColumnAfter,
        DeleteTableColumn,
        MergeTableCells,
        SplitTableCell,
    ]
);

/// Registers default cross-platform keybindings scoped to [`RICH_DOCUMENT_KEY_CONTEXT`].
pub fn register_default_keybindings(cx: &mut App) {
    let context = Some(RICH_DOCUMENT_KEY_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("escape", ClearSelection, context),
        KeyBinding::new("left", MoveLeft, context),
        KeyBinding::new("right", MoveRight, context),
        KeyBinding::new("up", MoveUp, context),
        KeyBinding::new("down", MoveDown, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", MoveLineStart, context),
        KeyBinding::new("end", MoveLineEnd, context),
        KeyBinding::new("shift-home", SelectLineStart, context),
        KeyBinding::new("shift-end", SelectLineEnd, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("enter", Enter, context),
        KeyBinding::new("shift-enter", SoftBreak, context),
        KeyBinding::new("tab", Indent, context),
        KeyBinding::new("shift-tab", Outdent, context),
    ];

    #[cfg(target_os = "macos")]
    bindings.extend([
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-shift-v", PastePlainText, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-=", ZoomIn, context),
        KeyBinding::new("cmd--", ZoomOut, context),
        KeyBinding::new("cmd-0", ZoomReset, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("cmd-y", Redo, context),
        KeyBinding::new("cmd-b", ToggleBold, context),
        KeyBinding::new("cmd-i", ToggleItalic, context),
        KeyBinding::new("cmd-u", ToggleUnderline, context),
        KeyBinding::new("cmd-e", ToggleInlineCode, context),
        KeyBinding::new("cmd-shift-x", ToggleStrikethrough, context),
        KeyBinding::new("cmd-\\", ClearFormatting, context),
        KeyBinding::new("cmd-shift-.", IncreaseFontSize, context),
        KeyBinding::new("cmd-shift-,", DecreaseFontSize, context),
        KeyBinding::new("cmd-alt-1", SetHeading1, context),
        KeyBinding::new("cmd-alt-2", SetHeading2, context),
        KeyBinding::new("cmd-alt-3", SetHeading3, context),
        KeyBinding::new("cmd-alt-0", SetParagraph, context),
        KeyBinding::new("cmd-shift-8", ToggleBulletList, context),
        KeyBinding::new("cmd-shift-7", ToggleOrderedList, context),
        KeyBinding::new("cmd-shift-9", ToggleChecklist, context),
        KeyBinding::new("cmd-shift-l", AlignLeft, context),
        KeyBinding::new("cmd-shift-e", AlignCenter, context),
        KeyBinding::new("cmd-shift-r", AlignRight, context),
        KeyBinding::new("cmd-shift-j", AlignJustify, context),
        KeyBinding::new("alt-left", MoveWordLeft, context),
        KeyBinding::new("alt-right", MoveWordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("cmd-left", MoveLineStart, context),
        KeyBinding::new("cmd-right", MoveLineEnd, context),
        KeyBinding::new("cmd-shift-left", SelectLineStart, context),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, context),
        KeyBinding::new("cmd-up", MoveDocStart, context),
        KeyBinding::new("cmd-down", MoveDocEnd, context),
        KeyBinding::new("cmd-shift-up", SelectDocStart, context),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, context),
        KeyBinding::new("alt-backspace", BackspaceWord, context),
        KeyBinding::new("alt-delete", DeleteWord, context),
    ]);

    #[cfg(not(target_os = "macos"))]
    bindings.extend([
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("ctrl-shift-v", PastePlainText, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("ctrl-=", ZoomIn, context),
        KeyBinding::new("ctrl--", ZoomOut, context),
        KeyBinding::new("ctrl-0", ZoomReset, context),
        KeyBinding::new("ctrl-z", Undo, context),
        KeyBinding::new("ctrl-shift-z", Redo, context),
        KeyBinding::new("ctrl-y", Redo, context),
        KeyBinding::new("ctrl-b", ToggleBold, context),
        KeyBinding::new("ctrl-i", ToggleItalic, context),
        KeyBinding::new("ctrl-u", ToggleUnderline, context),
        KeyBinding::new("ctrl-e", ToggleInlineCode, context),
        KeyBinding::new("ctrl-shift-x", ToggleStrikethrough, context),
        KeyBinding::new("ctrl-\\", ClearFormatting, context),
        KeyBinding::new("ctrl-shift-.", IncreaseFontSize, context),
        KeyBinding::new("ctrl-shift-,", DecreaseFontSize, context),
        KeyBinding::new("ctrl-alt-1", SetHeading1, context),
        KeyBinding::new("ctrl-alt-2", SetHeading2, context),
        KeyBinding::new("ctrl-alt-3", SetHeading3, context),
        KeyBinding::new("ctrl-alt-0", SetParagraph, context),
        KeyBinding::new("ctrl-shift-8", ToggleBulletList, context),
        KeyBinding::new("ctrl-shift-7", ToggleOrderedList, context),
        KeyBinding::new("ctrl-shift-9", ToggleChecklist, context),
        KeyBinding::new("ctrl-shift-l", AlignLeft, context),
        KeyBinding::new("ctrl-shift-e", AlignCenter, context),
        KeyBinding::new("ctrl-shift-r", AlignRight, context),
        KeyBinding::new("ctrl-shift-j", AlignJustify, context),
        KeyBinding::new("ctrl-left", MoveWordLeft, context),
        KeyBinding::new("ctrl-right", MoveWordRight, context),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
        KeyBinding::new("ctrl-home", MoveDocStart, context),
        KeyBinding::new("ctrl-end", MoveDocEnd, context),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, context),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, context),
        KeyBinding::new("ctrl-backspace", BackspaceWord, context),
        KeyBinding::new("ctrl-delete", DeleteWord, context),
    ]);

    cx.bind_keys(bindings);
}

/// Identifier returned when registering a [`CommandMiddleware`] on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MiddlewareId(u64);

/// Identifier returned when registering a [`DecorationProvider`] on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecorationProviderId(u64);

/// Ephemeral IME composition state held on [`RichDocumentView`] outside [`Document`] and [`History`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImePreedit {
    pub block_id: BlockId,
    /// UTF-8 byte range in `block_id` that will be replaced upon composition commit.
    pub replace_range: Range<usize>,
    /// Uncommitted composition text displayed with an inline underline.
    pub text: String,
    /// Selected byte range within [`Self::text`].
    pub selected_range: Range<usize>,
}

/// Identifies a selected inline image displaying drag-resize handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectedInlineImage {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
}

/// Corner handle of a selected inline image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub(crate) struct ImageResizeDrag {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub corner: ImageResizeCorner,
    pub start_canvas_point: Point,
    pub initial_width_pt: f32,
    pub initial_height_pt: f32,
    pub current_width_pt: f32,
    pub current_height_pt: f32,
}

/// Active floating popover anchored to a clicked [`rich_model::InlineObject::SmartChip`].
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSmartChipPopover {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: ChipKind,
    pub label: String,
    pub accent: Option<RgbaColor>,
}

pub(crate) struct TableColumnDrag {
    pub table_id: BlockId,
    pub column_index: usize,
    pub start_canvas_x: f32,
    pub initial_divider_x: f32,
    pub current_divider_x: f32,
    pub table_y: f32,
    pub table_height: f32,
    pub initial_widths_px: Vec<f32>,
}

/// Describes the active table cell/selection context for host toolbars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveTableContext {
    pub table_id: BlockId,
    pub row: usize,
    pub visual_col: usize,
    pub cell_index: usize,
    pub can_merge: bool,
    pub can_split: bool,
}

/// Host-observable events emitted by [`RichDocumentView`] via GPUI [`EventEmitter`].
#[derive(Clone, Debug, PartialEq)]
pub enum RichDocumentEvent {
    /// A hyperlink was activated by user click or programmatic [`RichCommand::OpenLink`].
    LinkActivated { href: String },
    /// A smart chip was clicked by the user.
    SmartChipActivated {
        block_id: BlockId,
        object_id: InlineObjectId,
        kind: ChipKind,
        label: String,
    },
    /// A code block's contents were copied via its built-in Copy button or [`RichCommand::CopyCodeBlock`].
    CodeBlockCopied { block_id: BlockId },
    /// The active selection or caret changed.
    SelectionChanged {
        selection: Option<DocumentSelection>,
    },
    /// A [`Transaction`] mutated the underlying [`Document`].
    DocumentMutated { change_set: ChangeSet },
    /// A [`RichCommand`] completed, no-oped, or was rejected/cancelled in the dispatcher.
    CommandExecuted {
        command_id: &'static str,
        effect: CommandEffectKind,
        resolution: CommandResolution,
        command: RichCommand,
        source: CommandSource,
        outcome: CommandOutcome,
    },
}

/// Interactive viewer and rich text editor host for a non-paginated document.
pub struct RichDocumentView {
    pub document: Document,
    pub history: History,
    pub flow: FlowLayout<GpuiGlyphBackend>,
    pub selection: Option<DocumentSelection>,
    pub focus_handle: FocusHandle,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub zoom: f32,
    pub debug_overlay: bool,
    policy: InteractionPolicy,
    pub theme: RichTheme,
    pub follow_app_theme: bool,
    pub open_external_links: bool,
    pub ime_preedit: Option<ImePreedit>,
    pub(crate) extensions: ExtensionRegistry,
    pub(crate) extension_diagnostics: Vec<ExtensionDiagnostic>,
    pub(crate) navigation: editor_navigation::NavigationState,
    pub(crate) pending_reveal: Option<DocPos>,
    pending_input_rule_undo: Option<editor_structure::InputRuleUndo>,
    pub(crate) font_resolver: Option<Box<FontResolver>>,
    pub(crate) decoration_providers: Vec<(DecorationProviderId, Box<dyn DecorationProvider>)>,
    pub(crate) last_viewport_bounds: Bounds<Pixels>,
    pub(crate) hovered_pointer: bool,
    pub(crate) caret_visible: bool,
    pub(crate) last_ime_anchor: Option<Option<(i32, i32, i32, i32)>>,
    pub(crate) copied_code_block: Option<BlockId>,
    pub(crate) selected_image: Option<SelectedInlineImage>,
    pub(crate) image_resize_drag: Option<ImageResizeDrag>,
    pub(crate) hovered_image_handle: Option<ImageResizeCorner>,
    pub(crate) active_chip_popover: Option<ActiveSmartChipPopover>,
    pub(crate) hovered_column_divider: Option<TableColumnDividerHit>,
    pub(crate) table_column_drag: Option<TableColumnDrag>,
    asset_provider: Option<Arc<dyn AssetProvider>>,
    asset_cache: HashMap<ImageAssetId, LoadedAsset>,
    asset_tasks: HashMap<ImageAssetId, Task<()>>,
    _copy_feedback_task: Task<()>,
    was_focused: bool,
    blink_epoch: usize,
    _blink_task: Task<()>,
    middleware: Vec<(MiddlewareId, Box<dyn CommandMiddleware>)>,
    next_middleware_id: u64,
    next_decoration_provider_id: u64,
    dragging: bool,
    drag_moved: bool,
    drag_origin: Option<Point>,
    drag_anchor_pos: Option<DocPos>,
    pending_link: Option<String>,
    pending_chip: Option<ActiveSmartChipPopover>,
    pending_footnote: Option<rich_model::AnchorId>,
}

impl EventEmitter<RichDocumentEvent> for RichDocumentView {}

impl RichDocumentView {
    /// Creates a new interactive viewer wrapping `document` with [`InteractionPolicy::viewer`]
    /// and [`RichTheme::light`].
    pub fn new(document: Document, cx: &mut Context<Self>) -> Self {
        Self {
            document,
            history: History::default(),
            flow: FlowLayout::default(),
            selection: None,
            focus_handle: cx.focus_handle(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            zoom: 1.0,
            debug_overlay: false,
            policy: InteractionPolicy::viewer(),
            theme: RichTheme::light(),
            follow_app_theme: false,
            open_external_links: true,
            ime_preedit: None,
            extensions: ExtensionRegistry::new(),
            extension_diagnostics: Vec::new(),
            navigation: editor_navigation::NavigationState::default(),
            pending_reveal: None,
            pending_input_rule_undo: None,
            font_resolver: None,
            decoration_providers: Vec::new(),
            last_viewport_bounds: Bounds::default(),
            hovered_pointer: false,
            caret_visible: true,
            last_ime_anchor: None,
            copied_code_block: None,
            selected_image: None,
            image_resize_drag: None,
            hovered_image_handle: None,
            active_chip_popover: None,
            hovered_column_divider: None,
            table_column_drag: None,
            asset_provider: None,
            asset_cache: HashMap::new(),
            asset_tasks: HashMap::new(),
            _copy_feedback_task: Task::ready(()),
            was_focused: false,
            blink_epoch: 0,
            _blink_task: Task::ready(()),
            middleware: Vec::new(),
            next_middleware_id: 1,
            next_decoration_provider_id: 1,
            dragging: false,
            drag_moved: false,
            drag_origin: None,
            drag_anchor_pos: None,
            pending_link: None,
            pending_chip: None,
            pending_footnote: None,
        }
    }

    /// Builder method configuring the initial [`InteractionPolicy`].
    pub fn with_policy(mut self, policy: InteractionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Returns the configured switches, including children currently masked by a parent.
    pub const fn policy(&self) -> InteractionPolicy {
        self.policy
    }

    /// Builder method configuring an explicit [`RichTheme`].
    pub fn with_theme(mut self, theme: RichTheme) -> Self {
        self.theme = theme;
        self.follow_app_theme = false;
        self
    }

    /// Builder method configuring an asynchronous [`AssetProvider`] for inline images.
    pub fn with_asset_provider(mut self, provider: impl AssetProvider) -> Self {
        self.asset_provider = Some(Arc::new(provider));
        self
    }

    /// Replaces or clears the [`AssetProvider`], clearing cached decoded images.
    pub fn set_asset_provider(
        &mut self,
        provider: Option<Arc<dyn AssetProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.asset_provider = provider;
        self.asset_cache.clear();
        self.asset_tasks.clear();
        cx.notify();
    }

    /// Evicts `asset_id` from the image cache so the next paint reloads it.
    pub fn invalidate_asset(&mut self, asset_id: &ImageAssetId, cx: &mut Context<Self>) {
        self.asset_cache.remove(asset_id);
        self.asset_tasks.remove(asset_id);
        cx.notify();
    }

    /// Returns the current cached load state for `asset_id`, if any.
    pub fn asset_state(&self, asset_id: &ImageAssetId) -> Option<&LoadedAsset> {
        self.asset_cache.get(asset_id)
    }

    /// Clears all cached decoded images.
    pub fn clear_asset_cache(&mut self, cx: &mut Context<Self>) {
        self.asset_cache.clear();
        self.asset_tasks.clear();
        cx.notify();
    }

    /// Returns the cached [`LoadedAsset`] state for `asset_id`, kicking off an async background
    /// fetch + decode task when an [`AssetProvider`] is configured and `asset_id` is uncached.
    pub(crate) fn resolve_or_request_image(
        &mut self,
        asset_id: &ImageAssetId,
        cx: &mut Context<Self>,
    ) -> Option<LoadedAsset> {
        if let Some(existing) = self.asset_cache.get(asset_id) {
            return Some(existing.clone());
        }
        let provider = self.asset_provider.as_ref()?.clone();
        let key = asset_id.clone();
        self.asset_cache.insert(key.clone(), LoadedAsset::Loading);
        let svg_renderer = cx.svg_renderer();
        let load_future = provider.load_image(asset_id);
        let key_for_decode = key.clone();
        let key_for_task = key.clone();
        let task = cx.spawn(async move |weak_view, cx| {
            let decoded_result = cx
                .background_executor()
                .spawn(async move {
                    let raw = load_future.await?;
                    decode_image_asset(&key_for_decode, raw, svg_renderer)
                })
                .await;
            if let Err(error) = weak_view.update(cx, |view, cx| {
                view.asset_tasks.remove(&key_for_task);
                match decoded_result {
                    Ok(image) => {
                        view.asset_cache
                            .insert(key_for_task, LoadedAsset::Ready(image));
                    }
                    Err(error) => {
                        view.asset_cache
                            .insert(key_for_task, LoadedAsset::Failed(error));
                    }
                }
                cx.notify();
            }) {
                // View was dropped before the async image load completed.
                let _ = error;
            }
        });
        self.asset_tasks.insert(key, task);
        Some(LoadedAsset::Loading)
    }

    /// Dismisses any open smart chip popover.
    pub fn dismiss_chip_popover(&mut self, cx: &mut Context<Self>) {
        if self.active_chip_popover.take().is_some() {
            cx.notify();
        }
    }

    /// Builds the GPUI [`ClipboardItem`] for the active selection, carrying plain text plus
    /// HTML and versioned structured fragment metadata for same-process rich paste.
    pub fn clipboard_item_for_selection(&self) -> Option<ClipboardItem> {
        let fragment = self.selected_fragment()?;
        let formatter = editor_clipboard::ComponentClipboardFormatter::new(self);
        let plain_text = fragment.to_plain_text_with(&formatter);
        let html = fragment.to_html_with(&formatter);
        let mut structured = fragment;
        editor_clipboard::attach_component_definitions(
            &self.document.components,
            &mut structured,
        );
        let mut metadata_map = serde_json::Map::new();
        metadata_map.insert(
            CLIPBOARD_HTML_METADATA_KEY.to_owned(),
            serde_json::Value::String(html),
        );
        if let Ok(fragment_json) = structured.to_versioned_json() {
            metadata_map.insert(
                CLIPBOARD_FRAGMENT_METADATA_KEY.to_owned(),
                serde_json::Value::String(fragment_json),
            );
        }
        let metadata = serde_json::Value::Object(metadata_map).to_string();
        Some(ClipboardItem::new_string_with_metadata(
            plain_text, metadata,
        ))
    }

    /// Pastes a GPUI [`ClipboardItem`] through the user command dispatch path.
    ///
    /// When `plain_text_only` is `true` (or rich paste is disabled by policy), private clipboard
    /// metadata is ignored and only plain text is dispatched.
    pub fn paste_clipboard_item(
        &mut self,
        item: &ClipboardItem,
        plain_text_only: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let plain_text = item.text();
        if !plain_text_only && self.policy.can_paste_rich_text() {
            let html = extract_clipboard_html(item);
            if let Some(fragment) = extract_clipboard_fragment(item) {
                return self.dispatch_command(
                    RichCommand::PasteFragment {
                        fragment,
                        html_fallback: html,
                        plain_text,
                    },
                    cx,
                );
            }
            if let Some(html) = html {
                return self.dispatch_command(RichCommand::PasteHtml { html, plain_text }, cx);
            }
        }
        if let Some(text) = plain_text {
            self.dispatch_command(RichCommand::PasteText(text), cx)
        } else {
            CommandOutcome::NoOp
        }
    }

    /// Encodes the current [`Document`] using a format-typed [`DocumentCodec`].
    pub fn export_document<C: DocumentCodec>(
        &self,
        codec: &C,
    ) -> Result<EncodedDocument<C::Format>, CodecError> {
        codec.encode(&self.document)
    }

    /// Encodes the current [`Document`] using a dynamically looked-up [`DynamicDocumentCodec`].
    pub fn export_document_dynamic(
        &self,
        codec: &dyn DynamicDocumentCodec,
    ) -> Result<String, CodecError> {
        codec.encode_raw(&self.document)
    }

    /// Replaces the displayed [`Document`], resetting selection, undo history, and layout cache.
    pub fn replace_document(&mut self, document: Document, cx: &mut Context<Self>) {
        self.document = document;
        self.history = History::default();
        self.flow.cache.clear();
        self.extensions.clear_resolved_caches();
        self.extension_diagnostics.clear();
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.ime_preedit = None;
        self.navigation = editor_navigation::NavigationState::default();
        self.pending_reveal = None;
        self.pending_input_rule_undo = None;
        self.selected_image = None;
        self.image_resize_drag = None;
        self.active_chip_popover = None;
        self.table_column_drag = None;
        self.update_selection(None, cx);
        self.resync_layout_with_anchor(None);
        cx.notify();
    }

    /// Applies an incoming CRDT [`SyncDelta`] through `bridge` without recording into local undo
    /// history or re-broadcasting to remote peers. Returns true when the document changed.
    pub fn apply_remote_sync_delta(
        &mut self,
        bridge: &mut CrdtSyncBridge,
        delta: &SyncDelta,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(transaction) = bridge.apply_remote_delta(delta, &self.document) else {
            return false;
        };
        bridge.set_applying_remote(true);
        let change_set = self.apply_transaction(transaction, false, cx);
        bridge.set_applying_remote(false);
        !change_set.dirty_blocks.is_empty() || change_set.structure_changed
    }

    /// Returns metadata describing the active table cell or selection, if any.
    pub fn active_table_context(&self) -> Option<ActiveTableContext> {
        match self.selection.as_ref()? {
            DocumentSelection::TableCells {
                table_id,
                anchor_cell,
                focus_cell,
            } => {
                let table = self.document.find_block(*table_id)?.kind.table_block()?;
                let (_, placements, occupancy) = table.resolve_grid();
                let r_min = anchor_cell.0.min(focus_cell.0);
                let c_min = anchor_cell.1.min(focus_cell.1);
                let placement_idx = occupancy
                    .get(r_min)
                    .and_then(|row| row.get(c_min))
                    .and_then(|slot| *slot)?;
                let placement = placements.get(placement_idx)?;
                Some(ActiveTableContext {
                    table_id: *table_id,
                    row: placement.row_index,
                    visual_col: placement.visual_col,
                    cell_index: placement.cell_index,
                    can_merge: anchor_cell != focus_cell,
                    can_split: placement.colspan > 1 || placement.rowspan > 1,
                })
            }
            DocumentSelection::Text { focus, .. } => {
                let (table_id, row, cell_index) = table_cell_of(focus)?;
                let table = self.document.find_block(table_id)?.kind.table_block()?;
                let placement = table.placement_for_stored_cell(row, cell_index)?;
                Some(ActiveTableContext {
                    table_id,
                    row: placement.row_index,
                    visual_col: placement.visual_col,
                    cell_index: placement.cell_index,
                    can_merge: false,
                    can_split: placement.colspan > 1 || placement.rowspan > 1,
                })
            }
        }
    }

    /// Builder method configuring a render-time [`FontResolver`] for mapping `(font_family, weight, italic)`
    /// into custom GPUI [`Font`]s without mutating the underlying [`Document`].
    pub fn with_font_resolver(
        mut self,
        resolver: impl Fn(&ShaperRunSpec, &Window) -> Option<Font> + 'static,
    ) -> Self {
        self.font_resolver = Some(Box::new(resolver));
        self
    }

    /// Replaces or clears the render-time [`FontResolver`], invalidating cached block layouts.
    pub fn set_font_resolver(
        &mut self,
        resolver: Option<Box<FontResolver>>,
        cx: &mut Context<Self>,
    ) {
        self.font_resolver = resolver;
        self.flow.cache.clear();
        cx.notify();
    }

    /// Builder method enabling or disabling live synchronization with [`gpui_kit::base::Theme`].
    pub fn with_follow_app_theme(mut self, follow: bool) -> Self {
        self.follow_app_theme = follow;
        self
    }

    /// Builder method attaching a [`CommandMiddleware`] hook.
    pub fn with_middleware(mut self, middleware: impl CommandMiddleware + 'static) -> Self {
        self.add_middleware(middleware);
        self
    }

    /// Registers a [`CommandMiddleware`] hook executed in registration order.
    pub fn add_middleware(&mut self, middleware: impl CommandMiddleware + 'static) -> MiddlewareId {
        let id = MiddlewareId(self.next_middleware_id);
        self.next_middleware_id = self.next_middleware_id.wrapping_add(1);
        self.middleware.push((id, Box::new(middleware)));
        id
    }

    /// Removes a previously registered [`CommandMiddleware`] by its [`MiddlewareId`].
    pub fn remove_middleware(&mut self, id: MiddlewareId) -> bool {
        let previous_len = self.middleware.len();
        self.middleware.retain(|(entry_id, _)| *entry_id != id);
        self.middleware.len() != previous_len
    }

    /// Builder method attaching an ephemeral [`DecorationProvider`].
    pub fn with_decoration_provider(
        mut self,
        provider: impl DecorationProvider + 'static,
    ) -> Self {
        self.add_decoration_provider(provider);
        self
    }

    /// Registers an ephemeral [`DecorationProvider`] feeding paint Layers 2, 4, and 5.
    pub fn add_decoration_provider(
        &mut self,
        provider: impl DecorationProvider + 'static,
    ) -> DecorationProviderId {
        let id = DecorationProviderId(self.next_decoration_provider_id);
        self.next_decoration_provider_id = self.next_decoration_provider_id.wrapping_add(1);
        self.decoration_providers.push((id, Box::new(provider)));
        id
    }

    /// Removes a previously registered [`DecorationProvider`].
    pub fn remove_decoration_provider(&mut self, id: DecorationProviderId) -> bool {
        let previous_len = self.decoration_providers.len();
        self.decoration_providers
            .retain(|(entry_id, _)| *entry_id != id);
        self.decoration_providers.len() != previous_len
    }

    /// Schedules a repaint so dynamic [`DecorationProvider`]s re-evaluate visible blocks.
    pub fn invalidate_decorations(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Borrows the view's [`ExtensionRegistry`] immutably.
    ///
    /// Mutations must go through [`Self::register_block_renderer`],
    /// [`Self::register_inline_renderer`], and related methods so layout and payload caches are
    /// invalidated consistently.
    pub fn extensions(&self) -> &ExtensionRegistry {
        &self.extensions
    }

    /// Returns validation diagnostics collected for visible registered custom nodes on the most
    /// recent prepaint pass.
    pub fn extension_diagnostics(&self) -> &[ExtensionDiagnostic] {
        &self.extension_diagnostics
    }

    /// Registers a spec-bound [`BlockRenderer`], invalidates cached layouts, and schedules repaint.
    pub fn register_block_renderer<R: BlockRenderer>(
        &mut self,
        renderer: R,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.extensions.register_block(renderer)?;
        self.extensions.clear_resolved_caches();
        self.extension_diagnostics.clear();
        self.flow.cache.clear();
        cx.notify();
        Ok(())
    }

    /// Registers a spec-bound [`InlineRenderer`], invalidates cached layouts, and schedules repaint.
    pub fn register_inline_renderer<R: InlineRenderer>(
        &mut self,
        renderer: R,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.extensions.register_inline(renderer)?;
        self.extensions.clear_resolved_caches();
        self.extension_diagnostics.clear();
        self.flow.cache.clear();
        cx.notify();
        Ok(())
    }

    /// Unregisters the [`BlockRenderer`] for [`BlockSpec`] `S` and invalidates cached layouts.
    pub fn unregister_block_renderer<S: BlockSpec>(&mut self, cx: &mut Context<Self>) -> bool {
        let removed = self.extensions.unregister_block::<S>();
        if removed {
            self.extensions.clear_resolved_caches();
            self.extension_diagnostics.clear();
            self.flow.cache.clear();
            cx.notify();
        }
        removed
    }

    /// Unregisters the [`InlineRenderer`] for [`InlineSpec`] `S` and invalidates cached layouts.
    pub fn unregister_inline_renderer<S: InlineSpec>(&mut self, cx: &mut Context<Self>) -> bool {
        let removed = self.extensions.unregister_inline::<S>();
        if removed {
            self.extensions.clear_resolved_caches();
            self.extension_diagnostics.clear();
            self.flow.cache.clear();
            cx.notify();
        }
        removed
    }

    /// Registers a handler for a [`ReadOnly`] [`ExtensionCommand`].
    pub fn register_read_command<C>(
        &mut self,
        handler: impl Fn(&C, &ReadCommandContext<'_>) -> ReadCommandOutcome + 'static,
    ) -> Result<(), ExtensionRegistrationError>
    where
        C: ExtensionCommand<Effect = ReadOnly>,
    {
        self.extensions.register_read_command::<C>(handler)
    }

    /// Registers a transaction planner for an [`Edit`] [`ExtensionCommand`].
    pub fn register_edit_command<C>(
        &mut self,
        handler: impl Fn(&C, &mut EditCommandContext<'_>) -> Option<Transaction> + 'static,
    ) -> Result<(), ExtensionRegistrationError>
    where
        C: ExtensionCommand<Effect = Edit>,
    {
        self.extensions.register_edit_command::<C>(handler)
    }

    /// Unregisters a typed [`ExtensionCommand`] handler.
    pub fn unregister_command<C: ExtensionCommand>(&mut self) -> bool {
        self.extensions.unregister_command::<C>()
    }

    /// Dispatches a user-initiated typed [`ExtensionCommand`] through middleware, policy, and events.
    pub fn dispatch_extension_command<C: ExtensionCommand>(
        &mut self,
        command: C,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.dispatch_command(RichCommand::extension(command), cx)
    }

    /// Dispatches a programmatic typed [`ExtensionCommand`], bypassing user-only policy gates.
    pub fn dispatch_programmatic_extension_command<C: ExtensionCommand>(
        &mut self,
        command: C,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.dispatch_programmatic_command(RichCommand::extension(command), cx)
    }

    /// Applies feature switches and cancels transient interactions that are no longer enabled.
    pub fn set_policy(&mut self, policy: InteractionPolicy, cx: &mut Context<Self>) {
        if self.policy == policy {
            return;
        }
        self.policy = policy;
        if !policy.editing.enabled {
            self.ime_preedit = None;
        }
        if !policy.can_apply_input_rules() {
            self.pending_input_rule_undo = None;
        }
        if !policy.can_resize_images() {
            self.selected_image = None;
            self.image_resize_drag = None;
            self.hovered_image_handle = None;
        }
        if !policy.can_resize_table_columns() {
            self.table_column_drag = None;
            self.hovered_column_divider = None;
        }
        if !policy.can_activate_smart_chips() {
            self.pending_chip = None;
            self.active_chip_popover = None;
        }
        if !policy.can_open_links() {
            self.pending_link = None;
        }
        if !policy.can_copy_code() {
            self.copied_code_block = None;
        }
        if !policy.can_select() {
            self.ime_preedit = None;
            self.dragging = false;
            self.drag_anchor_pos = None;
            self.pending_footnote = None;
            self.update_selection(None, cx);
        }
        self.hovered_pointer = false;
        if policy.should_show_caret() && self.was_focused {
            self.reset_caret_blink(cx);
        } else if !policy.should_show_caret() {
            self.stop_caret_blink();
        }
        cx.notify();
    }

    /// Applies an explicit [`RichTheme`] and disables automatic `gpui-kit` theme syncing.
    pub fn set_theme(&mut self, theme: RichTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        self.follow_app_theme = false;
        cx.notify();
    }

    /// Enables or disables live synchronization with [`gpui_kit::base::Theme`].
    pub fn set_follow_app_theme(&mut self, follow: bool, cx: &mut Context<Self>) {
        if self.follow_app_theme == follow {
            return;
        }
        self.follow_app_theme = follow;
        cx.notify();
    }

    /// Copies the current [`gpui_kit::base::Theme`] global into [`Self::theme`].
    pub fn sync_theme_from_gpui(&mut self, cx: &mut Context<Self>) {
        self.theme = RichTheme::from_app(cx);
        cx.notify();
    }

    /// Resolves the active [`RichTheme`] for painting, honoring [`Self::follow_app_theme`].
    pub fn active_theme(&self, cx: &App) -> RichTheme {
        if self.follow_app_theme {
            RichTheme::from_app(cx)
        } else {
            self.theme.clone()
        }
    }

    /// Tracks focus transitions from `RichDocumentElement::prepaint` to drive caret blinking
    /// and clean up ephemeral IME composition state on blur.
    pub(crate) fn sync_focus_state(&mut self, focused: bool, cx: &mut Context<Self>) {
        if focused == self.was_focused {
            return;
        }
        self.was_focused = focused;
        if focused {
            if self.policy.should_show_caret() {
                self.reset_caret_blink(cx);
            }
        } else {
            self.ime_preedit = None;
            self.last_ime_anchor = None;
            self.stop_caret_blink();
        }
    }

    /// Resets the caret to visible and restarts the blink cadence according to `gpui_kit::base::Theme`.
    pub fn reset_caret_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        self.caret_visible = true;
        if !self.policy.should_show_caret() {
            self._blink_task = Task::ready(());
            return;
        }

        let blink_policy = gpui_kit::base::Theme::global(cx).cursor_blink;
        let Some(interval) = blink_policy
            .interval()
            .filter(|duration| !duration.is_zero())
        else {
            self._blink_task = Task::ready(());
            return;
        };
        let idle_timeout = blink_policy
            .idle_timeout()
            .filter(|duration| !duration.is_zero());
        let epoch = self.blink_epoch;
        let started_at = cx.background_executor().now();

        self._blink_task = cx.spawn(async move |weak_view, cx| {
            let mut next_delay = CARET_BLINK_PAUSE_DELAY;
            loop {
                cx.background_executor().timer(next_delay).await;
                let keep_running = weak_view
                    .update(cx, |view, cx| {
                        if view.blink_epoch != epoch
                            || !view.was_focused
                            || !view.policy.should_show_caret()
                        {
                            return false;
                        }
                        if let Some(timeout) = idle_timeout
                            && cx.background_executor().now().saturating_duration_since(started_at)
                                >= timeout
                        {
                            view.caret_visible = true;
                            cx.notify();
                            return false;
                        }
                        view.caret_visible = !view.caret_visible;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
                next_delay = interval;
            }
        });
    }

    fn stop_caret_blink(&mut self) {
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        self._blink_task = Task::ready(());
        self.caret_visible = false;
    }

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
                    current_command = replacement;
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
                | RichCommand::Delete { direction: MoveDirection::Backward, unit: MoveUnit::Grapheme }
        ) {
            self.pending_input_rule_undo = None;
        }
        let outcome = self.execute_command(&current_command, source, cx);
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
        if let Some(selection) = next_selection {
            self.update_selection(Some(selection), cx);
        }

        self.resync_layout_with_anchor(anchor);
        self.notify_transaction_applied(applied.change_set.clone(), cx);
        self.reset_caret_blink(cx);
        self.ensure_caret_visible();
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
                if self.update_selection(selection.clone(), cx) {
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
            RichCommand::Component(command) => editor_components::execute_component_command(self, command, cx),
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
                    MoveDirection::Backward | MoveDirection::Up => rich_model::NavigationTarget::PreviousParagraph,
                    MoveDirection::Forward | MoveDirection::Down => rich_model::NavigationTarget::NextParagraph,
                };
                let Some(position) = rich_model::navigate_target(&self.document, &target, Some(&focus)) else {
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

    fn execute_insert_text_inner(&mut self, raw_text: &str, cx: &mut Context<Self>) -> CommandOutcome {
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
        stored_marks: Option<rich_model::InlineStyle>,
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
            let new_block = rich_model::BlockNode::new(
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
            return self.execute_grid_command(&rich_model::GridCommand::ClearSelection, cx);
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
                    let Some(transaction) = self
                        .document
                        .build_merge_with_previous_transaction(&focus, selection_before)
                    else {
                        return CommandOutcome::NoOp;
                    };
                    self.apply_transaction_grouped(
                        transaction,
                        Some(HistoryMergeGroup::Atomic),
                        cx,
                    );
                    return CommandOutcome::Executed;
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
                    let Some(transaction) = self
                        .document
                        .build_merge_with_next_transaction(&focus, selection_before)
                    else {
                        return CommandOutcome::NoOp;
                    };
                    self.apply_transaction_grouped(
                        transaction,
                        Some(HistoryMergeGroup::Atomic),
                        cx,
                    );
                    return CommandOutcome::Executed;
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

    fn execute_split_block(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some((anchor, focus, _)) = self.active_text_selection() else {
            return CommandOutcome::NoOp;
        };
        let selection_before = self.selection.clone();

        if anchor == focus {
            let Some(transaction) = self
                .document
                .build_split_block_transaction(&focus, selection_before)
            else {
                return CommandOutcome::NoOp;
            };
            self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
            return CommandOutcome::Executed;
        }

        // Delete active selection then split, combining both into one atomic undo entry.
        let anchor_scroll = self.flow.capture_anchor(self.scroll_y);
        let Some((delete_steps, caret_pos)) = self.document.delete_selection_steps(&anchor, &focus)
        else {
            return CommandOutcome::NoOp;
        };
        let applied_del = self.document.apply(Transaction::new(delete_steps));
        let Some(split_tx) = self
            .document
            .build_split_block_transaction(&caret_pos, None)
        else {
            return CommandOutcome::NoOp;
        };
        let next_selection = split_tx.selection_after.clone();
        let applied_split = self.document.apply(split_tx);

        let mut combined_inverse = applied_split.inverse.steps;
        combined_inverse.extend(applied_del.inverse.steps);
        let mut combined_change_set = applied_del.change_set;
        combined_change_set.structure_changed |= applied_split.change_set.structure_changed;
        for dirty in applied_split.change_set.dirty_blocks {
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
                BlockKind::list_item(
                    item.list_id,
                    item.kind,
                    next_level,
                    item.checked,
                    content,
                ),
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
                let mut style = BlockStyle::default().with_indent(indent, 0.0).with_align(align);
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

        let component_step = editor_clipboard::import_component_definitions(
            &self.document.components, &mut fragment,
        ).map(|components| Step::SetComponents { components });
        let rectangle = matches!(self.selection, Some(DocumentSelection::TableCells { .. }));
        let in_table = rectangle || self.selection.as_ref().is_some_and(|selection| match selection {
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
            return self.execute_grid_command(&rich_model::GridCommand::PasteTsv { text: plain }, cx);
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
            let first_inline = first_pasted.kind.inline_content().cloned().unwrap_or_default();
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
                            tail_id, BlockKind::paragraph(trailing_after_caret), BlockStyle::default(),
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
            CommandExecutionResult::Read(ReadCommandOutcome::Executed) => {
                cx.notify();
                CommandOutcome::Executed
            }
            CommandExecutionResult::Read(ReadCommandOutcome::NoOp) => CommandOutcome::NoOp,
            CommandExecutionResult::Edit {
                transaction: Some(transaction),
                next_block_id,
            } => {
                if transaction.steps.is_empty() {
                    return CommandOutcome::NoOp;
                }
                self.document.reserve_block_ids_up_to(next_block_id);
                let change_set = self.apply_transaction(transaction, true, cx);
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

    fn execute_set_table_columns(
        &mut self,
        table_id: BlockId,
        columns: Vec<ColumnSpec>,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        let Some(transaction) = self.document.build_set_table_columns_transaction(
            table_id,
            columns,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_insert_table_row(&mut self, after: bool, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let target_row = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                if after {
                    anchor_cell.0.max(focus_cell.0)
                } else {
                    anchor_cell.0.min(focus_cell.0)
                }
            }
            _ => context.row,
        };
        let Some(transaction) = self.document.build_insert_table_row_transaction(
            context.table_id,
            target_row,
            after,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_delete_table_row(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let row_range = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                let start = anchor_cell.0.min(focus_cell.0);
                let end = anchor_cell.0.max(focus_cell.0) + 1;
                start..end
            }
            _ => context.row..(context.row + 1),
        };
        let Some(transaction) = self.document.build_delete_table_row_transaction(
            context.table_id,
            row_range,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_insert_table_column(
        &mut self,
        after: bool,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let target_col = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                if after {
                    anchor_cell.1.max(focus_cell.1)
                } else {
                    anchor_cell.1.min(focus_cell.1)
                }
            }
            _ => context.visual_col,
        };
        let Some(transaction) = self.document.build_insert_table_column_transaction(
            context.table_id,
            target_col,
            after,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_delete_table_column(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let col_range = match self.selection.as_ref() {
            Some(DocumentSelection::TableCells {
                anchor_cell,
                focus_cell,
                ..
            }) => {
                let start = anchor_cell.1.min(focus_cell.1);
                let end = anchor_cell.1.max(focus_cell.1) + 1;
                start..end
            }
            _ => context.visual_col..(context.visual_col + 1),
        };
        let Some(transaction) = self.document.build_delete_table_column_transaction(
            context.table_id,
            col_range,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_merge_table_cells(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(DocumentSelection::TableCells {
            table_id,
            anchor_cell,
            focus_cell,
        }) = self.selection.clone()
        else {
            return CommandOutcome::NoOp;
        };
        let Some(transaction) = self.document.build_merge_table_cells_transaction(
            table_id,
            anchor_cell,
            focus_cell,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_split_table_cell(&mut self, cx: &mut Context<Self>) -> CommandOutcome {
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        let Some(transaction) = self.document.build_split_table_cell_transaction(
            context.table_id,
            context.row,
            context.cell_index,
            self.selection.clone(),
        ) else {
            return CommandOutcome::NoOp;
        };
        self.apply_transaction_grouped(transaction, Some(HistoryMergeGroup::Atomic), cx);
        CommandOutcome::Executed
    }

    fn execute_navigate_table_cell(
        &mut self,
        forward: bool,
        source: CommandSource,
        cx: &mut Context<Self>,
    ) -> CommandOutcome {
        if source == CommandSource::User && !self.policy.can_select() {
            return CommandOutcome::RejectedByPolicy;
        }
        self.ime_preedit = None;
        self.history.break_coalescing();
        let Some(context) = self.active_table_context() else {
            return CommandOutcome::NoOp;
        };
        match self
            .document
            .next_table_cell_pos(context.table_id, context.row, context.cell_index, forward)
        {
            Some(rich_model::TableCellStep::MoveTo(pos)) => {
                let next_selection = DocumentSelection::caret(pos, Affinity::Downstream);
                self.update_selection(Some(next_selection), cx);
                self.reset_caret_blink(cx);
                self.ensure_caret_visible();
                CommandOutcome::Executed
            }
            Some(rich_model::TableCellStep::AtBoundary)
                if forward
                    && self.policy.editing.enabled
                    && (source == CommandSource::Programmatic
                        || self.policy.can_edit_table_structure()) =>
            {
                // Tab past the final cell appends a new row below and focuses its first cell.
                self.execute_insert_table_row(true, cx)
            }
            _ => CommandOutcome::NoOp,
        }
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
    ) -> Option<rich_model::ListId> {
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
        let base_style = block
            .style
            .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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
            let base_style = block
                .style
                .resolved_base_text(&self.document.stylesheet, &self.document.config.default_style);
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

    fn active_text_selection(
        &self,
    ) -> Option<(DocPos, DocPos, Option<rich_model::InlineStyle>)> {
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

    fn first_pos_in_table_cell(&self, table_id: BlockId, cell_coords: (usize, usize)) -> Option<DocPos> {
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
                    let current_index = visible_leaves.iter().position(|block| block.id == leaf_id)?;
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
                    let current_index = visible_leaves.iter().position(|block| block.id == leaf_id)?;
                    let next_leaf = visible_leaves.get(current_index + 1)?;
                    self.document.doc_pos_for_block(next_leaf.id, 0)
                }
            }
        }
    }

    fn ensure_caret_visible(&mut self) {
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

    fn update_selection(
        &mut self,
        next_selection: Option<DocumentSelection>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.selection == next_selection {
            return false;
        }
        self.pending_reveal = None;
        self.selection = next_selection;
        cx.emit(RichDocumentEvent::SelectionChanged {
            selection: self.selection.clone(),
        });
        cx.notify();
        true
    }

    fn notify_transaction_applied(&mut self, change_set: ChangeSet, cx: &mut Context<Self>) {
        self.refresh_search_matches();
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
        Some(fragment.to_plain_text_with(&editor_clipboard::ComponentClipboardFormatter::new(self)))
    }

    /// Extracts normalized semantic HTML (`text/html`) covering the current selection.
    pub fn selected_html(&self) -> Option<String> {
        let fragment = self.selected_fragment()?;
        Some(fragment.to_html_with(&editor_clipboard::ComponentClipboardFormatter::new(self)))
    }

    fn apply_zoom(&mut self, target_zoom: f32, cx: &mut Context<Self>) -> CommandOutcome {
        let clamped = target_zoom.clamp(0.5, 2.0);
        if (clamped - self.zoom).abs() < 0.001 {
            return CommandOutcome::NoOp;
        }
        let anchor = self.flow.capture_anchor(self.scroll_y);
        self.zoom = clamped;
        self.resync_layout_with_anchor(anchor);
        cx.notify();
        CommandOutcome::Executed
    }

    fn resync_layout_with_anchor(&mut self, anchor: Option<rich_layout::ScrollAnchor>) {
        let viewport_width: f32 = self.last_viewport_bounds.size.width.into();
        let viewport_height: f32 = self.last_viewport_bounds.size.height.into();
        if viewport_width <= 1.0 {
            return;
        }
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
        self.dispatch_command(RichCommand::ScrollBy { delta_x, delta_y }, cx);
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
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

        self.pending_footnote = clicked_object.as_ref().and_then(|(_, _, object)| {
            rich_model::footnote_reference_id(object)
        });
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
                    && kind.as_ref() == rich_model::DROPDOWN_KIND =>
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
        self.dispatch_command(RichCommand::SetSelection(Some(next_selection)), cx);
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

            let next_divider = if self.policy.can_resize_table_columns()
                && next_image_handle.is_none()
            {
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
                        InlineObject::Custom { kind, .. } => kind.as_ref() == rich_model::DROPDOWN_KIND,
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
            self.dispatch_command(RichCommand::SetSelection(Some(next_selection)), cx);
        }
    }

    fn handle_mouse_up(
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
        if let Some(id) = self.pending_footnote.take() && !self.drag_moved {
            self.dispatch_command(RichCommand::Anchor(rich_model::AnchorCommand::RevealFootnote { id }), cx);
        }
        self.drag_moved = false;
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
        let preedit_end_committed = preedit.replace_range.end.min(committed_len).max(preedit_start);
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
                    existing_preedit
                        .as_ref()
                        .filter(|p| p.block_id == block_id),
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
        if new_text.is_empty() && self.selection.as_ref().is_none_or(DocumentSelection::is_collapsed)
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
            size(px(canvas_rect.width.max(2.0)), px(canvas_rect.height.max(14.0))),
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
                RichCommand::SetSelection(Some(DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: None,
                })),
                cx,
            );
        }
    }

    fn text_length_utf16(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
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

fn previous_grapheme_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    text.get(..clamped)
        .and_then(|prefix| prefix.grapheme_indices(true).next_back())
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    text.get(clamped..)
        .and_then(|suffix| suffix.graphemes(true).next())
        .map(|grapheme| clamped + grapheme.len())
        .unwrap_or(text.len())
}

fn previous_word_boundary(text: &str, offset: usize) -> usize {
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

fn next_word_boundary(text: &str, offset: usize) -> usize {
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

fn is_external_uri(href: &str) -> bool {
    href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:")
}

fn table_cell_of(pos: &DocPos) -> Option<(BlockId, usize, usize)> {
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
    if let (Some((anchor_table, anchor_row, anchor_cell_idx)), Some((focus_table, focus_row, focus_cell_idx))) =
        (table_cell_of(&anchor), table_cell_of(&focus))
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

fn extract_clipboard_html(item: &ClipboardItem) -> Option<String> {
    let raw_metadata = item.metadata()?;
    let parsed: serde_json::Value = serde_json::from_str(raw_metadata).ok()?;
    parsed
        .get(CLIPBOARD_HTML_METADATA_KEY)
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn extract_clipboard_fragment(item: &ClipboardItem) -> Option<ClipboardFragment> {
    let raw_metadata = item.metadata()?;
    let parsed: serde_json::Value = serde_json::from_str(raw_metadata).ok()?;
    let encoded = parsed
        .get(CLIPBOARD_FRAGMENT_METADATA_KEY)
        .and_then(|value| value.as_str())?;
    ClipboardFragment::from_versioned_json(encoded).ok()
}

fn last_text_pos_in_nodes(nodes: &[BlockNode]) -> Option<(BlockId, usize)> {
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

impl Focusable for RichDocumentView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RichDocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(RICH_DOCUMENT_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Copy, _window, cx| {
                this.dispatch_command(RichCommand::CopySelection, cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, _window, cx| {
                if !this.policy.editing.enabled {
                    return;
                }
                if this.dispatch_command(RichCommand::CopySelection, cx) == CommandOutcome::Executed
                {
                    this.dispatch_command(
                        RichCommand::Delete {
                            direction: MoveDirection::Backward,
                            unit: MoveUnit::Grapheme,
                        },
                        cx,
                    );
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| {
                if let Some(item) = cx.read_from_clipboard() {
                    this.paste_clipboard_item(&item, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PastePlainText, _window, cx| {
                if let Some(item) = cx.read_from_clipboard() {
                    this.paste_clipboard_item(&item, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| {
                this.dispatch_command(RichCommand::SelectAll, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _window, cx| {
                this.dispatch_command(RichCommand::ClearSelection, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _window, cx| {
                this.dispatch_command(RichCommand::AdjustZoom(0.1), cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _window, cx| {
                this.dispatch_command(RichCommand::AdjustZoom(-0.1), cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, _window, cx| {
                this.dispatch_command(RichCommand::ResetZoom, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleDebugOverlay, _window, cx| {
                this.dispatch_command(RichCommand::ToggleDebugOverlay, cx);
            }))
            .on_action(cx.listener(|this, _: &Undo, _window, cx| {
                this.dispatch_command(RichCommand::Undo, cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, _window, cx| {
                this.dispatch_command(RichCommand::Redo, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveLeft, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Grapheme,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveRight, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Grapheme,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveUp, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Up,
                        unit: MoveUnit::Line,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Down,
                        unit: MoveUnit::Line,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Grapheme,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Grapheme,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectUp, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Up,
                        unit: MoveUnit::Line,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectDown, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Down,
                        unit: MoveUnit::Line,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveWordLeft, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Word,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveWordRight, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Word,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectWordLeft, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Word,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectWordRight, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Word,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveLineStart, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::LineBoundary,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveLineEnd, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::LineBoundary,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectLineStart, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::LineBoundary,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectLineEnd, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::LineBoundary,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveDocStart, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::DocumentBoundary,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MoveDocEnd, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::DocumentBoundary,
                        extend_selection: false,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectDocStart, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::DocumentBoundary,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SelectDocEnd, _window, cx| {
                this.dispatch_command(
                    RichCommand::MoveCaret {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::DocumentBoundary,
                        extend_selection: true,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| {
                this.dispatch_command(
                    RichCommand::Delete {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Grapheme,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &Delete, _window, cx| {
                this.dispatch_command(
                    RichCommand::Delete {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Grapheme,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &BackspaceWord, _window, cx| {
                this.dispatch_command(
                    RichCommand::Delete {
                        direction: MoveDirection::Backward,
                        unit: MoveUnit::Word,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &DeleteWord, _window, cx| {
                this.dispatch_command(
                    RichCommand::Delete {
                        direction: MoveDirection::Forward,
                        unit: MoveUnit::Word,
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &Enter, _window, cx| {
                this.dispatch_command(RichCommand::SplitBlock, cx);
            }))
            .on_action(cx.listener(|this, _: &SoftBreak, _window, cx| {
                this.dispatch_command(RichCommand::InsertSoftBreak, cx);
            }))
            .on_action(cx.listener(|this, _: &Indent, _window, cx| {
                this.dispatch_command(RichCommand::Indent, cx);
            }))
            .on_action(cx.listener(|this, _: &Outdent, _window, cx| {
                this.dispatch_command(RichCommand::Outdent, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleBold, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Bold), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleItalic, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Italic), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleUnderline, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Underline), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleStrikethrough, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Strikethrough), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleInlineCode, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::InlineCode), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSuperscript, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Superscript), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSubscript, _window, cx| {
                this.dispatch_command(RichCommand::ToggleMark(MarkKind::Subscript), cx);
            }))
            .on_action(cx.listener(|this, _: &SetHeading1, _window, cx| {
                this.dispatch_command(RichCommand::SetHeading(Some(1)), cx);
            }))
            .on_action(cx.listener(|this, _: &SetHeading2, _window, cx| {
                this.dispatch_command(RichCommand::SetHeading(Some(2)), cx);
            }))
            .on_action(cx.listener(|this, _: &SetHeading3, _window, cx| {
                this.dispatch_command(RichCommand::SetHeading(Some(3)), cx);
            }))
            .on_action(cx.listener(|this, _: &SetParagraph, _window, cx| {
                this.dispatch_command(RichCommand::SetHeading(None), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleBulletList, _window, cx| {
                this.dispatch_command(RichCommand::ToggleList(ListKind::Bullet), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleOrderedList, _window, cx| {
                this.dispatch_command(
                    RichCommand::ToggleList(ListKind::Ordered(NumberStyle::Decimal)),
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &ToggleChecklist, _window, cx| {
                this.dispatch_command(RichCommand::ToggleList(ListKind::Checklist), cx);
            }))
            .on_action(cx.listener(|this, _: &AlignLeft, _window, cx| {
                this.dispatch_command(RichCommand::SetTextAlign(TextAlign::Left), cx);
            }))
            .on_action(cx.listener(|this, _: &AlignCenter, _window, cx| {
                this.dispatch_command(RichCommand::SetTextAlign(TextAlign::Center), cx);
            }))
            .on_action(cx.listener(|this, _: &AlignRight, _window, cx| {
                this.dispatch_command(RichCommand::SetTextAlign(TextAlign::Right), cx);
            }))
            .on_action(cx.listener(|this, _: &AlignJustify, _window, cx| {
                this.dispatch_command(RichCommand::SetTextAlign(TextAlign::Justify), cx);
            }))
            .on_action(cx.listener(|this, _: &ClearFormatting, _window, cx| {
                this.dispatch_command(RichCommand::ClearFormatting, cx);
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _window, cx| {
                this.dispatch_command(RichCommand::AdjustFontSize(1.0), cx);
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _window, cx| {
                this.dispatch_command(RichCommand::AdjustFontSize(-1.0), cx);
            }))
            .on_action(cx.listener(|this, _: &InsertTableRowAbove, _window, cx| {
                this.dispatch_command(RichCommand::InsertTableRow { after: false }, cx);
            }))
            .on_action(cx.listener(|this, _: &InsertTableRowBelow, _window, cx| {
                this.dispatch_command(RichCommand::InsertTableRow { after: true }, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteTableRow, _window, cx| {
                this.dispatch_command(RichCommand::DeleteTableRow, cx);
            }))
            .on_action(cx.listener(|this, _: &InsertTableColumnBefore, _window, cx| {
                this.dispatch_command(RichCommand::InsertTableColumn { after: false }, cx);
            }))
            .on_action(cx.listener(|this, _: &InsertTableColumnAfter, _window, cx| {
                this.dispatch_command(RichCommand::InsertTableColumn { after: true }, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteTableColumn, _window, cx| {
                this.dispatch_command(RichCommand::DeleteTableColumn, cx);
            }))
            .on_action(cx.listener(|this, _: &MergeTableCells, _window, cx| {
                this.dispatch_command(RichCommand::MergeTableCells, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitTableCell, _window, cx| {
                this.dispatch_command(RichCommand::SplitTableCell, cx);
            }))
            .on_scroll(cx.listener(Self::handle_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .child(RichDocumentElement::new(cx.entity()))
    }
}
