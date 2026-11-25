//! Extensibility Seam 3: spec-bound block and inline renderers ([`BlockRenderer`],
//! [`InlineRenderer`]), hosted GPUI `AnyElement` rendering, typed read/edit extension commands,
//! resolved payload caching, diagnostics, and [`ExtensionRegistry`].
//!
//! # Breaking changes
//!
//! * The raw `NodeExtension` trait and `register_node` / `register_typed_*` string-keyed methods
//!   have been removed. Renderers bind directly to a [`crate::model::BlockSpec`] or
//!   [`crate::model::InlineSpec`] and are stored in separate block and inline namespaces.
//! * Raw command handlers accepting `&mut Document` and caller-supplied `mutates_document` have
//!   been removed. Commands implement [`crate::model::ExtensionCommand`] with a sealed
//!   [`crate::model::ReadOnly`] or [`crate::model::Edit`] effect and register through
//!   [`ExtensionRegistry::register_read_command`] or [`ExtensionRegistry::register_edit_command`].
//!
//! # Example
//! ```rust
//! use gpui_rich::gpui::{ExtensionRegistry, ReadCommandOutcome};
//! use gpui_rich::model::{Edit, ExtensionCommand, ReadOnly, Transaction};
//!
//! #[derive(Debug)]
//! struct InspectBadge;
//! impl ExtensionCommand for InspectBadge {
//!     const ID: &'static str = "acme.badge.inspect";
//!     type Effect = ReadOnly;
//! }
//!
//! #[derive(Debug)]
//! struct RenameBadge {
//!     label: String,
//! }
//! impl ExtensionCommand for RenameBadge {
//!     const ID: &'static str = "acme.badge.rename";
//!     type Effect = Edit;
//! }
//!
//! let mut registry = ExtensionRegistry::new();
//! registry
//!     .register_read_command::<InspectBadge>(|_command, _context| ReadCommandOutcome::Executed)
//!     .unwrap();
//! registry
//!     .register_edit_command::<RenameBadge>(|_command, _context| Some(Transaction::new(Vec::new())))
//!     .unwrap();
//! ```
//!
//! A read command handler cannot return a [`Transaction`]:
//! ```compile_fail
//! use gpui_rich::gpui::ExtensionRegistry;
//! use gpui_rich::model::{ExtensionCommand, ReadOnly, Transaction};
//!
//! #[derive(Debug)]
//! struct InspectBadge;
//! impl ExtensionCommand for InspectBadge {
//!     const ID: &'static str = "acme.badge.inspect";
//!     type Effect = ReadOnly;
//! }
//!
//! let mut registry = ExtensionRegistry::new();
//! let _ = registry.register_read_command::<InspectBadge>(|_command, _context| {
//!     Some(Transaction::new(Vec::new()))
//! });
//! ```
//!
//! Registering a `ReadOnly` command as an edit command also fails at compile time:
//! ```compile_fail
//! use gpui_rich::gpui::ExtensionRegistry;
//! use gpui_rich::model::{ExtensionCommand, ReadOnly, Transaction};
//!
//! #[derive(Debug)]
//! struct InspectBadge;
//! impl ExtensionCommand for InspectBadge {
//!     const ID: &'static str = "acme.badge.inspect";
//!     type Effect = ReadOnly;
//! }
//!
//! let mut registry = ExtensionRegistry::new();
//! let _ = registry.register_edit_command::<InspectBadge>(|_command, _context| {
//!     Some(Transaction::new(Vec::new()))
//! });
//! ```

use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::layout::{InlineBoxMetrics, Insets, Size, pt_to_px};
use crate::model::{
    ActiveTableContext, BlockId, BlockShape, BlockShapeKind, BlockSpec, ClipboardCustomFormatter,
    CommandEffectKind, CommandResolution, CustomPayload, DROPDOWN_KIND, Document,
    DocumentSelection, Edit, ExtensionCommand, ExtensionInvocation, ExtensionQuery,
    FOOTNOTE_BODY_KIND, FOOTNOTE_REFERENCE_KIND, HistoryMergeGroup, InlineObjectId, InlineSpec,
    InlineVAlign, InteractionPolicy, ListId, PLACEHOLDER_KIND, QueryError, ReadOnly, RgbaColor,
    RichCommand, SearchState, SharedStr, Transaction, VARIABLE_KIND, block_kinds,
};
use gpui_kit::{AnyElement, App, Bounds, Context, Pixels, Window};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Weak};

/// Direct immediate-mode paint callback invoked for a custom node.
pub type DirectPaintCallback = Box<dyn FnOnce(Bounds<Pixels>, &RichTheme, &mut Window, &mut App)>;

/// Rendering output returned by [`BlockRenderer::render`] or [`InlineRenderer::render`].
pub enum CustomRenderOutput {
    /// Hosted interactive GPUI element (including `Entity<V>` views converted with `.into_any_element()`)
    /// laid out and prepainted at the node's computed canvas rectangle.
    Element(AnyElement),
    /// Direct immediate-mode paint callback invoked during element painting.
    DirectPaint(DirectPaintCallback),
}

/// Typed context passed to [`BlockRenderer::render`].
pub struct BlockRenderContext<'render, T> {
    pub block_id: BlockId,
    pub kind: &'render str,
    pub data: &'render T,
    pub bounds: Bounds<Pixels>,
    pub chrome: Insets,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Typed context passed to [`InlineRenderer::measure_with_context`].
pub struct InlineMeasureContext<'measure, T> {
    pub block_id: Option<BlockId>,
    pub object_id: InlineObjectId,
    pub data: &'measure T,
    pub font_size_pt: f32,
    pub text_color: RgbaColor,
    pub zoom: f32,
}

/// Typed context passed to [`InlineRenderer::render`].
pub struct InlineRenderContext<'render, T> {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: &'render str,
    pub data: &'render T,
    pub bounds: Bounds<Pixels>,
    pub font_size_pt: f32,
    pub text_color: RgbaColor,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Typed renderer and clipboard presentation hook bound to a [`BlockSpec`].
pub trait BlockRenderer: 'static {
    /// Model-side block specification identifying the persisted kind, shape, and payload type.
    type Spec: BlockSpec;

    /// Measures a custom leaf block (`block_shape::Leaf`) inside `max_width` pixels at `zoom`.
    fn measure_leaf(
        &self,
        _block_id: BlockId,
        _data: &<Self::Spec as BlockSpec>::Data,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    /// Measures chrome insets (in pixels) reserved around a custom text, container, or grid block.
    fn measure_chrome(
        &self,
        _data: &<Self::Spec as BlockSpec>::Data,
        _max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Insets {
        Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible custom block.
    ///
    /// Leaf blocks (`block_shape::Leaf`) paint on Layer 4 (foreground). Text, container, and grid
    /// blocks paint on Layer 1 (chrome behind editable text and children).
    fn render(
        &self,
        _context: BlockRenderContext<'_, <Self::Spec as BlockSpec>::Data>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Optional plain-text clipboard presentation for this block.
    fn plain_text(&self, _data: &<Self::Spec as BlockSpec>::Data) -> Option<String> {
        None
    }

    /// Optional HTML clipboard presentation for this block.
    ///
    /// Presentation HTML does not replace the versioned structured fragment used for same-process
    /// rich paste.
    fn html(&self, _data: &<Self::Spec as BlockSpec>::Data) -> Option<String> {
        None
    }
}

/// Typed renderer and clipboard presentation hook bound to an [`InlineSpec`].
pub trait InlineRenderer: 'static {
    /// Model-side inline specification identifying the persisted kind and payload type.
    type Spec: InlineSpec;

    /// Measures a custom inline object at `zoom`.
    fn measure(
        &self,
        _data: &<Self::Spec as InlineSpec>::Data,
        zoom: f32,
        _window: &Window,
    ) -> InlineBoxMetrics {
        InlineBoxMetrics::new(
            pt_to_px(72.0, zoom),
            pt_to_px(18.0, zoom),
            InlineVAlign::Middle,
        )
    }

    /// Measures a custom inline object with owning block, object ID, and surrounding text style.
    ///
    /// Defaults to calling [`Self::measure`].
    fn measure_with_context(
        &self,
        context: InlineMeasureContext<'_, <Self::Spec as InlineSpec>::Data>,
        window: &Window,
    ) -> InlineBoxMetrics {
        self.measure(context.data, context.zoom, window)
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible inline object.
    fn render(
        &self,
        _context: InlineRenderContext<'_, <Self::Spec as InlineSpec>::Data>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Optional plain-text clipboard presentation for this inline object.
    fn plain_text(&self, _data: &<Self::Spec as InlineSpec>::Data) -> Option<String> {
        None
    }

    /// Optional HTML clipboard presentation for this inline object.
    ///
    /// Presentation HTML does not replace the versioned structured fragment used for same-process
    /// rich paste.
    fn html(&self, _data: &<Self::Spec as InlineSpec>::Data) -> Option<String> {
        None
    }
}

/// Bundle of renderers, commands, and queries installed atomically onto a [`RichDocumentView`].
pub trait ExtensionPlugin {
    /// Registers this extension's renderers, commands, and queries on `view`.
    fn install(
        self,
        view: &mut RichDocumentView,
        cx: &mut Context<RichDocumentView>,
    ) -> Result<(), ExtensionRegistrationError>;
}

/// Namespace within [`ExtensionRegistry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionNamespace {
    Block,
    Inline,
    Command,
    Query,
    Completion,
}

impl fmt::Display for ExtensionNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block => formatter.write_str("block"),
            Self::Inline => formatter.write_str("inline"),
            Self::Command => formatter.write_str("command"),
            Self::Query => formatter.write_str("query"),
            Self::Completion => formatter.write_str("completion"),
        }
    }
}

/// Error returned when registering a block renderer, inline renderer, or extension command fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionRegistrationError {
    /// Registration key was empty or whitespace-only.
    EmptyKey { namespace: ExtensionNamespace },
    /// Registration key is reserved for built-in library nodes or commands.
    ReservedKey {
        namespace: ExtensionNamespace,
        key: &'static str,
    },
    /// Registration key is already registered in the same namespace.
    DuplicateKey {
        namespace: ExtensionNamespace,
        key: &'static str,
    },
}

impl fmt::Display for ExtensionRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey { namespace } => {
                write!(formatter, "empty registration key in {namespace} namespace")
            }
            Self::ReservedKey { namespace, key } => {
                write!(formatter, "reserved {namespace} registration key '{key}'")
            }
            Self::DuplicateKey { namespace, key } => {
                write!(formatter, "duplicate {namespace} registration key '{key}'")
            }
        }
    }
}

impl std::error::Error for ExtensionRegistrationError {}

/// Identifies a document node associated with an [`ExtensionDiagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionNodeTarget {
    Block {
        block_id: BlockId,
    },
    Inline {
        block_id: BlockId,
        object_id: InlineObjectId,
    },
}

/// Classification of a runtime validation failure on a registered custom node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionDiagnosticKind {
    /// Node kind is registered, but its structural shape does not match the registered [`BlockSpec`].
    ShapeMismatch {
        expected: BlockShapeKind,
        actual: BlockShapeKind,
    },
    /// Node kind is registered, but its payload failed decoding or held the wrong Rust type.
    InvalidPayload { message: String },
    /// Registered renderer returned non-finite or negative layout metrics.
    InvalidMetrics { message: String },
}

/// Diagnostic emitted when a node matches a registered extension kind but fails validation.
///
/// Unregistered custom kinds fall back silently without producing a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionDiagnostic {
    pub target: ExtensionNodeTarget,
    pub kind: SharedStr,
    pub detail: ExtensionDiagnosticKind,
}

/// Outcome returned by a read-only extension command handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadCommandOutcome {
    Executed,
    NoOp,
}

/// Read-only context supplied to a read-only extension command handler.
pub struct ReadCommandContext<'document> {
    document: &'document Document,
    selection: Option<&'document DocumentSelection>,
    invalidated_blocks: RefCell<Vec<BlockId>>,
}

impl<'document> ReadCommandContext<'document> {
    pub(crate) fn new(
        document: &'document Document,
        selection: Option<&'document DocumentSelection>,
    ) -> Self {
        Self {
            document,
            selection,
            invalidated_blocks: RefCell::new(Vec::new()),
        }
    }

    /// Borrows the current [`Document`] immutably.
    pub fn document(&self) -> &'document Document {
        self.document
    }

    /// Borrows the current [`DocumentSelection`], if any.
    pub fn selection(&self) -> Option<&'document DocumentSelection> {
        self.selection
    }

    /// Marks `block_id` for non-mutating layout re-measurement after this read command completes.
    pub fn invalidate_block_layout(&self, block_id: BlockId) {
        let mut blocks = self.invalidated_blocks.borrow_mut();
        if !blocks.contains(&block_id) {
            blocks.push(block_id);
        }
    }

    pub(crate) fn take_invalidated_blocks(&self) -> Vec<BlockId> {
        std::mem::take(&mut *self.invalidated_blocks.borrow_mut())
    }
}

/// Read-only view context supplied to a registered [`ExtensionQuery`] handler.
pub struct QueryContext<'view> {
    document: &'view Document,
    selection: Option<&'view DocumentSelection>,
    policy: InteractionPolicy,
    search_state: &'view SearchState,
    table_context: Option<ActiveTableContext>,
}

impl<'view> QueryContext<'view> {
    pub(crate) fn new(
        document: &'view Document,
        selection: Option<&'view DocumentSelection>,
        policy: InteractionPolicy,
        search_state: &'view SearchState,
        table_context: Option<ActiveTableContext>,
    ) -> Self {
        Self {
            document,
            selection,
            policy,
            search_state,
            table_context,
        }
    }

    /// Borrows the current [`Document`] immutably.
    pub fn document(&self) -> &'view Document {
        self.document
    }

    /// Borrows the current [`DocumentSelection`], if any.
    pub fn selection(&self) -> Option<&'view DocumentSelection> {
        self.selection
    }

    /// Returns the view's active [`InteractionPolicy`].
    pub const fn policy(&self) -> InteractionPolicy {
        self.policy
    }

    /// Borrows the view's active [`SearchState`].
    pub fn search_state(&self) -> &'view SearchState {
        self.search_state
    }

    /// Returns the active [`ActiveTableContext`], if the selection is inside a table.
    pub const fn active_table_context(&self) -> Option<ActiveTableContext> {
        self.table_context
    }
}

/// Read-only document context with [`BlockId`] reservation supplied to an edit command planner.
///
/// Reserving IDs alone does not mutate [`Document`], emit mutation events, or add undo entries.
pub struct EditCommandContext<'document> {
    document: &'document Document,
    selection: Option<&'document DocumentSelection>,
    next_block_id: u64,
    history_group: HistoryMergeGroup,
}

impl<'document> EditCommandContext<'document> {
    pub(crate) fn new(
        document: &'document Document,
        selection: Option<&'document DocumentSelection>,
    ) -> Self {
        Self {
            document,
            selection,
            next_block_id: document.peek_next_block_id(),
            history_group: HistoryMergeGroup::Atomic,
        }
    }

    /// Borrows the current [`Document`] immutably.
    pub fn document(&self) -> &'document Document {
        self.document
    }

    /// Borrows the current [`DocumentSelection`], if any.
    pub fn selection(&self) -> Option<&'document DocumentSelection> {
        self.selection
    }

    /// Reserves a fresh [`BlockId`] for a new node planned inside the returned [`Transaction`].
    pub fn reserve_block_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id = self.next_block_id.saturating_add(1);
        id
    }

    /// Allocates a [`ListId`] greater than any list currently in the document.
    pub fn alloc_list_id(&self) -> ListId {
        self.document.alloc_list_id()
    }

    /// Configures the undo [`HistoryMergeGroup`] applied when the returned [`Transaction`] commits.
    pub fn set_history_group(&mut self, history_group: HistoryMergeGroup) {
        self.history_group = history_group;
    }

    pub(crate) fn next_block_id(&self) -> u64 {
        self.next_block_id
    }

    pub(crate) fn history_group(&self) -> HistoryMergeGroup {
        self.history_group
    }
}

/// Internal resolved payload cache keyed by the raw `Arc<serde_json::Value>` pointer with
/// `Weak` liveness verification so unchanged raw payloads are decoded at most once across
/// measure, render, and clipboard callbacks.
struct PayloadCacheEntry<T> {
    source: Weak<serde_json::Value>,
    result: Result<Rc<T>, String>,
}

struct TypedPayloadCache<T> {
    entries: RefCell<HashMap<usize, PayloadCacheEntry<T>>>,
}

impl<T> Default for TypedPayloadCache<T> {
    fn default() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
        }
    }
}

impl<T: Clone + serde::de::DeserializeOwned + 'static> TypedPayloadCache<T> {
    fn clear(&self) {
        self.entries.borrow_mut().clear();
    }

    fn with_resolved<R>(
        &self,
        data: &CustomPayload,
        callback: impl FnOnce(&T) -> R,
    ) -> Result<R, String> {
        if let Some(borrowed) = data.downcast_ref::<T>() {
            return Ok(callback(borrowed));
        }
        let Some(raw_arc) = data.raw_json() else {
            return match data.try_as_typed::<T>() {
                Ok(cow) => Ok(callback(cow.as_ref())),
                Err(error) => Err(error.to_string()),
            };
        };

        let key = Arc::as_ptr(raw_arc) as usize;
        let cached = {
            let entries = self.entries.borrow();
            entries.get(&key).and_then(|entry| {
                let upgraded = entry.source.upgrade()?;
                Arc::ptr_eq(&upgraded, raw_arc).then(|| entry.result.clone())
            })
        };

        let resolved = match cached {
            Some(result) => result,
            None => {
                let decoded = serde_json::from_value::<T>((**raw_arc).clone())
                    .map(Rc::new)
                    .map_err(|error| error.to_string());
                let mut entries = self.entries.borrow_mut();
                if entries.len() >= 256 {
                    entries.retain(|_, entry| entry.source.strong_count() > 0);
                }
                entries.insert(
                    key,
                    PayloadCacheEntry {
                        source: Arc::downgrade(raw_arc),
                        result: decoded.clone(),
                    },
                );
                decoded
            }
        };

        match resolved {
            Ok(typed) => Ok(callback(typed.as_ref())),
            Err(message) => Err(message),
        }
    }
}

pub(crate) enum BlockCallbackResult<T> {
    Ok(T),
    Invalid(ExtensionDiagnosticKind),
}

pub(crate) enum InlineCallbackResult<T> {
    Ok(T),
    Invalid(ExtensionDiagnosticKind),
}

pub(crate) struct ErasedBlockRenderContext<'a> {
    pub(crate) shape: BlockShapeKind,
    pub(crate) block_id: BlockId,
    pub(crate) kind: &'a str,
    pub(crate) data: &'a CustomPayload,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) chrome: Insets,
    pub(crate) zoom: f32,
    pub(crate) theme: &'a RichTheme,
}

pub(crate) struct ErasedInlineMeasureContext<'a> {
    pub(crate) block_id: Option<BlockId>,
    pub(crate) object_id: InlineObjectId,
    pub(crate) data: &'a CustomPayload,
    pub(crate) font_size_pt: f32,
    pub(crate) text_color: RgbaColor,
    pub(crate) zoom: f32,
}

pub(crate) struct ErasedInlineRenderContext<'a> {
    pub(crate) block_id: BlockId,
    pub(crate) object_id: InlineObjectId,
    pub(crate) kind: &'a str,
    pub(crate) data: &'a CustomPayload,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) font_size_pt: f32,
    pub(crate) text_color: RgbaColor,
    pub(crate) zoom: f32,
    pub(crate) theme: &'a RichTheme,
}

pub(crate) trait ErasedBlockRenderer {
    fn clear_cache(&self);
    fn measure_leaf(
        &self,
        block_id: BlockId,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> BlockCallbackResult<Size>;
    fn measure_chrome(
        &self,
        shape: BlockShapeKind,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> BlockCallbackResult<Insets>;
    fn render(
        &self,
        context: ErasedBlockRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> BlockCallbackResult<Option<CustomRenderOutput>>;
    fn plain_text(&self, shape: BlockShapeKind, data: &CustomPayload) -> Option<String>;
    fn html(&self, shape: BlockShapeKind, data: &CustomPayload) -> Option<String>;
}

struct BlockRendererAdapter<R: BlockRenderer> {
    renderer: R,
    cache: TypedPayloadCache<<R::Spec as BlockSpec>::Data>,
}

impl<R: BlockRenderer> BlockRendererAdapter<R> {
    fn new(renderer: R) -> Self {
        Self {
            renderer,
            cache: TypedPayloadCache::default(),
        }
    }

    fn check_shape(&self, actual: BlockShapeKind) -> Result<(), ExtensionDiagnosticKind> {
        let expected = <R::Spec as BlockSpec>::Shape::KIND;
        if actual == expected {
            Ok(())
        } else {
            Err(ExtensionDiagnosticKind::ShapeMismatch { expected, actual })
        }
    }
}

impl<R: BlockRenderer> ErasedBlockRenderer for BlockRendererAdapter<R> {
    fn clear_cache(&self) {
        self.cache.clear();
    }

    fn measure_leaf(
        &self,
        block_id: BlockId,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> BlockCallbackResult<Size> {
        if let Err(detail) = self.check_shape(BlockShapeKind::Leaf) {
            return BlockCallbackResult::Invalid(detail);
        }
        match self.cache.with_resolved(data, |typed| {
            self.renderer
                .measure_leaf(block_id, typed, max_width, zoom, window)
        }) {
            Ok(size) => BlockCallbackResult::Ok(size),
            Err(message) => {
                BlockCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn measure_chrome(
        &self,
        shape: BlockShapeKind,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> BlockCallbackResult<Insets> {
        if let Err(detail) = self.check_shape(shape) {
            return BlockCallbackResult::Invalid(detail);
        }
        match self.cache.with_resolved(data, |typed| {
            self.renderer.measure_chrome(typed, max_width, zoom, window)
        }) {
            Ok(insets) => BlockCallbackResult::Ok(insets),
            Err(message) => {
                BlockCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn render(
        &self,
        context: ErasedBlockRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> BlockCallbackResult<Option<CustomRenderOutput>> {
        if let Err(detail) = self.check_shape(context.shape) {
            return BlockCallbackResult::Invalid(detail);
        }
        match self.cache.with_resolved(context.data, |typed| {
            self.renderer.render(
                BlockRenderContext {
                    block_id: context.block_id,
                    kind: context.kind,
                    data: typed,
                    bounds: context.bounds,
                    chrome: context.chrome,
                    zoom: context.zoom,
                    theme: context.theme,
                },
                window,
                cx,
            )
        }) {
            Ok(output) => BlockCallbackResult::Ok(output),
            Err(message) => {
                BlockCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn plain_text(&self, shape: BlockShapeKind, data: &CustomPayload) -> Option<String> {
        self.check_shape(shape).ok()?;
        self.cache
            .with_resolved(data, |typed| self.renderer.plain_text(typed))
            .ok()?
    }

    fn html(&self, shape: BlockShapeKind, data: &CustomPayload) -> Option<String> {
        self.check_shape(shape).ok()?;
        self.cache
            .with_resolved(data, |typed| self.renderer.html(typed))
            .ok()?
    }
}

pub(crate) trait ErasedInlineRenderer {
    fn clear_cache(&self);
    fn measure(
        &self,
        context: ErasedInlineMeasureContext<'_>,
        window: &Window,
    ) -> InlineCallbackResult<InlineBoxMetrics>;
    fn render(
        &self,
        context: ErasedInlineRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> InlineCallbackResult<Option<CustomRenderOutput>>;
    fn plain_text(&self, data: &CustomPayload) -> Option<String>;
    fn html(&self, data: &CustomPayload) -> Option<String>;
}

struct InlineRendererAdapter<R: InlineRenderer> {
    renderer: R,
    cache: TypedPayloadCache<<R::Spec as InlineSpec>::Data>,
}

impl<R: InlineRenderer> InlineRendererAdapter<R> {
    fn new(renderer: R) -> Self {
        Self {
            renderer,
            cache: TypedPayloadCache::default(),
        }
    }
}

impl<R: InlineRenderer> ErasedInlineRenderer for InlineRendererAdapter<R> {
    fn clear_cache(&self) {
        self.cache.clear();
    }

    fn measure(
        &self,
        context: ErasedInlineMeasureContext<'_>,
        window: &Window,
    ) -> InlineCallbackResult<InlineBoxMetrics> {
        match self.cache.with_resolved(context.data, |typed| {
            self.renderer.measure_with_context(
                InlineMeasureContext {
                    block_id: context.block_id,
                    object_id: context.object_id,
                    data: typed,
                    font_size_pt: context.font_size_pt,
                    text_color: context.text_color,
                    zoom: context.zoom,
                },
                window,
            )
        }) {
            Ok(metrics) => InlineCallbackResult::Ok(metrics),
            Err(message) => {
                InlineCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn render(
        &self,
        context: ErasedInlineRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> InlineCallbackResult<Option<CustomRenderOutput>> {
        match self.cache.with_resolved(context.data, |typed| {
            self.renderer.render(
                InlineRenderContext {
                    block_id: context.block_id,
                    object_id: context.object_id,
                    kind: context.kind,
                    data: typed,
                    bounds: context.bounds,
                    font_size_pt: context.font_size_pt,
                    text_color: context.text_color,
                    zoom: context.zoom,
                    theme: context.theme,
                },
                window,
                cx,
            )
        }) {
            Ok(output) => InlineCallbackResult::Ok(output),
            Err(message) => {
                InlineCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn plain_text(&self, data: &CustomPayload) -> Option<String> {
        self.cache
            .with_resolved(data, |typed| self.renderer.plain_text(typed))
            .ok()?
    }

    fn html(&self, data: &CustomPayload) -> Option<String> {
        self.cache
            .with_resolved(data, |typed| self.renderer.html(typed))
            .ok()?
    }
}

pub(crate) enum CommandExecutionResult {
    Read {
        outcome: ReadCommandOutcome,
        invalidated_blocks: Vec<BlockId>,
    },
    Edit {
        transaction: Option<Box<Transaction>>,
        next_block_id: u64,
        history_group: HistoryMergeGroup,
    },
    Unregistered,
    Mismatched,
}

type ErasedReadHandler =
    Box<dyn Fn(&ExtensionInvocation, &ReadCommandContext<'_>) -> ReadCommandOutcome>;
type ErasedEditHandler =
    Box<dyn Fn(&ExtensionInvocation, &mut EditCommandContext<'_>) -> Option<Transaction>>;
type ErasedQueryHandler = Box<dyn Fn(&dyn Any, &QueryContext<'_>) -> Option<Box<dyn Any>>>;

enum RegisteredCommandHandler {
    Read(ErasedReadHandler),
    Edit(ErasedEditHandler),
}

struct RegisteredCommand {
    type_id: TypeId,
    effect: CommandEffectKind,
    handler: RegisteredCommandHandler,
}

struct RegisteredQuery {
    query_type_id: TypeId,
    output_type_id: TypeId,
    handler: ErasedQueryHandler,
}

fn is_reserved_block_kind(kind: &str) -> bool {
    matches!(
        kind,
        block_kinds::PARAGRAPH
            | block_kinds::HEADING
            | block_kinds::LIST_ITEM
            | block_kinds::CODE_BLOCK
            | block_kinds::BLOCKQUOTE
            | block_kinds::TABLE
            | block_kinds::HORIZONTAL_RULE
            | FOOTNOTE_BODY_KIND
    )
}

fn is_reserved_inline_kind(kind: &str) -> bool {
    matches!(
        kind,
        "image"
            | "smart_chip"
            | "footnote_ref"
            | DROPDOWN_KIND
            | VARIABLE_KIND
            | PLACEHOLDER_KIND
            | FOOTNOTE_REFERENCE_KIND
    )
}

fn is_reserved_command_id(id: &str) -> bool {
    id.starts_with("rich.") || id.starts_with("anchor.")
}

fn is_reserved_query_id(id: &str) -> bool {
    id.starts_with("rich.") || id.starts_with("anchor.")
}

/// Registry storing spec-bound [`BlockRenderer`]s, [`InlineRenderer`]s, typed
/// [`ExtensionCommand`] handlers, and typed [`ExtensionQuery`] handlers in separate namespaces.
#[derive(Default)]
pub struct ExtensionRegistry {
    blocks: HashMap<&'static str, Rc<dyn ErasedBlockRenderer>>,
    inlines: HashMap<&'static str, Rc<dyn ErasedInlineRenderer>>,
    commands: HashMap<&'static str, RegisteredCommand>,
    queries: HashMap<&'static str, RegisteredQuery>,
}

impl ExtensionRegistry {
    /// Creates an empty [`ExtensionRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a spec-bound [`BlockRenderer`] under `R::Spec::KIND`.
    ///
    /// Rejects empty, reserved built-in, and duplicate block keys. Block and inline namespaces are
    /// independent, so the same kind spelling may be registered in both.
    pub fn register_block<R: BlockRenderer>(
        &mut self,
        renderer: R,
    ) -> Result<(), ExtensionRegistrationError> {
        let key = R::Spec::KIND;
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Block,
            });
        }
        if is_reserved_block_kind(key) {
            return Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Block,
                key,
            });
        }
        if self.blocks.contains_key(key) {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Block,
                key,
            });
        }
        self.blocks
            .insert(key, Rc::new(BlockRendererAdapter::new(renderer)));
        Ok(())
    }

    /// Registers a spec-bound [`InlineRenderer`] under `R::Spec::KIND`.
    ///
    /// Rejects empty, reserved built-in, and duplicate inline keys. Block and inline namespaces are
    /// independent, so the same kind spelling may be registered in both.
    pub fn register_inline<R: InlineRenderer>(
        &mut self,
        renderer: R,
    ) -> Result<(), ExtensionRegistrationError> {
        let key = R::Spec::KIND;
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Inline,
            });
        }
        if is_reserved_inline_kind(key) {
            return Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Inline,
                key,
            });
        }
        if self.inlines.contains_key(key) {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Inline,
                key,
            });
        }
        self.inlines
            .insert(key, Rc::new(InlineRendererAdapter::new(renderer)));
        Ok(())
    }

    /// Removes the registered [`BlockRenderer`] for [`BlockSpec`] `S`.
    pub fn unregister_block<S: BlockSpec>(&mut self) -> bool {
        self.blocks.remove(S::KIND).is_some()
    }

    /// Removes the registered [`InlineRenderer`] for [`InlineSpec`] `S`.
    pub fn unregister_inline<S: InlineSpec>(&mut self) -> bool {
        self.inlines.remove(S::KIND).is_some()
    }

    /// Returns `true` when a block renderer is registered for `kind`.
    pub fn has_block(&self, kind: &str) -> bool {
        self.blocks.contains_key(kind)
    }

    /// Returns `true` when an inline renderer is registered for `kind`.
    pub fn has_inline(&self, kind: &str) -> bool {
        self.inlines.contains_key(kind)
    }

    /// Registers a handler for a [`ReadOnly`] [`ExtensionCommand`] type `C`.
    pub fn register_read_command<C>(
        &mut self,
        handler: impl Fn(&C, &ReadCommandContext<'_>) -> ReadCommandOutcome + 'static,
    ) -> Result<(), ExtensionRegistrationError>
    where
        C: ExtensionCommand<Effect = ReadOnly>,
    {
        let key = C::ID;
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Command,
            });
        }
        if is_reserved_command_id(key) {
            return Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Command,
                key,
            });
        }
        if self.commands.contains_key(key) {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Command,
                key,
            });
        }
        let erased: ErasedReadHandler = Box::new(move |invocation, context| {
            let Some(args) = invocation.args::<C>() else {
                return ReadCommandOutcome::NoOp;
            };
            handler(args, context)
        });
        self.commands.insert(
            key,
            RegisteredCommand {
                type_id: TypeId::of::<C>(),
                effect: CommandEffectKind::ReadOnly,
                handler: RegisteredCommandHandler::Read(erased),
            },
        );
        Ok(())
    }

    /// Registers a transaction planner for an [`Edit`] [`ExtensionCommand`] type `C`.
    pub fn register_edit_command<C>(
        &mut self,
        handler: impl Fn(&C, &mut EditCommandContext<'_>) -> Option<Transaction> + 'static,
    ) -> Result<(), ExtensionRegistrationError>
    where
        C: ExtensionCommand<Effect = Edit>,
    {
        let key = C::ID;
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Command,
            });
        }
        if is_reserved_command_id(key) {
            return Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Command,
                key,
            });
        }
        if self.commands.contains_key(key) {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Command,
                key,
            });
        }
        let erased: ErasedEditHandler = Box::new(move |invocation, context| {
            let args = invocation.args::<C>()?;
            handler(args, context)
        });
        self.commands.insert(
            key,
            RegisteredCommand {
                type_id: TypeId::of::<C>(),
                effect: CommandEffectKind::Edit,
                handler: RegisteredCommandHandler::Edit(erased),
            },
        );
        Ok(())
    }

    /// Removes the registered handler for [`ExtensionCommand`] `C`.
    pub fn unregister_command<C: ExtensionCommand>(&mut self) -> bool {
        self.commands.remove(C::ID).is_some()
    }

    /// Registers a read-only handler for a typed [`ExtensionQuery`] `Q`.
    pub fn register_query<Q: ExtensionQuery>(
        &mut self,
        handler: impl Fn(&Q, &QueryContext<'_>) -> Q::Output + 'static,
    ) -> Result<(), ExtensionRegistrationError> {
        let key = Q::ID;
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Query,
            });
        }
        if is_reserved_query_id(key) {
            return Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Query,
                key,
            });
        }
        if self.queries.contains_key(key) {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Query,
                key,
            });
        }
        let erased: ErasedQueryHandler = Box::new(move |query, context| {
            let typed = query.downcast_ref::<Q>()?;
            Some(Box::new(handler(typed, context)))
        });
        self.queries.insert(
            key,
            RegisteredQuery {
                query_type_id: TypeId::of::<Q>(),
                output_type_id: TypeId::of::<Q::Output>(),
                handler: erased,
            },
        );
        Ok(())
    }

    /// Removes the registered handler for [`ExtensionQuery`] `Q`.
    pub fn unregister_query<Q: ExtensionQuery>(&mut self) -> bool {
        self.queries.remove(Q::ID).is_some()
    }

    /// Returns `true` when an extension query handler is registered for `id`.
    pub fn has_query(&self, id: &str) -> bool {
        self.queries.contains_key(id)
    }

    /// Executes a registered [`ExtensionQuery`] against `context`.
    pub fn execute_extension_query<Q: ExtensionQuery>(
        &self,
        query: &Q,
        context: &QueryContext<'_>,
    ) -> Result<Q::Output, QueryError> {
        let Some(entry) = self.queries.get(Q::ID) else {
            return Err(QueryError::UnregisteredQuery);
        };
        if entry.query_type_id != TypeId::of::<Q>()
            || entry.output_type_id != TypeId::of::<Q::Output>()
        {
            return Err(QueryError::MismatchedQueryType);
        }
        let Some(boxed) = (entry.handler)(query, context) else {
            return Err(QueryError::MismatchedQueryType);
        };
        boxed
            .downcast::<Q::Output>()
            .map(|value| *value)
            .map_err(|_| QueryError::MismatchedQueryType)
    }

    /// Resolves `command` against registered command IDs, concrete Rust types, and effects.
    pub fn resolve_command(&self, command: &RichCommand) -> CommandResolution {
        match command {
            RichCommand::Extension(invocation) => self.resolve_invocation(invocation),
            _ => CommandResolution::BuiltIn,
        }
    }

    pub(crate) fn resolve_invocation(&self, invocation: &ExtensionInvocation) -> CommandResolution {
        let Some(entry) = self.commands.get(invocation.id()) else {
            return CommandResolution::UnregisteredExtension;
        };
        if entry.type_id == invocation.type_id() && entry.effect == invocation.effect() {
            CommandResolution::RegisteredExtension
        } else {
            CommandResolution::MismatchedExtension
        }
    }

    pub(crate) fn execute_extension_command(
        &self,
        invocation: &ExtensionInvocation,
        document: &Document,
        selection: Option<&DocumentSelection>,
    ) -> CommandExecutionResult {
        let Some(entry) = self.commands.get(invocation.id()) else {
            return CommandExecutionResult::Unregistered;
        };
        if entry.type_id != invocation.type_id() || entry.effect != invocation.effect() {
            return CommandExecutionResult::Mismatched;
        }
        match &entry.handler {
            RegisteredCommandHandler::Read(handler) => {
                let context = ReadCommandContext::new(document, selection);
                let outcome = handler(invocation, &context);
                CommandExecutionResult::Read {
                    outcome,
                    invalidated_blocks: context.take_invalidated_blocks(),
                }
            }
            RegisteredCommandHandler::Edit(handler) => {
                let mut context = EditCommandContext::new(document, selection);
                let transaction = handler(invocation, &mut context).map(Box::new);
                CommandExecutionResult::Edit {
                    transaction,
                    next_block_id: context.next_block_id(),
                    history_group: context.history_group(),
                }
            }
        }
    }

    pub(crate) fn get_block(&self, kind: &str) -> Option<Rc<dyn ErasedBlockRenderer>> {
        self.blocks.get(kind).cloned()
    }

    pub(crate) fn get_inline(&self, kind: &str) -> Option<Rc<dyn ErasedInlineRenderer>> {
        self.inlines.get(kind).cloned()
    }

    pub(crate) fn clear_resolved_caches(&self) {
        for renderer in self.blocks.values() {
            renderer.clear_cache();
        }
        for renderer in self.inlines.values() {
            renderer.clear_cache();
        }
    }
}

impl ClipboardCustomFormatter for ExtensionRegistry {
    fn plain_text_block(
        &self,
        shape: BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        self.blocks.get(kind)?.plain_text(shape, data)
    }

    fn html_block(
        &self,
        shape: BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
    ) -> Option<String> {
        self.blocks.get(kind)?.html(shape, data)
    }

    fn plain_text_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.inlines.get(kind)?.plain_text(data)
    }

    fn html_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.inlines.get(kind)?.html(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BlockStyle, InlineContent};

    #[derive(Debug)]
    struct ReservedQuery;
    impl ExtensionQuery for ReservedQuery {
        const ID: &'static str = "rich.query.custom";
        type Output = usize;
    }

    #[derive(Debug)]
    struct EmptyKeyQuery;
    impl ExtensionQuery for EmptyKeyQuery {
        const ID: &'static str = "   ";
        type Output = usize;
    }

    #[derive(Debug)]
    struct ParagraphCount {
        min_len: usize,
    }
    impl ExtensionQuery for ParagraphCount {
        const ID: &'static str = "acme.doc.paragraph_count";
        type Output = usize;
    }

    #[derive(Debug)]
    struct ConflictingParagraphCount;
    impl ExtensionQuery for ConflictingParagraphCount {
        const ID: &'static str = "acme.doc.paragraph_count";
        type Output = String;
    }

    #[test]
    fn extension_query_registry_enforces_keys_and_type_safety() {
        let mut registry = ExtensionRegistry::new();
        assert_eq!(
            registry.register_query::<ReservedQuery>(|_, _| 0),
            Err(ExtensionRegistrationError::ReservedKey {
                namespace: ExtensionNamespace::Query,
                key: ReservedQuery::ID,
            })
        );
        assert_eq!(
            registry.register_query::<EmptyKeyQuery>(|_, _| 0),
            Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Query,
            })
        );

        let mut document = Document::new();
        document.add_paragraph(InlineContent::plain("hi"), BlockStyle::default());
        document.add_paragraph(InlineContent::plain("hello world"), BlockStyle::default());
        let search = SearchState::default();
        let context =
            QueryContext::new(&document, None, InteractionPolicy::viewer(), &search, None);

        assert_eq!(
            registry.execute_extension_query(&ParagraphCount { min_len: 1 }, &context),
            Err(QueryError::UnregisteredQuery)
        );

        registry
            .register_query::<ParagraphCount>(|query, context| {
                context
                    .document()
                    .blocks
                    .iter()
                    .filter(|block| {
                        block
                            .kind
                            .inline_content()
                            .is_some_and(|inline| inline.text.len() >= query.min_len)
                    })
                    .count()
            })
            .expect("register ParagraphCount");
        assert!(registry.has_query(ParagraphCount::ID));

        assert_eq!(
            registry.register_query::<ParagraphCount>(|_, _| 0),
            Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Query,
                key: ParagraphCount::ID,
            })
        );

        assert_eq!(
            registry.execute_extension_query(&ParagraphCount { min_len: 5 }, &context),
            Ok(1)
        );
        assert_eq!(
            registry.execute_extension_query(&ConflictingParagraphCount, &context),
            Err(QueryError::MismatchedQueryType)
        );

        assert!(registry.unregister_query::<ParagraphCount>());
        assert!(!registry.has_query(ParagraphCount::ID));
        assert_eq!(
            registry.execute_extension_query(&ParagraphCount { min_len: 1 }, &context),
            Err(QueryError::UnregisteredQuery)
        );
    }
}
