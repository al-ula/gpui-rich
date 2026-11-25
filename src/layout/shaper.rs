use crate::layout::geometry::{Insets, Size, pt_to_px};
use crate::model::{
    BlockId, BlockShapeKind, CustomPayload, FontWeight, InlineObject, InlineVAlign,
    ResolvedTextStyle, RgbaColor, SharedStr, UnderlineKind,
};
use std::fmt::Debug;
use std::hash::Hash;

/// Associates backend-specific font and glyph handles (e.g. `gpui::FontId` + `gpui::GlyphId`).
pub trait GlyphBackend: Copy + Eq + Debug + 'static {
    type Font: Copy + Eq + Hash + Debug + 'static;
    type Glyph: Copy + Eq + Hash + Debug + 'static;
}

/// Headless backend for unit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MockBackend;

impl GlyphBackend for MockBackend {
    type Font = u32;
    type Glyph = u32;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShaperRunSpec {
    /// Length of this run in UTF-8 bytes within the segment's text slice.
    pub len: usize,
    pub font_family: SharedStr,
    pub weight: FontWeight,
    pub italic: bool,
    pub color: RgbaColor,
    pub background: Option<RgbaColor>,
    pub underline: Option<UnderlineKind>,
    pub strikethrough: bool,
    pub inline_code: bool,
    /// UAX #9 Unicode BiDi embedding level (even = LTR, odd = RTL).
    pub bidi_level: u8,
}

impl ShaperRunSpec {
    pub fn from_resolved(len: usize, style: &ResolvedTextStyle) -> Self {
        Self {
            len,
            font_family: style.font_family.clone(),
            weight: style.weight,
            italic: style.italic,
            color: style.color,
            background: style.background,
            underline: style.underline,
            strikethrough: style.strikethrough,
            inline_code: style.inline_code,
            bidi_level: 0,
        }
    }

    /// Sets the UAX #9 BiDi embedding level for this run.
    pub fn with_bidi_level(mut self, bidi_level: u8) -> Self {
        self.bidi_level = bidi_level;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph<G> {
    pub glyph_id: G,
    /// Origin relative to the segment start (x) and segment baseline (y).
    pub x: f32,
    pub y: f32,
    /// UTF-8 byte offset within the shaped segment string.
    pub byte_index: usize,
    pub is_emoji: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedGlyphRun<B: GlyphBackend> {
    pub font: B::Font,
    pub color: RgbaColor,
    pub glyphs: Vec<ShapedGlyph<B::Glyph>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedSegment<B: GlyphBackend> {
    pub font_size_px: f32,
    pub ascent: f32,
    pub descent: f32,
    pub width: f32,
    /// Sorted UTF-8 byte boundaries -> x advance inside this segment, ending with `(text.len(), width)`.
    pub x_for_byte: Vec<(usize, f32)>,
    pub runs: Vec<ShapedGlyphRun<B>>,
}

impl<B: GlyphBackend> ShapedSegment<B> {
    /// Looks up the x coordinate at a UTF-8 byte offset within this segment.
    pub fn x_at_byte(&self, local_byte: usize) -> f32 {
        match self
            .x_for_byte
            .binary_search_by_key(&local_byte, |&(b, _)| b)
        {
            Ok(idx) => self.x_for_byte[idx].1,
            Err(0) => 0.0,
            Err(idx) if idx >= self.x_for_byte.len() => self.width,
            Err(idx) => {
                let (b0, x0) = self.x_for_byte[idx - 1];
                let (b1, x1) = self.x_for_byte[idx];
                if b1 == b0 {
                    x0
                } else {
                    let t = (local_byte - b0) as f32 / (b1 - b0) as f32;
                    x0 + (x1 - x0) * t
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBoxMetrics {
    pub width: f32,
    pub height: f32,
    pub valign: InlineVAlign,
}

impl InlineBoxMetrics {
    /// Returns `true` when width and height are finite and nonnegative.
    pub fn is_valid(&self) -> bool {
        self.width.is_finite() && self.width >= 0.0 && self.height.is_finite() && self.height >= 0.0
    }
}

/// Returns `true` when both `width` and `height` are finite and nonnegative.
pub fn is_valid_custom_size(size: Size) -> bool {
    size.width.is_finite() && size.width >= 0.0 && size.height.is_finite() && size.height >= 0.0
}

/// Returns `true` when all four chrome inset edges are finite and nonnegative.
pub fn is_valid_custom_insets(insets: Insets) -> bool {
    insets.top.is_finite()
        && insets.top >= 0.0
        && insets.right.is_finite()
        && insets.right >= 0.0
        && insets.bottom.is_finite()
        && insets.bottom >= 0.0
        && insets.left.is_finite()
        && insets.left >= 0.0
}

/// Shaping abstraction keeping `layout` pure Rust and testable without a GPU window.
pub trait TextShaper {
    type Backend: GlyphBackend;

    fn shape_segment(
        &mut self,
        text: &str,
        font_size_px: f32,
        runs: &[ShaperRunSpec],
    ) -> ShapedSegment<Self::Backend>;

    fn measure_inline_object(&mut self, obj: &InlineObject, zoom: f32) -> InlineBoxMetrics;

    /// Resolves a display-only atom without changing its backing document identity.
    fn resolve_inline_object(&self, _object: &InlineObject) -> Option<InlineObject> {
        None
    }

    fn set_footnote_numbers(
        &mut self,
        _numbers: &std::sync::Arc<std::collections::HashMap<crate::model::AnchorId, u32>>,
    ) {
    }

    /// Measures a custom leaf block (`BlockKind::Leaf`) inside `max_width` pixels at `zoom`.
    fn measure_custom_block(
        &mut self,
        _block_id: BlockId,
        _kind: &str,
        _data: &CustomPayload,
        max_width: f32,
        zoom: f32,
    ) -> Size {
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    /// Measures chrome insets (in pixels) reserved around a custom text, container, or grid block.
    fn measure_custom_chrome(
        &mut self,
        _shape: BlockShapeKind,
        _kind: &str,
        _data: &CustomPayload,
        _max_width: f32,
        zoom: f32,
    ) -> Insets {
        Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
    }
}

/// Deterministic headless shaper for unit tests and offline layout verification.
#[derive(Clone, Debug, Default)]
pub struct MockShaper;

impl TextShaper for MockShaper {
    type Backend = MockBackend;

    fn shape_segment(
        &mut self,
        text: &str,
        font_size_px: f32,
        runs: &[ShaperRunSpec],
    ) -> ShapedSegment<Self::Backend> {
        let char_w = font_size_px * 0.55;
        let ascent = font_size_px * 0.8;
        let descent = font_size_px * 0.2;

        let mut x = 0.0f32;
        let mut x_for_byte = Vec::with_capacity(text.len() + 1);
        let mut shaped_runs = Vec::with_capacity(runs.len().max(1));

        let mut cursor = 0usize;
        let effective_runs: Vec<ShaperRunSpec> = if runs.is_empty() {
            vec![ShaperRunSpec {
                len: text.len(),
                font_family: std::sync::Arc::from("mock"),
                weight: FontWeight::NORMAL,
                italic: false,
                color: RgbaColor::BLACK,
                background: None,
                underline: None,
                strikethrough: false,
                inline_code: false,
                bidi_level: 0,
            }]
        } else {
            runs.to_vec()
        };

        let is_rtl = effective_runs
            .first()
            .is_some_and(|run| run.bidi_level % 2 == 1);

        if is_rtl {
            let total_width: f32 = text
                .chars()
                .map(|ch| if ch == ' ' { char_w * 0.65 } else { char_w })
                .sum();
            let mut advance = 0.0f32;
            for (run_idx, run) in effective_runs.iter().enumerate() {
                let end = (cursor + run.len).min(text.len());
                let slice = &text[cursor..end];
                let mut glyphs = Vec::new();
                for (rel_idx, ch) in slice.char_indices() {
                    let byte_idx = cursor + rel_idx;
                    let adv = if ch == ' ' { char_w * 0.65 } else { char_w };
                    let right_x = total_width - advance;
                    let left_x = (right_x - adv).max(0.0);
                    x_for_byte.push((byte_idx, right_x));
                    glyphs.push(ShapedGlyph {
                        glyph_id: ch as u32,
                        x: left_x,
                        y: 0.0,
                        byte_index: byte_idx,
                        is_emoji: false,
                    });
                    advance += adv;
                }
                glyphs.reverse();
                shaped_runs.push(ShapedGlyphRun {
                    font: run_idx as u32,
                    color: run.color,
                    glyphs,
                });
                cursor = end;
            }
            x_for_byte.push((text.len(), 0.0));
            return ShapedSegment {
                font_size_px,
                ascent,
                descent,
                width: total_width,
                x_for_byte,
                runs: shaped_runs,
            };
        }

        for (run_idx, run) in effective_runs.iter().enumerate() {
            let end = (cursor + run.len).min(text.len());
            let slice = &text[cursor..end];
            let mut glyphs = Vec::new();
            for (rel_idx, ch) in slice.char_indices() {
                let byte_idx = cursor + rel_idx;
                x_for_byte.push((byte_idx, x));
                let adv = if ch == ' ' { char_w * 0.65 } else { char_w };
                glyphs.push(ShapedGlyph {
                    glyph_id: ch as u32,
                    x,
                    y: 0.0,
                    byte_index: byte_idx,
                    is_emoji: false,
                });
                x += adv;
            }
            shaped_runs.push(ShapedGlyphRun {
                font: run_idx as u32,
                color: run.color,
                glyphs,
            });
            cursor = end;
        }
        x_for_byte.push((text.len(), x));

        ShapedSegment {
            font_size_px,
            ascent,
            descent,
            width: x,
            x_for_byte,
            runs: shaped_runs,
        }
    }

    fn measure_inline_object(&mut self, obj: &InlineObject, zoom: f32) -> InlineBoxMetrics {
        match obj {
            InlineObject::Image {
                width_pt,
                height_pt,
                valign,
                ..
            } => InlineBoxMetrics {
                width: pt_to_px(*width_pt, zoom),
                height: pt_to_px(*height_pt, zoom),
                valign: *valign,
            },
            InlineObject::SmartChip { label, .. } => {
                let font_px = pt_to_px(10.0, zoom);
                InlineBoxMetrics {
                    width: (label.chars().count() as f32) * font_px * 0.58 + pt_to_px(18.0, zoom),
                    height: pt_to_px(18.0, zoom),
                    valign: InlineVAlign::Middle,
                }
            }
            InlineObject::FootnoteRef { index } => {
                let digits = index.to_string().len().max(1) as f32;
                InlineBoxMetrics {
                    width: pt_to_px(8.0 + digits * 5.0, zoom),
                    height: pt_to_px(12.0, zoom),
                    valign: InlineVAlign::Baseline,
                }
            }
            InlineObject::Custom { kind, .. } => {
                let font_px = pt_to_px(10.0, zoom);
                InlineBoxMetrics {
                    width: (kind.chars().count().max(4) as f32) * font_px * 0.58
                        + pt_to_px(16.0, zoom),
                    height: pt_to_px(18.0, zoom),
                    valign: InlineVAlign::Middle,
                }
            }
        }
    }
}
