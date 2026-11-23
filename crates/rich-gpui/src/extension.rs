//! Extensibility Seam 3: spec-bound block and inline renderers ([`BlockRenderer`],
//! [`InlineRenderer`]), hosted GPUI `AnyElement` rendering, typed read/edit extension commands,
//! resolved payload caching, diagnostics, and [`ExtensionRegistry`].
//!
//! # Breaking changes
//!
//! * The raw `NodeExtension` trait and `register_node` / `register_typed_*` string-keyed methods
//!   have been removed. Renderers bind directly to a [`rich_model::BlockSpec`] or
//!   [`rich_model::InlineSpec`] and are stored in separate block and inline namespaces.
//! * Raw command handlers accepting `&mut Document` and caller-supplied `mutates_document` have
//!   been removed. Commands implement [`rich_model::ExtensionCommand`] with a sealed
//!   [`rich_model::ReadOnly`] or [`rich_model::Edit`] effect and register through
//!   [`ExtensionRegistry::register_read_command`] or [`ExtensionRegistry::register_edit_command`].
//!
//! # Example
//! ```rust
//! use rich_gpui::{ExtensionRegistry, ReadCommandOutcome};
//! use rich_model::{Edit, ExtensionCommand, ReadOnly, Transaction};
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
//! use rich_gpui::ExtensionRegistry;
//! use rich_model::{ExtensionCommand, ReadOnly, Transaction};
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
//! use rich_gpui::ExtensionRegistry;
//! use rich_model::{ExtensionCommand, ReadOnly, Transaction};
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

use crate::theme::RichTheme;
use gpui_kit::{AnyElement, App, Bounds, Pixels, Window};
use rich_layout::{InlineBoxMetrics, Insets, Size, pt_to_px};
use rich_model::{
    BlockId, BlockShape, BlockShapeKind, BlockSpec, ClipboardCustomFormatter, CommandEffectKind,
    CommandResolution, CustomPayload, DROPDOWN_KIND, Document, DocumentSelection, Edit,
    ExtensionCommand, ExtensionInvocation, FOOTNOTE_BODY_KIND, FOOTNOTE_REFERENCE_KIND,
    InlineObjectId, InlineSpec, InlineVAlign, ListId, PLACEHOLDER_KIND, ReadOnly, RichCommand,
    SharedStr, Transaction, VARIABLE_KIND, block_kinds,
};
use std::any::TypeId;
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

/// Typed context passed to [`InlineRenderer::render`].
pub struct InlineRenderContext<'render, T> {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: &'render str,
    pub data: &'render T,
    pub bounds: Bounds<Pixels>,
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
        InlineBoxMetrics {
            width: pt_to_px(72.0, zoom),
            height: pt_to_px(18.0, zoom),
            valign: InlineVAlign::Middle,
        }
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

/// Namespace within [`ExtensionRegistry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionNamespace {
    Block,
    Inline,
    Command,
}

impl fmt::Display for ExtensionNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block => formatter.write_str("block"),
            Self::Inline => formatter.write_str("inline"),
            Self::Command => formatter.write_str("command"),
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
}

impl<'document> ReadCommandContext<'document> {
    pub(crate) fn new(
        document: &'document Document,
        selection: Option<&'document DocumentSelection>,
    ) -> Self {
        Self {
            document,
            selection,
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
}

/// Read-only document context with [`BlockId`] reservation supplied to an edit command planner.
///
/// Reserving IDs alone does not mutate [`Document`], emit mutation events, or add undo entries.
pub struct EditCommandContext<'document> {
    document: &'document Document,
    selection: Option<&'document DocumentSelection>,
    next_block_id: u64,
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

    pub(crate) fn next_block_id(&self) -> u64 {
        self.next_block_id
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

pub(crate) trait ErasedBlockRenderer {
    fn clear_cache(&self);
    fn measure_leaf(
        &self,
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
        shape: BlockShapeKind,
        block_id: BlockId,
        kind: &str,
        data: &CustomPayload,
        bounds: Bounds<Pixels>,
        chrome: Insets,
        zoom: f32,
        theme: &RichTheme,
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
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> BlockCallbackResult<Size> {
        if let Err(detail) = self.check_shape(BlockShapeKind::Leaf) {
            return BlockCallbackResult::Invalid(detail);
        }
        match self
            .cache
            .with_resolved(data, |typed| self.renderer.measure_leaf(typed, max_width, zoom, window))
        {
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
        match self
            .cache
            .with_resolved(data, |typed| self.renderer.measure_chrome(typed, max_width, zoom, window))
        {
            Ok(insets) => BlockCallbackResult::Ok(insets),
            Err(message) => {
                BlockCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn render(
        &self,
        shape: BlockShapeKind,
        block_id: BlockId,
        kind: &str,
        data: &CustomPayload,
        bounds: Bounds<Pixels>,
        chrome: Insets,
        zoom: f32,
        theme: &RichTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> BlockCallbackResult<Option<CustomRenderOutput>> {
        if let Err(detail) = self.check_shape(shape) {
            return BlockCallbackResult::Invalid(detail);
        }
        match self.cache.with_resolved(data, |typed| {
            self.renderer.render(
                BlockRenderContext {
                    block_id,
                    kind,
                    data: typed,
                    bounds,
                    chrome,
                    zoom,
                    theme,
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
        data: &CustomPayload,
        zoom: f32,
        window: &Window,
    ) -> InlineCallbackResult<InlineBoxMetrics>;
    fn render(
        &self,
        block_id: BlockId,
        object_id: InlineObjectId,
        kind: &str,
        data: &CustomPayload,
        bounds: Bounds<Pixels>,
        zoom: f32,
        theme: &RichTheme,
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
        data: &CustomPayload,
        zoom: f32,
        window: &Window,
    ) -> InlineCallbackResult<InlineBoxMetrics> {
        match self
            .cache
            .with_resolved(data, |typed| self.renderer.measure(typed, zoom, window))
        {
            Ok(metrics) => InlineCallbackResult::Ok(metrics),
            Err(message) => {
                InlineCallbackResult::Invalid(ExtensionDiagnosticKind::InvalidPayload { message })
            }
        }
    }

    fn render(
        &self,
        block_id: BlockId,
        object_id: InlineObjectId,
        kind: &str,
        data: &CustomPayload,
        bounds: Bounds<Pixels>,
        zoom: f32,
        theme: &RichTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> InlineCallbackResult<Option<CustomRenderOutput>> {
        match self.cache.with_resolved(data, |typed| {
            self.renderer.render(
                InlineRenderContext {
                    block_id,
                    object_id,
                    kind,
                    data: typed,
                    bounds,
                    zoom,
                    theme,
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
    Read(ReadCommandOutcome),
    Edit {
        transaction: Option<Transaction>,
        next_block_id: u64,
    },
    Unregistered,
    Mismatched,
}

type ErasedReadHandler =
    Box<dyn Fn(&ExtensionInvocation, &ReadCommandContext<'_>) -> ReadCommandOutcome>;
type ErasedEditHandler =
    Box<dyn Fn(&ExtensionInvocation, &mut EditCommandContext<'_>) -> Option<Transaction>>;

enum RegisteredCommandHandler {
    Read(ErasedReadHandler),
    Edit(ErasedEditHandler),
}

struct RegisteredCommand {
    type_id: TypeId,
    effect: CommandEffectKind,
    handler: RegisteredCommandHandler,
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

/// Registry storing spec-bound [`BlockRenderer`]s, [`InlineRenderer`]s, and typed
/// [`ExtensionCommand`] handlers in separate namespaces.
#[derive(Default)]
pub struct ExtensionRegistry {
    blocks: HashMap<&'static str, Rc<dyn ErasedBlockRenderer>>,
    inlines: HashMap<&'static str, Rc<dyn ErasedInlineRenderer>>,
    commands: HashMap<&'static str, RegisteredCommand>,
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
                CommandExecutionResult::Read(handler(invocation, &context))
            }
            RegisteredCommandHandler::Edit(handler) => {
                let mut context = EditCommandContext::new(document, selection);
                let transaction = handler(invocation, &mut context);
                CommandExecutionResult::Edit {
                    transaction,
                    next_block_id: context.next_block_id(),
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
