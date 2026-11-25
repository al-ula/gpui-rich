//! Pure-Rust document model, backend contract, typed commands, and queries for built-in
//! LaTeX equations:
//! - Display equation blocks (`BlockKind::Leaf` with [`EQUATION_BLOCK_KIND`]).
//! - Inline equation objects (`InlineObject::Custom` with [`INLINE_EQUATION_KIND`]).

#[cfg(feature = "equation")]
mod compiler;
#[cfg(feature = "equation")]
pub use compiler::BuiltInEquationBackend;

use crate::model::block::{BlockId, BlockSpec, block_shape};
use crate::model::command::{Edit, ExtensionCommand, ReadOnly};
use crate::model::inline::{InlineObjectId, InlineSpec};
use crate::model::query::ExtensionQuery;
use crate::model::style::RgbaColor;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Persisted block kind identifier for built-in display equation leaf blocks.
pub const EQUATION_BLOCK_KIND: &str = "equation";

/// Persisted inline object kind identifier for built-in inline equations.
pub const INLINE_EQUATION_KIND: &str = "inline_equation";

/// Persisted payload for a display equation block (`BlockKind::Leaf`).
///
/// Ephemeral UI state (such as edit mode, draft text, and SVG/raster caches) lives on the view
/// extension so opening or closing the editor never dirties the document or undo history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquationBlockData {
    /// Raw LaTeX math source text.
    pub source: String,
    /// Optional right-aligned equation number or tag (for example, `"1"` or `"2.1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numbering: Option<String>,
    /// Optional semantic reference label (for example, `"eq:euler"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional base font size override in typographic points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_pt: Option<f32>,
}

impl EquationBlockData {
    /// Creates a display equation block payload with the given LaTeX source.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            numbering: None,
            label: None,
            font_size_pt: None,
        }
    }

    /// Attaches a right-aligned equation number or tag.
    pub fn with_numbering(mut self, numbering: impl Into<String>) -> Self {
        self.numbering = Some(numbering.into());
        self
    }

    /// Attaches a semantic reference label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets an explicit base font size in typographic points.
    pub fn with_font_size_pt(mut self, font_size_pt: f32) -> Self {
        self.font_size_pt = Some(font_size_pt);
        self
    }
}

/// Model-side specification binding [`EQUATION_BLOCK_KIND`] to `block_shape::Leaf` and
/// [`EquationBlockData`].
pub struct EquationBlockSpec;

impl BlockSpec for EquationBlockSpec {
    const KIND: &'static str = EQUATION_BLOCK_KIND;
    type Shape = block_shape::Leaf;
    type Data = EquationBlockData;
}

/// Persisted payload for an inline equation object (`InlineObject::Custom`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineEquationData {
    /// Raw LaTeX math source text.
    pub source: String,
}

impl InlineEquationData {
    /// Creates an inline equation payload with the given LaTeX source.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// Model-side specification binding [`INLINE_EQUATION_KIND`] to [`InlineEquationData`].
pub struct InlineEquationSpec;

impl InlineSpec for InlineEquationSpec {
    const KIND: &'static str = INLINE_EQUATION_KIND;
    type Data = InlineEquationData;
}

/// TeX math presentation style passed to [`EquationBackend::render_svg`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EquationDisplayMode {
    /// Compact `\textstyle` for equations embedded within prose lines.
    Inline,
    /// Full-height `\displaystyle` for standalone display equation blocks.
    #[default]
    Display,
}

/// Platform-agnostic rendering options passed to [`EquationBackend::render_svg`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EquationRenderOptions {
    pub mode: EquationDisplayMode,
    /// Base font size in typographic points (positive and finite).
    pub font_size_pt: f32,
    /// Default foreground ink color.
    pub color: RgbaColor,
}

impl EquationRenderOptions {
    /// Creates options for an inline (`\textstyle`) equation.
    pub const fn inline(font_size_pt: f32, color: RgbaColor) -> Self {
        Self {
            mode: EquationDisplayMode::Inline,
            font_size_pt,
            color,
        }
    }

    /// Creates options for a standalone display (`\displaystyle`) equation.
    pub const fn display(font_size_pt: f32, color: RgbaColor) -> Self {
        Self {
            mode: EquationDisplayMode::Display,
            font_size_pt,
            color,
        }
    }

    /// Computes a deterministic cache key covering mode, quantized font size, and foreground color.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.mode.hash(&mut hasher);
        (self.font_size_pt * 10.0)
            .round()
            .to_bits()
            .hash(&mut hasher);
        self.color.hash(&mut hasher);
        hasher.finish()
    }
}

/// Compiled standalone SVG artwork and TeX typographic metrics in points.
#[derive(Clone, Debug, PartialEq)]
pub struct EquationSvg {
    /// Complete standalone resvg-compatible SVG document.
    pub svg: String,
    /// Physical width in typographic points (positive and finite).
    pub width_pt: f32,
    /// Total physical height (`ascent + depth`) in typographic points (positive and finite).
    pub height_pt: f32,
    /// Distance below the mathematical baseline in typographic points (nonnegative, `<= height_pt`).
    pub depth_pt: f32,
}

impl EquationSvg {
    /// Returns the ascent above the mathematical baseline in typographic points.
    pub fn ascent_pt(&self) -> f32 {
        (self.height_pt - self.depth_pt).max(0.0)
    }
}

/// Structured compile or layout error returned by [`EquationBackend::render_svg`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquationRenderError {
    pub message: String,
    /// Optional byte/character offset in the LaTeX source where the error occurred.
    pub offset: Option<usize>,
}

impl EquationRenderError {
    /// Creates an error with only a diagnostic message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            offset: None,
        }
    }

    /// Creates an error with an optional source offset.
    pub fn with_offset(message: impl Into<String>, offset: Option<usize>) -> Self {
        Self {
            message: message.into(),
            offset,
        }
    }
}

impl fmt::Display for EquationRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(formatter, "offset {offset}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for EquationRenderError {}

/// Pluggable compiler turning LaTeX math source into standalone resvg-compatible SVG and metrics.
pub trait EquationBackend: Send + Sync + 'static {
    /// Compiles `source` under `options` into standalone SVG markup and typographic metrics.
    fn render_svg(
        &self,
        source: &str,
        options: &EquationRenderOptions,
    ) -> Result<EquationSvg, EquationRenderError>;
}

/// Ephemeral display mode for a display equation block in a view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EquationViewMode {
    #[default]
    Rendered,
    Edit,
}

/// Ephemeral compilation state reported by [`EquationBlockStatusQuery`].
#[derive(Clone, Debug, PartialEq)]
pub enum EquationRenderStatus {
    Idle,
    Rendering,
    Ready {
        width_pt: f32,
        height_pt: f32,
        depth_pt: f32,
    },
    Error(EquationRenderError),
}

/// Ephemeral per-block status returned by [`EquationBlockStatusQuery`].
#[derive(Clone, Debug, PartialEq)]
pub struct EquationBlockStatus {
    pub mode: EquationViewMode,
    pub render: EquationRenderStatus,
}

/// Edit extension command inserting a new display equation block after the active block (or end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertEquationBlock {
    pub source: String,
    pub numbering: Option<String>,
    pub label: Option<String>,
}

impl InsertEquationBlock {
    /// Creates an insert command for `source` without numbering or label.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            numbering: None,
            label: None,
        }
    }

    /// Attaches a right-aligned equation number or tag.
    pub fn with_numbering(mut self, numbering: impl Into<String>) -> Self {
        self.numbering = Some(numbering.into());
        self
    }

    /// Attaches a semantic reference label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl ExtensionCommand for InsertEquationBlock {
    const ID: &'static str = "equation.insert_block";
    type Effect = Edit;
}

/// Edit extension command updating the LaTeX source of an existing display equation block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateEquationBlock {
    pub block_id: BlockId,
    pub source: String,
}

impl ExtensionCommand for UpdateEquationBlock {
    const ID: &'static str = "equation.update_block";
    type Effect = Edit;
}

/// Edit extension command updating the optional numbering tag of an existing display equation block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetEquationNumbering {
    pub block_id: BlockId,
    pub numbering: Option<String>,
}

impl ExtensionCommand for SetEquationNumbering {
    const ID: &'static str = "equation.set_numbering";
    type Effect = Edit;
}

/// Edit extension command inserting a new inline equation at the active text selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertInlineEquation {
    pub source: String,
}

impl InsertInlineEquation {
    /// Creates an insert command for an inline equation.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

impl ExtensionCommand for InsertInlineEquation {
    const ID: &'static str = "equation.insert_inline";
    type Effect = Edit;
}

/// Edit extension command updating the LaTeX source of an existing inline equation object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInlineEquation {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub source: String,
}

impl ExtensionCommand for UpdateInlineEquation {
    const ID: &'static str = "equation.update_inline";
    type Effect = Edit;
}

/// Read-only extension command switching ephemeral Rendered vs. Edit mode for one equation block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetEquationViewMode {
    pub block_id: BlockId,
    pub mode: EquationViewMode,
}

impl ExtensionCommand for SetEquationViewMode {
    const ID: &'static str = "equation.set_view_mode";
    type Effect = ReadOnly;
}

/// Read-only extension command toggling ephemeral Rendered vs. Edit mode for one equation block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleEquationViewMode {
    pub block_id: BlockId,
}

impl ExtensionCommand for ToggleEquationViewMode {
    const ID: &'static str = "equation.toggle_view_mode";
    type Effect = ReadOnly;
}

/// Read-only extension command opening the anchored inline equation popover editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenInlineEquationEditor {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
}

impl ExtensionCommand for OpenInlineEquationEditor {
    const ID: &'static str = "equation.open_inline_editor";
    type Effect = ReadOnly;
}

/// Read-only extension command closing any active anchored inline equation popover editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseInlineEquationEditor;

impl ExtensionCommand for CloseInlineEquationEditor {
    const ID: &'static str = "equation.close_inline_editor";
    type Effect = ReadOnly;
}

/// Read-only extension query reporting ephemeral view mode and render status for an equation block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EquationBlockStatusQuery {
    pub block_id: BlockId,
}

impl ExtensionQuery for EquationBlockStatusQuery {
    const ID: &'static str = "equation.status";
    type Output = Option<EquationBlockStatus>;
}

/// Computes a stable 64-bit hash for LaTeX equation source text.
pub fn hash_equation_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}
