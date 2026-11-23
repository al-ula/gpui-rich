//! Ephemeral decoration contracts feeding paint Layers 2, 4, and 5 without mutating [`Document`].
//!
//! Consumers implement [`DecorationProvider`] and emit spans or point overlays through a checked
//! [`DecorationWriter`] scoped to the current text-bearing leaf block. Invalid bounds, reversed
//! ranges, and offsets that do not fall on UTF-8 character boundaries are rejected with
//! [`DecorationError`] before paint.
//!
//! Long-lived annotations should be stored as document positions or anchors and projected to
//! current block-local byte offsets during [`DecorationProvider::collect_decorations`]. Consumers
//! that cache block-local ranges must key them by `(block.id, block.version)`.

use crate::block::{BlockNode, Document};
use crate::selection::{Affinity, DocumentSelection};
use crate::style::{RgbaColor, UnderlineKind};
use std::fmt;
use std::ops::Range;

/// UTF-8 byte offset scoped to a single text-bearing leaf block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockByteOffset(pub usize);

impl BlockByteOffset {
    /// Creates a block-local byte offset.
    pub const fn new(byte_offset: usize) -> Self {
        Self(byte_offset)
    }

    /// Returns the raw byte offset.
    pub const fn get(self) -> usize {
        self.0
    }
}

/// Half-open UTF-8 byte range scoped to a single text-bearing leaf block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockByteRange {
    pub start: BlockByteOffset,
    pub end: BlockByteOffset,
}

impl BlockByteRange {
    /// Creates a block-local byte range from distinct start and end offsets.
    pub const fn new(start: BlockByteOffset, end: BlockByteOffset) -> Self {
        Self { start, end }
    }

    /// Converts a standard `Range<usize>` into a [`BlockByteRange`].
    pub const fn from_usize_range(range: Range<usize>) -> Self {
        Self {
            start: BlockByteOffset(range.start),
            end: BlockByteOffset(range.end),
        }
    }
}

/// Validation failure returned by [`DecorationWriter`] when a decoration cannot be applied to the
/// current block's UTF-8 text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationError {
    /// The target block does not carry [`crate::InlineContent`].
    NonTextBlock,
    /// A byte offset exceeded `text.len()`.
    OutOfBounds { offset: usize, len: usize },
    /// A range had `start > end`.
    InvalidOrder { start: usize, end: usize },
    /// A byte offset did not lie on a UTF-8 character boundary.
    InvalidUtf8Boundary { offset: usize },
}

impl fmt::Display for DecorationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonTextBlock => formatter.write_str("block does not contain inline text"),
            Self::OutOfBounds { offset, len } => {
                write!(formatter, "byte offset {offset} out of bounds for length {len}")
            }
            Self::InvalidOrder { start, end } => {
                write!(formatter, "invalid byte range order: {start}..{end}")
            }
            Self::InvalidUtf8Boundary { offset } => {
                write!(formatter, "byte offset {offset} is not a UTF-8 character boundary")
            }
        }
    }
}

impl std::error::Error for DecorationError {}

/// Visual styling properties for a transient inline decoration submitted to [`DecorationWriter`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineDecorationStyle {
    /// Optional Layer 2 background highlight (for example find-in-document or comment ranges).
    pub background: Option<RgbaColor>,
    /// Optional Layer 4 foreground glyph color override (for example code syntax highlighting).
    pub foreground: Option<RgbaColor>,
    /// Optional Layer 4 underline (for example spellcheck wavy red underlines).
    pub underline: Option<UnderlineKind>,
    /// Optional Layer 4 strikethrough overlay.
    pub strikethrough: bool,
    /// Priority tie-breaker when overlapping decorations target the same visual channel (higher wins).
    pub priority: i32,
}

impl InlineDecorationStyle {
    /// Creates an empty inline decoration style.
    pub const fn new() -> Self {
        Self {
            background: None,
            foreground: None,
            underline: None,
            strikethrough: false,
            priority: 0,
        }
    }

    /// Sets the Layer 2 background highlight color.
    pub const fn with_background(mut self, color: RgbaColor) -> Self {
        self.background = Some(color);
        self
    }

    /// Sets the Layer 4 foreground glyph color override.
    pub const fn with_foreground(mut self, color: RgbaColor) -> Self {
        self.foreground = Some(color);
        self
    }

    /// Sets the Layer 4 underline style.
    pub const fn with_underline(mut self, underline: UnderlineKind) -> Self {
        self.underline = Some(underline);
        self
    }

    /// Enables or disables a Layer 4 strikethrough overlay.
    pub const fn with_strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }

    /// Sets the sorting priority when decorations overlap.
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// Validated transient inline overlay scoped to a UTF-8 byte range inside one text leaf block.
///
/// Constructed exclusively through [`DecorationWriter::push_inline`].
#[derive(Clone, Debug, PartialEq)]
pub struct InlineDecoration {
    range: Range<usize>,
    pub background: Option<RgbaColor>,
    pub foreground: Option<RgbaColor>,
    pub underline: Option<UnderlineKind>,
    pub strikethrough: bool,
    pub priority: i32,
}

impl InlineDecoration {
    /// Returns the validated UTF-8 byte range within the target leaf block's `InlineContent`.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }
}

/// Payload for a validated Layer 5 point overlay anchored inside a single text leaf block.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayDecorationKind {
    /// Inline ghost text (such as AI completions) rendered without modifying the document.
    GhostText {
        text: String,
        color: Option<RgbaColor>,
    },
    /// Remote collaborator caret rendered with an optional name badge.
    RemoteCaret {
        color: RgbaColor,
        label: Option<String>,
    },
}

/// Validated transient Layer 5 point overlay anchored inside one text leaf block.
///
/// Constructed exclusively through [`DecorationWriter::push_ghost_text`] and
/// [`DecorationWriter::push_remote_caret`].
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayDecoration {
    offset: BlockByteOffset,
    affinity: Affinity,
    kind: OverlayDecorationKind,
}

impl OverlayDecoration {
    /// Returns the validated UTF-8 byte offset inside the target leaf block.
    pub const fn offset(&self) -> BlockByteOffset {
        self.offset
    }

    /// Returns the line-wrap placement [`Affinity`].
    pub const fn affinity(&self) -> Affinity {
        self.affinity
    }

    /// Returns the overlay kind and visual attributes.
    pub fn kind(&self) -> &OverlayDecorationKind {
        &self.kind
    }
}

/// Aggregated validated transient decorations produced for one visible leaf block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockDecorations {
    pub inline: Vec<InlineDecoration>,
    pub overlays: Vec<OverlayDecoration>,
}

impl BlockDecorations {
    /// Returns true when neither inline nor overlay decorations are present.
    pub fn is_empty(&self) -> bool {
        self.inline.is_empty() && self.overlays.is_empty()
    }

    /// Appends all decorations from `other` into `self`.
    pub fn extend(&mut self, other: Self) {
        self.inline.extend(other.inline);
        self.overlays.extend(other.overlays);
    }
}

/// Checked block-scoped writer validating UTF-8 boundaries, bounds, and range order before paint.
pub struct DecorationWriter<'document> {
    text: Option<&'document str>,
    decorations: BlockDecorations,
}

impl<'document> DecorationWriter<'document> {
    /// Creates a checked writer bound to `block`.
    pub fn new(block: &'document BlockNode) -> Self {
        Self {
            text: block
                .kind
                .inline_content()
                .map(|content| content.text.as_str()),
            decorations: BlockDecorations::default(),
        }
    }

    fn validate_offset(&self, offset: BlockByteOffset) -> Result<usize, DecorationError> {
        let Some(text) = self.text else {
            return Err(DecorationError::NonTextBlock);
        };
        let value = offset.get();
        if value > text.len() {
            return Err(DecorationError::OutOfBounds {
                offset: value,
                len: text.len(),
            });
        }
        if !text.is_char_boundary(value) {
            return Err(DecorationError::InvalidUtf8Boundary { offset: value });
        }
        Ok(value)
    }

    /// Validates `range` against the current block and appends an inline decoration.
    pub fn push_inline(
        &mut self,
        range: BlockByteRange,
        style: InlineDecorationStyle,
    ) -> Result<(), DecorationError> {
        let start = range.start.get();
        let end = range.end.get();
        if start > end {
            return Err(DecorationError::InvalidOrder { start, end });
        }
        let valid_start = self.validate_offset(range.start)?;
        let valid_end = self.validate_offset(range.end)?;
        if valid_start == valid_end {
            return Ok(());
        }
        self.decorations.inline.push(InlineDecoration {
            range: valid_start..valid_end,
            background: style.background,
            foreground: style.foreground,
            underline: style.underline,
            strikethrough: style.strikethrough,
            priority: style.priority,
        });
        Ok(())
    }

    /// Validates `offset` against the current block and appends a Layer 5 ghost text overlay.
    pub fn push_ghost_text(
        &mut self,
        offset: BlockByteOffset,
        affinity: Affinity,
        text: impl Into<String>,
        color: Option<RgbaColor>,
    ) -> Result<(), DecorationError> {
        self.validate_offset(offset)?;
        self.decorations.overlays.push(OverlayDecoration {
            offset,
            affinity,
            kind: OverlayDecorationKind::GhostText {
                text: text.into(),
                color,
            },
        });
        Ok(())
    }

    /// Validates `offset` against the current block and appends a Layer 5 remote caret overlay.
    pub fn push_remote_caret(
        &mut self,
        offset: BlockByteOffset,
        affinity: Affinity,
        color: RgbaColor,
        label: Option<String>,
    ) -> Result<(), DecorationError> {
        self.validate_offset(offset)?;
        self.decorations.overlays.push(OverlayDecoration {
            offset,
            affinity,
            kind: OverlayDecorationKind::RemoteCaret { color, label },
        });
        Ok(())
    }

    /// Consumes the writer and returns all validated decorations collected for the block.
    pub fn finish(self) -> BlockDecorations {
        self.decorations
    }
}

/// Read-only context supplied to a [`DecorationProvider`] for each visible text leaf block.
pub struct DecorationContext<'document> {
    pub document: &'document Document,
    pub block: &'document BlockNode,
    pub selection: Option<&'document DocumentSelection>,
}

/// Extensibility Seam 2: computes transient Layer 2, 4, and 5 visual overlays at prepaint time
/// without mutating [`Document`] or recording Undo [`crate::History`].
pub trait DecorationProvider {
    /// Emits transient decorations for `context.block` into the checked `writer`.
    fn collect_decorations(
        &self,
        context: &DecorationContext<'_>,
        writer: &mut DecorationWriter<'_>,
    );
}
