//! Interactive GPUI view hosting [`RichDocumentElement`] with command-driven actions,
//! policy gating, theming, host events, keyboard navigation, IME input, and core editing.

use crate::gpui::asset::{AssetProvider, LoadedAsset, decode_image_asset};
use crate::gpui::element::RichDocumentElement;
use crate::gpui::extension::{
    BlockRenderer, CommandExecutionResult, EditCommandContext, ExtensionDiagnostic,
    ExtensionRegistrationError, ExtensionRegistry, InlineRenderer, ReadCommandContext,
    ReadCommandOutcome,
};
use crate::gpui::shaper::{FontResolver, GpuiGlyphBackend, GpuiShaper};
use crate::gpui::theme::RichTheme;
use crate::layout::{
    FlowLayout, Point, Rect, ShaperRunSpec, TableColumnDividerHit, pt_to_px,
    resolve_horizontal_metrics,
};
use crate::model::{
    Affinity, BlockId, BlockKind, BlockNode, BlockSpec, BlockStyle, ChangeSet, ChipKind,
    ClipboardBlock, ClipboardFragment, CodecError, ColumnSpec, CommandDecision, CommandEffectKind,
    CommandMiddleware, CommandOutcome, CommandResolution, CommandSource, CrdtSyncBridge,
    DecorationProvider, DocPos, Document, DocumentCodec, DocumentSelection, DynamicDocumentCodec,
    Edges, Edit, EncodedDocument, ExtensionCommand, ExtensionInvocation, History,
    HistoryMergeGroup, HorizontalLayout, ImageAssetId, InlineContent, InlineObject, InlineObjectId,
    InlineSpec, InlineStyle, InteractionPolicy, LinkTarget, ListKind, MarkKind, MoveDirection,
    MoveUnit, NamedStyle, NumberStyle, OBJECT_REPLACEMENT_UTF8_LEN, PathSegment, ReadOnly,
    ResolvedCommand, ResolvedTextStyle, RgbaColor, RichCommand, SelectionValue, SharedStr, Step,
    StyleId, SyncDelta, TextAlign, Transaction,
};
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    Font, InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point as GpuiPoint, Render, ScrollDelta, ScrollEvent,
    Styled, Task, UTF16Selection, Window, actions, div, point, px, size,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

mod actions;
mod anchors;
mod clipboard;
mod commands;
pub(crate) mod components;
mod input;
mod navigation;
mod structure;
mod table;

pub use actions::*;
pub(crate) use input::image_resize_handle_rects;
use input::{
    extract_clipboard_fragment, extract_clipboard_html, is_external_uri, last_text_pos_in_nodes,
    next_grapheme_boundary, next_word_boundary, previous_grapheme_boundary, previous_word_boundary,
    table_cell_of,
};

const CARET_BLINK_PAUSE_DELAY: Duration = Duration::from_millis(500);
const CARET_SCROLL_MARGIN_PX: f32 = 28.0;
const MAX_LIST_INDENT_LEVEL: u8 = 6;
const CLIPBOARD_HTML_METADATA_KEY: &str = "gpui_rich_html";
const CLIPBOARD_FRAGMENT_METADATA_KEY: &str = "gpui_rich_fragment_v1";

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
    pub(crate) navigation: navigation::NavigationState,
    pub(crate) pending_reveal: Option<DocPos>,
    pending_input_rule_undo: Option<structure::InputRuleUndo>,
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
    _paste_task: Task<()>,
    document_revision: u64,
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
    pending_footnote: Option<crate::model::AnchorId>,
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
            navigation: navigation::NavigationState::default(),
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
            _paste_task: Task::ready(()),
            document_revision: 0,
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
        let formatter = clipboard::ComponentClipboardFormatter::new(self);
        let plain_text = fragment.to_plain_text_with(&formatter);
        let html = fragment.to_html_with(&formatter);
        let mut structured = fragment;
        clipboard::attach_component_definitions(&self.document.components, &mut structured);
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

    fn handle_paste_action(
        &mut self,
        plain_text_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.policy.editing.enabled {
            return;
        }
        if let Some(item) = cx.read_from_clipboard() {
            self.ime_preedit = None;
            self.paste_clipboard_item(&item, plain_text_only, cx);
            return;
        }
        // Web browsers expose clipboard reads asynchronously behind user activation.
        let read = cx.read_from_clipboard_async();
        let target_revision = self.document_revision;
        let target_selection = self.selection.clone();
        self._paste_task = cx.spawn_in(window, async move |weak_view, cx| match read.await {
            Ok(Some(item)) => {
                if weak_view
                    .update_in(cx, |view, window, cx| {
                        if view.policy.editing.enabled
                            && view.focus_handle.is_focused(window)
                            && view.document_revision == target_revision
                            && view.selection == target_selection
                        {
                            view.ime_preedit = None;
                            view.paste_clipboard_item(&item, plain_text_only, cx);
                        }
                    })
                    .is_err()
                {
                    // View was dropped before the asynchronous clipboard read completed.
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("rich-gpui: failed to read clipboard for paste: {error}");
            }
        });
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
        self.document_revision = self.document_revision.wrapping_add(1);
        self._paste_task = Task::ready(());
        self.history = History::default();
        self.flow.cache.clear();
        self.extensions.clear_resolved_caches();
        self.extension_diagnostics.clear();
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.ime_preedit = None;
        self.navigation = navigation::NavigationState::default();
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
    pub fn with_decoration_provider(mut self, provider: impl DecorationProvider + 'static) -> Self {
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
                            && cx
                                .background_executor()
                                .now()
                                .saturating_duration_since(started_at)
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

    fn resync_layout_with_anchor(&mut self, anchor: Option<crate::layout::ScrollAnchor>) {
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
            .on_action(cx.listener(|this, _: &Paste, window, cx| {
                this.handle_paste_action(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PastePlainText, window, cx| {
                this.handle_paste_action(true, window, cx);
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
            .on_action(
                cx.listener(|this, _: &InsertTableColumnBefore, _window, cx| {
                    this.dispatch_command(RichCommand::InsertTableColumn { after: false }, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &InsertTableColumnAfter, _window, cx| {
                    this.dispatch_command(RichCommand::InsertTableColumn { after: true }, cx);
                }),
            )
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
