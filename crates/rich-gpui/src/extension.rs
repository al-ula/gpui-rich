//! Extensibility Seam 3: custom block/inline node extensions (`NodeExtension`),
//! typed adapters (`TypedBlockExtension`, `TypedInlineExtension`), hosted GPUI `AnyElement`
//! rendering, custom command handlers, and [`ExtensionRegistry`].

use crate::theme::RichTheme;
use gpui_kit::{AnyElement, App, Bounds, Pixels, Window};
use rich_layout::{InlineBoxMetrics, Insets, Size, pt_to_px};
use rich_model::{
    BlockId, ClipboardCustomFormatter, CustomData, CustomPayload, Document, DocumentSelection,
    InlineObjectId, InlineVAlign, Transaction,
};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::rc::Rc;

/// Direct immediate-mode paint callback invoked for a custom node.
pub type DirectPaintCallback = Box<dyn FnOnce(Bounds<Pixels>, &RichTheme, &mut Window, &mut App)>;

/// Rendering output returned by [`NodeExtension::render_block`] or [`NodeExtension::render_inline`].
pub enum CustomRenderOutput {
    /// Hosted interactive GPUI element (including `Entity<V>` views converted with `.into_any_element()`)
    /// laid out and prepainted at the node's computed canvas rectangle.
    Element(AnyElement),
    /// Direct immediate-mode paint callback invoked during element painting.
    DirectPaint(DirectPaintCallback),
}

/// Context passed to [`NodeExtension::render_block`].
pub struct CustomBlockRenderContext<'render> {
    pub block_id: BlockId,
    pub kind: &'render str,
    pub data: &'render CustomPayload,
    pub bounds: Bounds<Pixels>,
    pub chrome: Insets,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Context passed to [`NodeExtension::render_inline`].
pub struct CustomInlineRenderContext<'render> {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: &'render str,
    pub data: &'render CustomPayload,
    pub bounds: Bounds<Pixels>,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Context passed to [`TypedBlockExtension::render_block`].
pub struct TypedBlockRenderContext<'render, T> {
    pub block_id: BlockId,
    pub kind: &'render str,
    pub data: &'render T,
    pub bounds: Bounds<Pixels>,
    pub chrome: Insets,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Context passed to [`TypedInlineExtension::render_inline`].
pub struct TypedInlineRenderContext<'render, T> {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: &'render str,
    pub data: &'render T,
    pub bounds: Bounds<Pixels>,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Handler registered in [`ExtensionRegistry`] for an open semantic `kind` on [`rich_model::BlockKind`]
/// and/or [`rich_model::InlineObject::Custom`].
pub trait NodeExtension {
    /// Measures a custom leaf block (`BlockKind::Leaf`) inside `max_width` pixels at `zoom`.
    fn measure_block(
        &self,
        _data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    /// Measures chrome insets (in pixels) reserved around a custom text, container, or grid block.
    fn measure_chrome(
        &self,
        _data: &CustomPayload,
        _max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Insets {
        Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
    }

    /// Measures a custom inline object at `zoom`.
    fn measure_inline(
        &self,
        _data: &CustomPayload,
        zoom: f32,
        _window: &Window,
    ) -> InlineBoxMetrics {
        InlineBoxMetrics {
            width: pt_to_px(72.0, zoom),
            height: pt_to_px(18.0, zoom),
            valign: InlineVAlign::Middle,
        }
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible custom block.
    ///
    /// Leaf blocks (`BlockKind::Leaf`) paint on Layer 4 (foreground). Text, container, and grid
    /// blocks (`BlockKind::Text`, `BlockKind::Container`, `BlockKind::Grid`) paint on Layer 1
    /// (chrome behind editable text and children).
    fn render_block(
        &self,
        _context: CustomBlockRenderContext<'_>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible custom inline object.
    fn render_inline(
        &self,
        _context: CustomInlineRenderContext<'_>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Optional plain-text clipboard fallback for a custom block.
    fn plain_text_block(&self, _data: &CustomPayload) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom block.
    fn html_block(&self, _data: &CustomPayload) -> Option<String> {
        None
    }

    /// Optional plain-text clipboard fallback for a custom inline object.
    fn plain_text_inline(&self, _data: &CustomPayload) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom inline object.
    fn html_inline(&self, _data: &CustomPayload) -> Option<String> {
        None
    }
}

/// Strongly typed adapter for custom block extensions whose [`CustomPayload`] decodes to `Data`.
pub trait TypedBlockExtension {
    type Data: CustomData + Clone + DeserializeOwned + 'static;

    /// Measures a custom leaf block (`BlockKind::Leaf`) inside `max_width` pixels at `zoom`.
    fn measure_block(
        &self,
        _data: &Self::Data,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    /// Measures chrome insets (in pixels) reserved around a custom text, container, or grid block.
    fn measure_chrome(
        &self,
        _data: &Self::Data,
        _max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Insets {
        Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible custom block.
    fn render_block(
        &self,
        _context: TypedBlockRenderContext<'_, Self::Data>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Optional plain-text clipboard fallback for a custom block.
    fn plain_text_block(&self, _data: &Self::Data) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom block.
    fn html_block(&self, _data: &Self::Data) -> Option<String> {
        None
    }
}

/// Strongly typed adapter for custom inline extensions whose [`CustomPayload`] decodes to `Data`.
pub trait TypedInlineExtension {
    type Data: CustomData + Clone + DeserializeOwned + 'static;

    /// Measures a custom inline object at `zoom`.
    fn measure_inline(
        &self,
        _data: &Self::Data,
        zoom: f32,
        _window: &Window,
    ) -> InlineBoxMetrics {
        InlineBoxMetrics {
            width: pt_to_px(72.0, zoom),
            height: pt_to_px(18.0, zoom),
            valign: InlineVAlign::Middle,
        }
    }

    /// Produces a hosted GPUI [`AnyElement`] or direct paint callback for a visible custom inline object.
    fn render_inline(
        &self,
        _context: TypedInlineRenderContext<'_, Self::Data>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        None
    }

    /// Optional plain-text clipboard fallback for a custom inline object.
    fn plain_text_inline(&self, _data: &Self::Data) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom inline object.
    fn html_inline(&self, _data: &Self::Data) -> Option<String> {
        None
    }
}

struct TypedBlockExtensionAdapter<E>(E);

impl<E: TypedBlockExtension> NodeExtension for TypedBlockExtensionAdapter<E> {
    fn measure_block(
        &self,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> Size {
        if let Some(typed) = data.as_typed::<E::Data>() {
            self.0.measure_block(typed.as_ref(), max_width, zoom, window)
        } else {
            Size::new(max_width, pt_to_px(44.0, zoom))
        }
    }

    fn measure_chrome(
        &self,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
        window: &Window,
    ) -> Insets {
        if let Some(typed) = data.as_typed::<E::Data>() {
            self.0.measure_chrome(typed.as_ref(), max_width, zoom, window)
        } else {
            Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
        }
    }

    fn render_block(
        &self,
        context: CustomBlockRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let typed = context.data.as_typed::<E::Data>()?;
        self.0.render_block(
            TypedBlockRenderContext {
                block_id: context.block_id,
                kind: context.kind,
                data: typed.as_ref(),
                bounds: context.bounds,
                chrome: context.chrome,
                zoom: context.zoom,
                theme: context.theme,
            },
            window,
            cx,
        )
    }

    fn plain_text_block(&self, data: &CustomPayload) -> Option<String> {
        let typed = data.as_typed::<E::Data>()?;
        self.0.plain_text_block(typed.as_ref())
    }

    fn html_block(&self, data: &CustomPayload) -> Option<String> {
        let typed = data.as_typed::<E::Data>()?;
        self.0.html_block(typed.as_ref())
    }
}

struct TypedInlineExtensionAdapter<E>(E);

impl<E: TypedInlineExtension> NodeExtension for TypedInlineExtensionAdapter<E> {
    fn measure_inline(
        &self,
        data: &CustomPayload,
        zoom: f32,
        window: &Window,
    ) -> InlineBoxMetrics {
        if let Some(typed) = data.as_typed::<E::Data>() {
            self.0.measure_inline(typed.as_ref(), zoom, window)
        } else {
            InlineBoxMetrics {
                width: pt_to_px(72.0, zoom),
                height: pt_to_px(18.0, zoom),
                valign: InlineVAlign::Middle,
            }
        }
    }

    fn render_inline(
        &self,
        context: CustomInlineRenderContext<'_>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let typed = context.data.as_typed::<E::Data>()?;
        self.0.render_inline(
            TypedInlineRenderContext {
                block_id: context.block_id,
                object_id: context.object_id,
                kind: context.kind,
                data: typed.as_ref(),
                bounds: context.bounds,
                zoom: context.zoom,
                theme: context.theme,
            },
            window,
            cx,
        )
    }

    fn plain_text_inline(&self, data: &CustomPayload) -> Option<String> {
        let typed = data.as_typed::<E::Data>()?;
        self.0.plain_text_inline(typed.as_ref())
    }

    fn html_inline(&self, data: &CustomPayload) -> Option<String> {
        let typed = data.as_typed::<E::Data>()?;
        self.0.html_inline(typed.as_ref())
    }
}

/// Execution outcome returned by a custom command handler registered on [`ExtensionRegistry`].
pub enum CustomCommandResult {
    /// Apply a reversible document [`Transaction`] through History, layout sync, and host events.
    Transaction(Box<Transaction>),
    /// Executed without mutating the document.
    Executed,
    /// No-op.
    NoOp,
}

/// Handler invoked when [`rich_model::RichCommand::Custom`] matches a registered command `id`.
pub type CustomCommandHandler = Box<
    dyn Fn(&CustomPayload, &mut Document, Option<&DocumentSelection>) -> CustomCommandResult,
>;

/// Registry storing [`NodeExtension`] renderers/measurers and [`CustomCommandHandler`] callbacks.
#[derive(Default)]
pub struct ExtensionRegistry {
    nodes: HashMap<String, Rc<dyn NodeExtension>>,
    commands: HashMap<String, CustomCommandHandler>,
}

impl ExtensionRegistry {
    /// Creates an empty [`ExtensionRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a [`NodeExtension`] for `kind`, replacing any prior handler for the same `kind`.
    pub fn register_node(
        &mut self,
        kind: impl Into<String>,
        extension: impl NodeExtension + 'static,
    ) {
        self.nodes.insert(kind.into(), Rc::new(extension));
    }

    /// Registers a strongly typed [`TypedBlockExtension`] for `kind`.
    pub fn register_typed_block(
        &mut self,
        kind: impl Into<String>,
        extension: impl TypedBlockExtension + 'static,
    ) {
        self.register_node(kind, TypedBlockExtensionAdapter(extension));
    }

    /// Registers a strongly typed [`TypedInlineExtension`] for `kind`.
    pub fn register_typed_inline(
        &mut self,
        kind: impl Into<String>,
        extension: impl TypedInlineExtension + 'static,
    ) {
        self.register_node(kind, TypedInlineExtensionAdapter(extension));
    }

    /// Removes the [`NodeExtension`] registered for `kind`.
    pub fn unregister_node(&mut self, kind: &str) -> bool {
        self.nodes.remove(kind).is_some()
    }

    /// Looks up the registered [`NodeExtension`] handle for `kind`.
    pub fn get_node(&self, kind: &str) -> Option<Rc<dyn NodeExtension>> {
        self.nodes.get(kind).cloned()
    }

    /// Registers a handler for [`rich_model::RichCommand::Custom`] matching `id`.
    pub fn register_command(
        &mut self,
        id: impl Into<String>,
        handler: impl Fn(
            &CustomPayload,
            &mut Document,
            Option<&DocumentSelection>,
        ) -> CustomCommandResult
        + 'static,
    ) {
        self.commands.insert(id.into(), Box::new(handler));
    }

    /// Removes a previously registered custom command handler.
    pub fn unregister_command(&mut self, id: &str) -> bool {
        self.commands.remove(id).is_some()
    }

    /// Executes a registered custom command handler if one exists for `id`.
    pub fn execute_command(
        &self,
        id: &str,
        payload: &CustomPayload,
        document: &mut Document,
        selection: Option<&DocumentSelection>,
    ) -> Option<CustomCommandResult> {
        let handler = self.commands.get(id)?;
        Some(handler(payload, document, selection))
    }
}

impl ClipboardCustomFormatter for ExtensionRegistry {
    fn plain_text_block(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.nodes.get(kind)?.plain_text_block(data)
    }

    fn html_block(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.nodes.get(kind)?.html_block(data)
    }

    fn plain_text_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.nodes.get(kind)?.plain_text_inline(data)
    }

    fn html_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.nodes.get(kind)?.html_inline(data)
    }
}
