//! Extensibility Seam 3: custom block/inline node extensions (`NodeExtension`),
//! hosted GPUI `AnyElement` rendering, custom command handlers, and [`ExtensionRegistry`].

use crate::theme::RichTheme;
use gpui_kit::{AnyElement, App, Bounds, Pixels, Window};
use rich_layout::{InlineBoxMetrics, Size, pt_to_px};
use rich_model::{
    BlockId, ClipboardCustomFormatter, Document, DocumentSelection, InlineObjectId, InlineVAlign,
    Transaction,
};
use std::collections::HashMap;
use std::rc::Rc;

/// Direct immediate-mode paint callback invoked on Layer 4 for a custom node.
pub type DirectPaintCallback = Box<dyn FnOnce(Bounds<Pixels>, &RichTheme, &mut Window, &mut App)>;

/// Rendering output returned by [`NodeExtension::render_block`] or [`NodeExtension::render_inline`].
pub enum CustomRenderOutput {
    /// Hosted interactive GPUI element (including `Entity<V>` views converted with `.into_any_element()`)
    /// laid out and prepainted at the node's computed canvas rectangle.
    Element(AnyElement),
    /// Direct immediate-mode paint callback invoked on Layer 4.
    DirectPaint(DirectPaintCallback),
}

/// Context passed to [`NodeExtension::render_block`].
pub struct CustomBlockRenderContext<'render> {
    pub block_id: BlockId,
    pub kind: &'render str,
    pub data: &'render serde_json::Value,
    pub bounds: Bounds<Pixels>,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Context passed to [`NodeExtension::render_inline`].
pub struct CustomInlineRenderContext<'render> {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: &'render str,
    pub data: &'render serde_json::Value,
    pub bounds: Bounds<Pixels>,
    pub zoom: f32,
    pub theme: &'render RichTheme,
}

/// Handler registered in [`ExtensionRegistry`] for a custom `kind` on [`rich_model::BlockKind::Custom`]
/// and/or [`rich_model::InlineObject::Custom`].
pub trait NodeExtension {
    /// Measures a custom block node inside `max_width` pixels at `zoom`.
    fn measure_block(
        &self,
        _data: &serde_json::Value,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    /// Measures a custom inline object at `zoom`.
    fn measure_inline(
        &self,
        _data: &serde_json::Value,
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
    fn plain_text_block(&self, _data: &serde_json::Value) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom block.
    fn html_block(&self, _data: &serde_json::Value) -> Option<String> {
        None
    }

    /// Optional plain-text clipboard fallback for a custom inline object.
    fn plain_text_inline(&self, _data: &serde_json::Value) -> Option<String> {
        None
    }

    /// Optional HTML clipboard fallback for a custom inline object.
    fn html_inline(&self, _data: &serde_json::Value) -> Option<String> {
        None
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
    dyn Fn(&serde_json::Value, &mut Document, Option<&DocumentSelection>) -> CustomCommandResult,
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
            &serde_json::Value,
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
        payload: &serde_json::Value,
        document: &mut Document,
        selection: Option<&DocumentSelection>,
    ) -> Option<CustomCommandResult> {
        let handler = self.commands.get(id)?;
        Some(handler(payload, document, selection))
    }
}

impl ClipboardCustomFormatter for ExtensionRegistry {
    fn plain_text_block(&self, kind: &str, data: &serde_json::Value) -> Option<String> {
        self.nodes.get(kind)?.plain_text_block(data)
    }

    fn html_block(&self, kind: &str, data: &serde_json::Value) -> Option<String> {
        self.nodes.get(kind)?.html_block(data)
    }

    fn plain_text_inline(&self, kind: &str, data: &serde_json::Value) -> Option<String> {
        self.nodes.get(kind)?.plain_text_inline(data)
    }

    fn html_inline(&self, kind: &str, data: &serde_json::Value) -> Option<String> {
        self.nodes.get(kind)?.html_inline(data)
    }
}
