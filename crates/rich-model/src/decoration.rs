//! Ephemeral decoration contracts feeding paint Layers 2, 4, and 5 without mutating [`Document`].

use crate::block::{BlockNode, Document};
use crate::selection::{Affinity, DocumentSelection};
use crate::style::{RgbaColor, UnderlineKind};
use std::ops::Range;

/// Transient inline overlay scoped to a UTF-8 byte range inside a single text-bearing leaf block.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineDecoration {
    /// UTF-8 byte range within the target leaf block's `InlineContent`.
    pub range: Range<usize>,
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

impl InlineDecoration {
    /// Creates an empty inline decoration covering `range`.
    pub fn new(range: Range<usize>) -> Self {
        Self {
            range,
            background: None,
            foreground: None,
            underline: None,
            strikethrough: false,
            priority: 0,
        }
    }

    /// Sets the Layer 2 background highlight color.
    pub fn with_background(mut self, color: RgbaColor) -> Self {
        self.background = Some(color);
        self
    }

    /// Sets the Layer 4 foreground glyph color override.
    pub fn with_foreground(mut self, color: RgbaColor) -> Self {
        self.foreground = Some(color);
        self
    }

    /// Sets the Layer 4 underline style.
    pub fn with_underline(mut self, underline: UnderlineKind) -> Self {
        self.underline = Some(underline);
        self
    }

    /// Enables or disables a Layer 4 strikethrough overlay.
    pub fn with_strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }

    /// Sets the sorting priority when decorations overlap.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// Transient Layer 5 point overlay anchored inside a single text-bearing leaf block.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayDecoration {
    /// Inline ghost text (such as AI completions) rendered at `offset` without modifying the document.
    GhostText {
        offset: usize,
        text: String,
        color: Option<RgbaColor>,
    },
    /// Remote collaborator caret rendered at `offset` with an optional name badge.
    RemoteCaret {
        offset: usize,
        affinity: Affinity,
        color: RgbaColor,
        label: Option<String>,
    },
}

/// Aggregated transient decorations produced for one visible leaf block.
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

/// Read-only context supplied to a [`DecorationProvider`] for each visible leaf block.
pub struct DecorationContext<'document> {
    pub document: &'document Document,
    pub block: &'document BlockNode,
    pub selection: Option<&'document DocumentSelection>,
}

/// Extensibility Seam 2: computes transient Layer 2, 4, and 5 visual overlays at prepaint time
/// without mutating [`Document`] or recording Undo [`crate::History`].
pub trait DecorationProvider {
    /// Returns transient decorations for `context.block`.
    fn decorations_for_block(&self, context: &DecorationContext<'_>) -> BlockDecorations;
}
