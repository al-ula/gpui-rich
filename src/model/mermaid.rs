//! Pure-Rust document model, backend contract, typed commands, and queries for built-in
//! Mermaid diagram blocks (`BlockKind::Leaf` with [`MERMAID_KIND`]).

#[cfg(feature = "mermaid")]
pub mod compiler;
#[cfg(feature = "mermaid")]
pub use compiler::BuiltInMermaidBackend;

pub mod labels;
pub use labels::{
    MermaidLabelMatch, MermaidSvgLabel, SvgViewBox, extract_svg_labels,
    find_editable_mermaid_label, normalize_label_text, parse_svg_view_box, replace_mermaid_label,
};

use crate::model::block::{BlockId, BlockSpec, block_shape};
use crate::model::command::{Edit, ExtensionCommand, ReadOnly};
use crate::model::query::ExtensionQuery;
use crate::model::style::RgbaColor;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Persisted block kind identifier for built-in Mermaid diagram leaf blocks.
pub const MERMAID_KIND: &str = "mermaid";

/// Persisted payload for a Mermaid diagram block.
///
/// Ephemeral UI state (such as Diagram vs. Source view mode, draft edits, and render caches)
/// lives on the view extension rather than inside this struct so reading or toggling previews
/// never dirties the document or adds undo entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MermaidData {
    /// Raw Mermaid diagram source text.
    pub source: String,
    /// Optional caption displayed beneath the rendered diagram.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Optional maximum height in typographic points for the rendered diagram body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height_pt: Option<f32>,
}

impl MermaidData {
    /// Creates a Mermaid block payload with the given diagram source.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            caption: None,
            max_height_pt: None,
        }
    }

    /// Attaches an optional display caption.
    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// Sets an explicit maximum diagram body height in typographic points.
    pub fn with_max_height_pt(mut self, max_height_pt: f32) -> Self {
        self.max_height_pt = Some(max_height_pt);
        self
    }
}

/// Model-side specification binding [`MERMAID_KIND`] to `block_shape::Leaf` and [`MermaidData`].
pub struct MermaidSpec;

impl BlockSpec for MermaidSpec {
    const KIND: &'static str = MERMAID_KIND;
    type Shape = block_shape::Leaf;
    type Data = MermaidData;
}

/// Platform-agnostic color palette passed to [`MermaidBackend::render_svg`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MermaidThemePalette {
    pub dark: bool,
    pub background: RgbaColor,
    pub surface: RgbaColor,
    pub text: RgbaColor,
    pub muted_text: RgbaColor,
    pub border: RgbaColor,
    pub accent: RgbaColor,
}

impl MermaidThemePalette {
    /// Default light palette matching `RichTheme::light`.
    pub const fn light() -> Self {
        Self {
            dark: false,
            background: RgbaColor::WHITE,
            surface: RgbaColor::rgb(0xf6, 0xf8, 0xfa),
            text: RgbaColor::rgb(0x1f, 0x23, 0x28),
            muted_text: RgbaColor::rgb(0x59, 0x63, 0x6e),
            border: RgbaColor::rgb(0xd0, 0xd7, 0xde),
            accent: RgbaColor::rgb(0x0b, 0x57, 0xd0),
        }
    }

    /// Default dark palette matching `RichTheme::dark`.
    pub const fn dark() -> Self {
        Self {
            dark: true,
            background: RgbaColor::rgb(0x16, 0x1b, 0x22),
            surface: RgbaColor::rgb(0x1f, 0x26, 0x30),
            text: RgbaColor::rgb(0xe6, 0xed, 0xf3),
            muted_text: RgbaColor::rgb(0x8b, 0x94, 0x9e),
            border: RgbaColor::rgb(0x30, 0x36, 0x3d),
            accent: RgbaColor::rgb(0x58, 0xa6, 0xff),
        }
    }

    /// Computes a deterministic fingerprint for cache keying across theme switches.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// Compiled SVG document and intrinsic dimensions returned by a [`MermaidBackend`].
#[derive(Clone, Debug, PartialEq)]
pub struct MermaidSvg {
    /// Complete standalone SVG markup.
    pub svg: String,
    /// Intrinsic diagram width in CSS pixels (positive and finite).
    pub intrinsic_width: f32,
    /// Intrinsic diagram height in CSS pixels (positive and finite).
    pub intrinsic_height: f32,
}

impl MermaidSvg {
    /// Wraps raw SVG markup, extracting intrinsic dimensions from the root `<svg>` element when
    /// available and falling back to `fallback_width` x `fallback_height`.
    pub fn from_svg_with_fallback(
        svg: impl Into<String>,
        fallback_width: f32,
        fallback_height: f32,
    ) -> Self {
        let svg = svg.into();
        let (width, height) = parse_svg_intrinsic_size(&svg)
            .unwrap_or((fallback_width.max(1.0), fallback_height.max(1.0)));
        Self {
            svg,
            intrinsic_width: width,
            intrinsic_height: height,
        }
    }
}

/// Structured compile or layout failure returned by [`MermaidBackend::render_svg`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidRenderError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl MermaidRenderError {
    /// Creates an error with only a diagnostic message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }

    /// Creates an error with optional 1-based line and column coordinates.
    pub fn with_location(
        message: impl Into<String>,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Self {
        Self {
            message: message.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for MermaidRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(formatter, "line {line}, col {column}: {}", self.message)
            }
            (Some(line), None) => write!(formatter, "line {line}: {}", self.message),
            _ => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for MermaidRenderError {}

/// Pluggable compiler turning Mermaid diagram source into standalone resvg-compatible SVG.
pub trait MermaidBackend: Send + Sync + 'static {
    /// Compiles `source` under `palette` into standalone SVG markup and intrinsic dimensions.
    fn render_svg(
        &self,
        source: &str,
        palette: &MermaidThemePalette,
    ) -> Result<MermaidSvg, MermaidRenderError>;
}

/// Ephemeral display mode for a Mermaid diagram card in a view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MermaidViewMode {
    #[default]
    Diagram,
    Split,
    Source,
}

/// Ephemeral compilation and rasterization state reported by [`MermaidBlockStatusQuery`].
#[derive(Clone, Debug, PartialEq)]
pub enum MermaidRenderStatus {
    Idle,
    Rendering,
    Ready {
        intrinsic_width: f32,
        intrinsic_height: f32,
    },
    Error(MermaidRenderError),
}

/// Ephemeral per-block Mermaid status returned by [`MermaidBlockStatusQuery`].
#[derive(Clone, Debug, PartialEq)]
pub struct MermaidBlockStatus {
    pub mode: MermaidViewMode,
    pub render: MermaidRenderStatus,
}

/// Edit extension command inserting a new Mermaid diagram block at the current selection or end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertMermaidDiagram {
    pub source: String,
    pub caption: Option<String>,
}

impl InsertMermaidDiagram {
    /// Creates an insert command for `source` without a caption.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            caption: None,
        }
    }

    /// Attaches a caption to the inserted diagram.
    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

impl ExtensionCommand for InsertMermaidDiagram {
    const ID: &'static str = "mermaid.insert";
    type Effect = Edit;
}

/// Edit extension command updating the Mermaid source of an existing diagram block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateMermaidSource {
    pub block_id: BlockId,
    pub source: String,
}

impl ExtensionCommand for UpdateMermaidSource {
    const ID: &'static str = "mermaid.update_source";
    type Effect = Edit;
}

/// Edit extension command updating the optional caption of an existing Mermaid block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetMermaidCaption {
    pub block_id: BlockId,
    pub caption: Option<String>,
}

impl ExtensionCommand for SetMermaidCaption {
    const ID: &'static str = "mermaid.set_caption";
    type Effect = Edit;
}

/// Read-only extension command switching ephemeral Diagram vs. Source view mode for one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetMermaidViewMode {
    pub block_id: BlockId,
    pub mode: MermaidViewMode,
}

impl ExtensionCommand for SetMermaidViewMode {
    const ID: &'static str = "mermaid.set_view_mode";
    type Effect = ReadOnly;
}

/// Read-only extension command toggling ephemeral Diagram vs. Source view mode for one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleMermaidViewMode {
    pub block_id: BlockId,
}

impl ExtensionCommand for ToggleMermaidViewMode {
    const ID: &'static str = "mermaid.toggle_view_mode";
    type Effect = ReadOnly;
}

/// Read-only extension query reporting ephemeral view mode and render status for a Mermaid block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MermaidBlockStatusQuery {
    pub block_id: BlockId,
}

impl ExtensionQuery for MermaidBlockStatusQuery {
    const ID: &'static str = "mermaid.status";
    type Output = Option<MermaidBlockStatus>;
}

/// Parses positive finite `(width, height)` dimensions from the root `<svg ...>` element.
///
/// Prefers `viewBox="minX minY width height"` so percentage widths such as `width="100%"` still
/// yield exact intrinsic dimensions, falling back to numeric `width` and `height` attributes.
pub fn parse_svg_intrinsic_size(svg: &str) -> Option<(f32, f32)> {
    let open_start = find_svg_open_tag(svg)?;
    let tail = svg.get(open_start..)?;
    let open_end = tail.find('>')?;
    let header = tail.get(..open_end)?;

    if let Some(view_box) = extract_xml_attribute(header, "viewBox")
        && let Some((width, height)) = parse_view_box_dimensions(view_box)
    {
        return Some((width, height));
    }

    let width = extract_xml_attribute(header, "width").and_then(parse_svg_length_px)?;
    let height = extract_xml_attribute(header, "height").and_then(parse_svg_length_px)?;
    Some((width, height))
}

fn find_svg_open_tag(svg: &str) -> Option<usize> {
    let bytes = svg.as_bytes();
    let mut index = 0usize;
    while index + 4 <= bytes.len() {
        if bytes.get(index) == Some(&b'<')
            && svg
                .get(index + 1..index + 4)
                .is_some_and(|tag| tag.eq_ignore_ascii_case("svg"))
            && bytes
                .get(index + 4)
                .is_some_and(|next| next.is_ascii_whitespace() || *next == b'>')
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn extract_xml_attribute<'a>(tag_header: &'a str, attribute: &str) -> Option<&'a str> {
    let bytes = tag_header.as_bytes();
    let attr_len = attribute.len();
    let mut index = 0usize;
    while index + attr_len < bytes.len() {
        let preceded_by_space =
            index == 0 || bytes.get(index - 1).is_some_and(u8::is_ascii_whitespace);
        if preceded_by_space
            && tag_header
                .get(index..index + attr_len)
                .is_some_and(|slice| slice.eq_ignore_ascii_case(attribute))
        {
            let mut cursor = index + attr_len;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                    cursor += 1;
                }
                let quote = *bytes.get(cursor)?;
                if quote == b'"' || quote == b'\'' {
                    let value_start = cursor + 1;
                    let rest = tag_header.get(value_start..)?;
                    let value_end = rest.find(quote as char)?;
                    return rest.get(..value_end);
                }
            }
        }
        index += 1;
    }
    None
}

fn parse_view_box_dimensions(raw: &str) -> Option<(f32, f32)> {
    let mut parts = raw
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|part| !part.is_empty());
    let _min_x: f32 = parts.next()?.parse().ok()?;
    let _min_y: f32 = parts.next()?.parse().ok()?;
    let width: f32 = parts.next()?.parse().ok()?;
    let height: f32 = parts.next()?.parse().ok()?;
    if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
        Some((width, height))
    } else {
        None
    }
}

fn parse_svg_length_px(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if trimmed.ends_with('%') {
        return None;
    }
    let (number, factor) = if let Some(value) = trimmed.strip_suffix("px") {
        (value, 1.0f32)
    } else if let Some(value) = trimmed.strip_suffix("pt") {
        (value, 4.0 / 3.0)
    } else {
        (trimmed, 1.0f32)
    };
    let parsed: f32 = number.trim().parse().ok()?;
    let scaled = parsed * factor;
    if scaled.is_finite() && scaled > 0.0 {
        Some(scaled)
    } else {
        None
    }
}

/// Computes a stable 64-bit hash for diagram source text.
pub fn hash_mermaid_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}
