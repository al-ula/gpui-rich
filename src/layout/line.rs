use crate::layout::geometry::{Point, Rect, pt_to_px};
use crate::layout::shaper::{
    GlyphBackend, InlineBoxMetrics, ShapedGlyph, ShapedGlyphRun, ShapedSegment, ShaperRunSpec,
    TextShaper,
};
use crate::model::{
    Affinity, BlockId, BlockStyle, DocumentLayoutConfig, InlineContent, InlineObject,
    InlineObjectId, InlineStyle, InlineVAlign, LineHeight, LinkTarget, OBJECT_REPLACEMENT_CHAR,
    ResolvedTextStyle, RgbaColor, StyleSheet, TextAlign, TextDirection, UnderlineKind,
};
use std::ops::Range;
use unicode_bidi::{BidiInfo, Level};
use unicode_linebreak::linebreaks;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq)]
pub struct DecorationSpan {
    pub doc_range: Range<usize>,
    pub background: Option<RgbaColor>,
    pub underline: Option<UnderlineKind>,
    pub strikethrough: bool,
    pub inline_code: bool,
    pub link: Option<LinkTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextFragment<B: GlyphBackend> {
    pub doc_range: Range<usize>,
    pub x: f32,
    pub width: f32,
    pub font_size_px: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Positive shifts upward (superscript), negative shifts downward (subscript).
    pub baseline_shift_px: f32,
    pub bidi_level: u8,
    /// Sorted `(doc_byte_offset, local_x)` pairs covering `doc_range.start..=doc_range.end`.
    pub x_for_byte: Vec<(usize, f32)>,
    pub runs: Vec<ShapedGlyphRun<B>>,
    pub decorations: Vec<DecorationSpan>,
}

impl<B: GlyphBackend> TextFragment<B> {
    /// Returns true when this fragment has an odd UAX #9 BiDi embedding level (RTL).
    pub const fn is_rtl(&self) -> bool {
        self.bidi_level % 2 == 1
    }

    pub fn local_x_for_byte(&self, doc_byte: usize) -> f32 {
        let clamped = doc_byte.clamp(self.doc_range.start, self.doc_range.end);
        match self.x_for_byte.binary_search_by_key(&clamped, |&(b, _)| b) {
            Ok(idx) => self.x_for_byte[idx].1,
            Err(0) => {
                if self.is_rtl() {
                    self.width
                } else {
                    0.0
                }
            }
            Err(idx) if idx >= self.x_for_byte.len() => {
                if self.is_rtl() {
                    0.0
                } else {
                    self.width
                }
            }
            Err(idx) => {
                let (b0, x0) = self.x_for_byte[idx - 1];
                let (b1, x1) = self.x_for_byte[idx];
                if b1 == b0 {
                    x0
                } else {
                    let t = (clamped - b0) as f32 / (b1 - b0) as f32;
                    x0 + (x1 - x0) * t
                }
            }
        }
    }

    pub fn byte_for_local_x(&self, local_x: f32) -> usize {
        if self.x_for_byte.len() < 2 {
            return self.doc_range.start;
        }
        if local_x <= 0.0 {
            return if self.is_rtl() {
                self.doc_range.end
            } else {
                self.doc_range.start
            };
        }
        if local_x >= self.width {
            return if self.is_rtl() {
                self.doc_range.start
            } else {
                self.doc_range.end
            };
        }
        for pair in self.x_for_byte.windows(2) {
            let (b0, x0) = pair[0];
            let (b1, x1) = pair[1];
            let low = x0.min(x1);
            let high = x0.max(x1);
            if local_x >= low && local_x <= high {
                let mid = (low + high) * 0.5;
                return if x0 <= x1 {
                    if local_x < mid { b0 } else { b1 }
                } else if local_x < mid {
                    b1
                } else {
                    b0
                };
            }
        }
        if self.is_rtl() {
            self.doc_range.start
        } else {
            self.doc_range.end
        }
    }

    pub fn byte_containing_local_x(&self, local_x: f32) -> Option<usize> {
        if self.x_for_byte.len() < 2 || local_x < 0.0 || local_x > self.width {
            return None;
        }
        for pair in self.x_for_byte.windows(2) {
            let (b0, x0) = pair[0];
            let (_, x1) = pair[1];
            let low = x0.min(x1);
            let high = x0.max(x1);
            if local_x >= low && local_x <= high {
                return Some(b0);
            }
        }
        None
    }

    pub fn link_at(&self, doc_byte: usize) -> Option<&LinkTarget> {
        self.decorations
            .iter()
            .find(|d| d.doc_range.contains(&doc_byte))
            .and_then(|d| d.link.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InlineBoxFragment {
    pub doc_range: Range<usize>,
    pub object_id: InlineObjectId,
    pub object: InlineObject,
    pub x: f32,
    pub width: f32,
    pub height: f32,
    /// Distance in pixels from the mathematical baseline to the bottom of the box (used with
    /// [`InlineVAlign::Baseline`]).
    pub depth: f32,
    pub valign: InlineVAlign,
    pub font_size_pt: f32,
    pub text_color: RgbaColor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabFragment {
    pub doc_range: Range<usize>,
    pub x: f32,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LineFragment<B: GlyphBackend> {
    Text(TextFragment<B>),
    InlineBox(InlineBoxFragment),
    Tab(TabFragment),
}

impl<B: GlyphBackend> LineFragment<B> {
    pub fn doc_range(&self) -> Range<usize> {
        match self {
            Self::Text(t) => t.doc_range.clone(),
            Self::InlineBox(b) => b.doc_range.clone(),
            Self::Tab(t) => t.doc_range.clone(),
        }
    }

    pub fn x(&self) -> f32 {
        match self {
            Self::Text(t) => t.x,
            Self::InlineBox(b) => b.x,
            Self::Tab(t) => t.x,
        }
    }

    pub fn width(&self) -> f32 {
        match self {
            Self::Text(t) => t.width,
            Self::InlineBox(b) => b.width,
            Self::Tab(t) => t.width,
        }
    }

    pub fn set_x(&mut self, new_x: f32) {
        match self {
            Self::Text(t) => t.x = new_x,
            Self::InlineBox(b) => b.x = new_x,
            Self::Tab(t) => t.x = new_x,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VisualLine<B: GlyphBackend> {
    pub doc_range: Range<usize>,
    pub rect: Rect,
    pub baseline_y: f32,
    pub ascent: f32,
    pub descent: f32,
    pub soft_wrapped: bool,
    pub base_direction_rtl: bool,
    pub fragments: Vec<LineFragment<B>>,
}

impl<B: GlyphBackend> VisualLine<B> {
    /// Computes the vertical rect of an inline box relative to the block origin.
    pub fn inline_box_rect(&self, bx: &InlineBoxFragment) -> Rect {
        let top = match bx.valign {
            InlineVAlign::Baseline => {
                let clamped_depth = bx.depth.clamp(0.0, bx.height);
                self.baseline_y - (bx.height - clamped_depth)
            }
            InlineVAlign::Middle => {
                let mid = self.baseline_y - self.ascent * 0.35;
                mid - bx.height * 0.5
            }
            InlineVAlign::Bottom => self.baseline_y + self.descent - bx.height,
        };
        Rect::new(bx.x, top, bx.width, bx.height)
    }

    pub fn x_for_offset(&self, offset: usize) -> f32 {
        if self.fragments.is_empty() {
            return if self.base_direction_rtl {
                self.rect.right()
            } else {
                self.rect.x
            };
        }

        // Fragments are in visual order (which can differ from logical order in BiDi lines),
        // so prefer an interior match before boundary matches.
        let mut boundary_match: Option<f32> = None;
        for frag in &self.fragments {
            let range = frag.doc_range();
            if offset >= range.start && offset <= range.end {
                let x = match frag {
                    LineFragment::Text(t) => t.x + t.local_x_for_byte(offset),
                    LineFragment::InlineBox(b) => {
                        if offset <= b.doc_range.start {
                            b.x
                        } else {
                            b.x + b.width
                        }
                    }
                    LineFragment::Tab(t) => {
                        if offset <= t.doc_range.start {
                            t.x
                        } else {
                            t.x + t.width
                        }
                    }
                };
                if offset > range.start && offset < range.end {
                    return x;
                }
                if boundary_match.is_none() {
                    boundary_match = Some(x);
                }
            }
        }
        if let Some(x) = boundary_match {
            return x;
        }

        // Fall back to logical start/end edges.
        if offset <= self.doc_range.start {
            if self.base_direction_rtl {
                self.fragments
                    .last()
                    .map(|last| last.x() + last.width())
                    .unwrap_or(self.rect.right())
            } else {
                self.fragments
                    .first()
                    .map(|first| first.x())
                    .unwrap_or(self.rect.x)
            }
        } else if self.base_direction_rtl {
            self.fragments
                .first()
                .map(|first| first.x())
                .unwrap_or(self.rect.x)
        } else {
            self.fragments
                .last()
                .map(|last| last.x() + last.width())
                .unwrap_or(self.rect.x)
        }
    }

    /// Returns `(top_y, height)` for a caret sized to the local fragment at `offset`.
    pub fn caret_vertical_metrics(&self, offset: usize) -> (f32, f32) {
        for frag in &self.fragments {
            let range = frag.doc_range();
            if offset >= range.start && offset <= range.end {
                return match frag {
                    LineFragment::Text(t) => {
                        let base = self.baseline_y - t.baseline_shift_px;
                        let top = base - t.ascent;
                        (top, (t.ascent + t.descent).max(1.0))
                    }
                    LineFragment::InlineBox(b) => {
                        let r = self.inline_box_rect(b);
                        (r.y, r.height.max(1.0))
                    }
                    LineFragment::Tab(_) => (
                        self.baseline_y - self.ascent,
                        (self.ascent + self.descent).max(1.0),
                    ),
                };
            }
        }
        (
            self.baseline_y - self.ascent,
            (self.ascent + self.descent).max(1.0),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HitResult {
    pub offset: usize,
    pub affinity: Affinity,
    pub inline_object: Option<InlineObjectId>,
    pub link: Option<LinkTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphLayout<B: GlyphBackend> {
    pub width: f32,
    pub height: f32,
    pub lines: Vec<VisualLine<B>>,
    hidden_ranges: Vec<Range<usize>>,
}

impl<B: GlyphBackend> ParagraphLayout<B> {
    pub fn hit_test(&self, local_point: Point) -> HitResult {
        let mut hit = self.hit_test_geometry(local_point);
        if let Some(range) = self
            .hidden_ranges
            .iter()
            .find(|range| range.start <= hit.offset && hit.offset <= range.end)
        {
            let line = &self.lines[self.line_index_for_pos(hit.offset, hit.affinity)];
            if !line.fragments.is_empty()
                && range.start > line.doc_range.start
                && self
                    .lines
                    .last()
                    .is_some_and(|last| range.end == last.doc_range.end)
            {
                hit.offset = range.start;
            } else {
                hit.offset = range.end;
            }
        }
        hit
    }

    fn hit_test_geometry(&self, local_point: Point) -> HitResult {
        if self.lines.is_empty() {
            return HitResult {
                offset: 0,
                affinity: Affinity::Downstream,
                inline_object: None,
                link: None,
            };
        }

        // Binary-search visual lines by vertical span.
        let line_idx = match self.lines.binary_search_by(|line| {
            if local_point.y < line.rect.y {
                std::cmp::Ordering::Greater
            } else if local_point.y > line.rect.bottom() {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(idx) => idx,
            Err(0) => 0,
            Err(idx) => (idx - 1).min(self.lines.len() - 1),
        };

        let line = &self.lines[line_idx];
        if line.fragments.is_empty() {
            return HitResult {
                offset: line.doc_range.start,
                affinity: Affinity::Downstream,
                inline_object: None,
                link: None,
            };
        }

        let Some(first) = line.fragments.first() else {
            return HitResult {
                offset: line.doc_range.start,
                affinity: Affinity::Downstream,
                inline_object: None,
                link: None,
            };
        };
        if local_point.x <= first.x() {
            let offset = if line.base_direction_rtl {
                line.doc_range.end
            } else {
                line.doc_range.start
            };
            return HitResult {
                offset,
                affinity: if line.soft_wrapped && offset == line.doc_range.end {
                    Affinity::Upstream
                } else {
                    Affinity::Downstream
                },
                inline_object: None,
                link: None,
            };
        }

        for frag in &line.fragments {
            let right = frag.x() + frag.width();
            if local_point.x <= right {
                return match frag {
                    LineFragment::Text(t) => {
                        let local_x = local_point.x - t.x;
                        let offset = t.byte_for_local_x(local_x);
                        let affinity = if line.soft_wrapped && offset == line.doc_range.end {
                            Affinity::Upstream
                        } else {
                            Affinity::Downstream
                        };
                        let link = t
                            .byte_containing_local_x(local_x)
                            .and_then(|byte| t.link_at(byte))
                            .cloned();
                        HitResult {
                            offset,
                            affinity,
                            inline_object: None,
                            link,
                        }
                    }
                    LineFragment::InlineBox(b) => {
                        let mid = b.x + b.width * 0.5;
                        let offset = if local_point.x < mid {
                            b.doc_range.start
                        } else {
                            b.doc_range.end
                        };
                        HitResult {
                            offset,
                            affinity: Affinity::Downstream,
                            inline_object: Some(b.object_id),
                            link: None,
                        }
                    }
                    LineFragment::Tab(t) => {
                        let mid = t.x + t.width * 0.5;
                        let offset = if local_point.x < mid {
                            t.doc_range.start
                        } else {
                            t.doc_range.end
                        };
                        HitResult {
                            offset,
                            affinity: Affinity::Downstream,
                            inline_object: None,
                            link: None,
                        }
                    }
                };
            }
        }

        let end_offset = if line.base_direction_rtl {
            line.doc_range.start
        } else {
            line.doc_range.end
        };
        HitResult {
            offset: end_offset,
            affinity: if line.soft_wrapped && end_offset == line.doc_range.end {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            },
            inline_object: None,
            link: None,
        }
    }

    pub fn line_index_for_pos(&self, offset: usize, affinity: Affinity) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        for (idx, line) in self.lines.iter().enumerate() {
            let is_last = idx + 1 == self.lines.len();
            if offset < line.doc_range.start {
                return idx;
            }
            if offset < line.doc_range.end {
                return idx;
            }
            if offset == line.doc_range.end
                && (is_last || !line.soft_wrapped || affinity == Affinity::Upstream)
            {
                return idx;
            }
        }
        self.lines.len() - 1
    }

    pub fn caret_rect(&self, offset: usize, affinity: Affinity) -> Rect {
        if self.lines.is_empty() {
            return Rect::new(0.0, 0.0, 2.0, 16.0);
        }
        let line_idx = self.line_index_for_pos(offset, affinity);
        let line = &self.lines[line_idx];
        let x = line.x_for_offset(offset);
        let (top, height) = line.caret_vertical_metrics(offset);
        Rect::new(x, top, 2.0, height)
    }

    pub fn selection_rects(
        &self,
        range: Range<usize>,
        include_trailing_newline: bool,
    ) -> Vec<Rect> {
        let mut rects = Vec::new();
        if range.is_empty() && !include_trailing_newline {
            return rects;
        }
        for (idx, line) in self.lines.iter().enumerate() {
            let is_last = idx + 1 == self.lines.len();
            if range.end < line.doc_range.start {
                break;
            }
            if range.start > line.doc_range.end {
                continue;
            }
            if range.start == line.doc_range.end && (!is_last || !include_trailing_newline) {
                continue;
            }
            let s = range.start.max(line.doc_range.start);
            let e = range.end.min(line.doc_range.end);

            if line.fragments.is_empty() {
                if is_last && include_trailing_newline {
                    let x0 = line.x_for_offset(s);
                    rects.push(Rect::new(x0, line.rect.y, 6.0, line.rect.height));
                }
                continue;
            }

            // Compute per-fragment visual spans so mixed LTR/RTL selections produce exact
            // disjoint rectangles when a logical range crosses BiDi boundaries.
            let mut spans: Vec<(f32, f32)> = Vec::new();
            for frag in &line.fragments {
                let frag_range = frag.doc_range();
                let fs = s.max(frag_range.start);
                let fe = e.min(frag_range.end);
                if fs >= fe {
                    continue;
                }
                let (x_a, x_b) = match frag {
                    LineFragment::Text(t) => {
                        (t.x + t.local_x_for_byte(fs), t.x + t.local_x_for_byte(fe))
                    }
                    LineFragment::InlineBox(b) => (b.x, b.x + b.width),
                    LineFragment::Tab(t) => (t.x, t.x + t.width),
                };
                let left = x_a.min(x_b);
                let right = x_a.max(x_b);
                if right > left {
                    spans.push((left, right));
                }
            }

            if is_last && include_trailing_newline {
                let end_x = line
                    .fragments
                    .last()
                    .map(|last| last.x() + last.width())
                    .unwrap_or(line.rect.x);
                spans.push((end_x, end_x + 6.0));
            }

            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f32, f32)> = Vec::with_capacity(spans.len());
            for (left, right) in spans {
                if let Some(last) = merged.last_mut()
                    && left <= last.1 + 0.5
                {
                    last.1 = last.1.max(right);
                } else {
                    merged.push((left, right));
                }
            }
            for (left, right) in merged {
                if right > left {
                    rects.push(Rect::new(left, line.rect.y, right - left, line.rect.height));
                }
            }
        }
        rects
    }

    pub fn vertical_step(
        &self,
        offset: usize,
        affinity: Affinity,
        goal_x: f32,
        delta_lines: isize,
    ) -> Option<(usize, Affinity)> {
        if self.lines.is_empty() {
            return None;
        }
        let cur = self.line_index_for_pos(offset, affinity) as isize;
        let target = cur + delta_lines;
        if target < 0 || target >= self.lines.len() as isize {
            return None;
        }
        let line = &self.lines[target as usize];
        let hit = self.hit_test(Point::new(goal_x, line.rect.center().y));
        Some((hit.offset, hit.affinity))
    }
}

// ============================================================================
// Internal Itemization, Line Breaking & Baseline Composition
// ============================================================================

enum ShapedItem<B: GlyphBackend> {
    Text {
        doc_range: Range<usize>,
        font_size_px: f32,
        baseline_shift_px: f32,
        bidi_level: u8,
        segment: ShapedSegment<B>,
        decorations: Vec<DecorationSpan>,
    },
    InlineBox {
        doc_range: Range<usize>,
        object_id: InlineObjectId,
        object: InlineObject,
        metrics: InlineBoxMetrics,
        font_size_pt: f32,
        text_color: RgbaColor,
    },
    Tab {
        doc_range: Range<usize>,
    },
    HardBreak {
        doc_range: Range<usize>,
    },
}

impl<B: GlyphBackend> ShapedItem<B> {
    fn doc_range(&self) -> Range<usize> {
        match self {
            Self::Text { doc_range, .. }
            | Self::InlineBox { doc_range, .. }
            | Self::Tab { doc_range }
            | Self::HardBreak { doc_range } => doc_range.clone(),
        }
    }
}

struct PendingRun {
    doc_range: Range<usize>,
    resolved: ResolvedTextStyle,
    bidi_level: u8,
}

fn quantize_tenth(v: f32) -> i32 {
    (v * 10.0).round() as i32
}

struct ParagraphBidi {
    /// Per-UTF-8-byte UAX #9 embedding level (empty when fast-pathed to all-LTR).
    byte_levels: Vec<u8>,
    /// True when the paragraph base embedding level is odd (RTL).
    base_rtl: bool,
    /// True when any byte in the paragraph has an odd BiDi level or base is RTL.
    has_rtl: bool,
}

impl ParagraphBidi {
    fn analyze(text: &str, direction: TextDirection, is_ascii: bool) -> Self {
        if is_ascii && direction != TextDirection::Rtl {
            return Self {
                byte_levels: Vec::new(),
                base_rtl: false,
                has_rtl: false,
            };
        }
        let default_level = match direction {
            TextDirection::Auto => None,
            TextDirection::Ltr => Some(Level::ltr()),
            TextDirection::Rtl => Some(Level::rtl()),
        };
        let info = BidiInfo::new(text, default_level);
        let base_rtl = info
            .paragraphs
            .first()
            .map(|para| para.level.is_rtl())
            .unwrap_or(direction == TextDirection::Rtl);
        let mut has_rtl = base_rtl;
        let byte_levels: Vec<u8> = info
            .levels
            .iter()
            .map(|level| {
                let num = level.number();
                if num % 2 == 1 {
                    has_rtl = true;
                }
                num
            })
            .collect();
        Self {
            byte_levels,
            base_rtl,
            has_rtl,
        }
    }

    #[inline]
    fn level_at(&self, byte_index: usize) -> u8 {
        self.byte_levels.get(byte_index).copied().unwrap_or(0)
    }

    #[inline]
    fn base_level(&self) -> u8 {
        if self.base_rtl { 1 } else { 0 }
    }
}

pub fn layout_paragraph<S: TextShaper>(
    content: &InlineContent,
    block_style: &BlockStyle,
    sheet: &StyleSheet,
    config: &DocumentLayoutConfig,
    max_width: f32,
    zoom: f32,
    shaper: &mut S,
) -> ParagraphLayout<S::Backend> {
    layout_paragraph_for_block(
        None,
        content,
        block_style,
        sheet,
        config,
        max_width,
        zoom,
        shaper,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_for_block<S: TextShaper>(
    block_id: Option<BlockId>,
    content: &InlineContent,
    block_style: &BlockStyle,
    sheet: &StyleSheet,
    config: &DocumentLayoutConfig,
    max_width: f32,
    zoom: f32,
    shaper: &mut S,
) -> ParagraphLayout<S::Backend> {
    let base_text = block_style.resolved_base_text(sheet, &config.default_style);
    let base_font_px = pt_to_px(base_text.font_size_pt, zoom).max(1.0);
    let line_height_spec = block_style.resolved_line_height(sheet);
    let indent_start = pt_to_px(block_style.indent_start_pt, zoom);
    let indent_end = pt_to_px(block_style.indent_end_pt, zoom);
    let first_line_indent = pt_to_px(block_style.first_line_indent_pt, zoom);
    let default_tab_px = pt_to_px(config.default_tab_interval_pt, zoom).max(8.0);

    // Measure default metrics for empty lines.
    let default_probe = shaper.shape_segment(
        " ",
        base_font_px,
        &[ShaperRunSpec::from_resolved(1, &base_text)],
    );
    let default_ascent = default_probe.ascent.max(base_font_px * 0.75);
    let default_descent = default_probe.descent.max(base_font_px * 0.25);

    if content.is_empty() {
        let natural = default_ascent + default_descent;
        let lh = resolve_line_height(line_height_spec, natural, zoom);
        let half_leading = ((lh - natural) * 0.5).max(0.0);
        let line_x = (indent_start + first_line_indent).max(0.0);
        let avail = (max_width - line_x - indent_end).max(1.0);
        return ParagraphLayout {
            width: max_width,
            height: lh,
            lines: vec![VisualLine {
                doc_range: 0..0,
                rect: Rect::new(line_x, 0.0, avail, lh),
                baseline_y: half_leading + default_ascent,
                ascent: default_ascent,
                descent: default_descent,
                soft_wrapped: false,
                base_direction_rtl: block_style.direction == TextDirection::Rtl,
                fragments: Vec::new(),
            }],
            hidden_ranges: Vec::new(),
        };
    }

    // 0. Analyze UAX #9 BiDi levels (fast-pathed to zero allocations for ASCII LTR).
    let bidi = ParagraphBidi::analyze(&content.text, block_style.direction, content.is_ascii);

    // 1. Itemize into ShapedItems (batched by uniform font_size + baseline_shift + bidi_level).
    let items = itemize_and_shape(block_id, content, &base_text, &bidi, zoom, shaper);

    // 2. Collect UAX #14 break points plus forced breaks around tabs, hard breaks, and U+FFFC.
    let mut break_points: Vec<usize> = linebreaks(&content.text)
        .map(|(offset, _)| offset)
        .filter(|&o| o > 0 && o <= content.text.len())
        .collect();
    for item in &items {
        let r = item.doc_range();
        match item {
            ShapedItem::Tab { .. }
            | ShapedItem::HardBreak { .. }
            | ShapedItem::InlineBox { .. } => {
                if r.start > 0 {
                    break_points.push(r.start);
                }
                break_points.push(r.end);
            }
            ShapedItem::Text { .. } => {}
        }
    }
    break_points.push(content.text.len());
    break_points.sort_unstable();
    break_points.dedup();

    // 3. Greedy line breaking with glyph slicing and emergency grapheme fallback.
    type RawVisualLine<B> = (Range<usize>, bool, Vec<LineFragment<B>>);
    let mut raw_lines: Vec<RawVisualLine<S::Backend>> = Vec::new();
    let mut cur_frags: Vec<LineFragment<S::Backend>> = Vec::new();
    let mut cur_x = 0.0f32;
    let mut cur_line_start = 0usize;
    let mut line_index = 0usize;

    let line_available_width = |idx: usize| -> f32 {
        let start = if idx == 0 {
            (indent_start + first_line_indent).max(0.0)
        } else {
            indent_start.max(0.0)
        };
        (max_width - start - indent_end).max(24.0)
    };

    let mut chunk_start = 0usize;
    for &chunk_end in &break_points {
        if chunk_end <= chunk_start {
            continue;
        }
        let slice_text = &content.text[chunk_start..chunk_end];

        // Explicit newline (`\n`) ends the current visual line.
        if slice_text == "\n" {
            raw_lines.push((
                cur_line_start..chunk_start,
                false,
                std::mem::take(&mut cur_frags),
            ));
            line_index += 1;
            cur_x = 0.0;
            cur_line_start = chunk_end;
            chunk_start = chunk_end;
            continue;
        }

        // Tab stop fragment.
        if slice_text == "\t" {
            let line_start_x = if line_index == 0 {
                (indent_start + first_line_indent).max(0.0)
            } else {
                indent_start.max(0.0)
            };
            let abs_x = line_start_x + cur_x;
            let next_tab_x = next_tab_stop(abs_x, &block_style.tab_stops, default_tab_px, zoom);
            let tab_w = (next_tab_x - abs_x).max(4.0);
            cur_frags.push(LineFragment::Tab(TabFragment {
                doc_range: chunk_start..chunk_end,
                x: cur_x,
                width: tab_w,
            }));
            cur_x += tab_w;
            chunk_start = chunk_end;
            continue;
        }

        let chunk_frags = slice_items_range(&items, chunk_start..chunk_end);
        let chunk_total_w: f32 = chunk_frags.iter().map(|f| f.width()).sum();
        let trimmed_end = chunk_start + slice_text.trim_end_matches(' ').len();
        let chunk_visible_w = if trimmed_end == chunk_end {
            chunk_total_w
        } else {
            slice_items_range(&items, chunk_start..trimmed_end)
                .iter()
                .map(|f| f.width())
                .sum()
        };

        let avail = line_available_width(line_index);
        if !cur_frags.is_empty() && cur_x + chunk_visible_w > avail + 0.01 {
            raw_lines.push((
                cur_line_start..chunk_start,
                true,
                std::mem::take(&mut cur_frags),
            ));
            line_index += 1;
            cur_x = 0.0;
            cur_line_start = chunk_start;
        }

        let avail_now = line_available_width(line_index);
        if cur_frags.is_empty() && chunk_visible_w > avail_now + 0.01 {
            // Emergency grapheme-cluster break for unbreakable oversized tokens (e.g., long URLs).
            let mut g_start = chunk_start;
            let graphemes: Vec<(usize, &str)> = content.text[chunk_start..chunk_end]
                .grapheme_indices(true)
                .collect();
            let mut idx = 0usize;
            while idx < graphemes.len() {
                let g_rel = graphemes[idx].0;
                let g_len = graphemes[idx].1.len();
                let g_end = chunk_start + g_rel + g_len;
                let cand = slice_items_range(&items, g_start..g_end);
                let cand_w: f32 = cand.iter().map(|f| f.width()).sum();
                if cand_w > avail_now + 0.01 && g_end > g_start + g_len {
                    let prev_end = chunk_start + g_rel;
                    let frags = slice_items_range(&items, g_start..prev_end);
                    append_positioned_fragments(&mut cur_frags, &mut cur_x, frags);
                    raw_lines.push((
                        cur_line_start..prev_end,
                        true,
                        std::mem::take(&mut cur_frags),
                    ));
                    line_index += 1;
                    cur_x = 0.0;
                    cur_line_start = prev_end;
                    g_start = prev_end;
                    continue;
                }
                idx += 1;
            }
            if g_start < chunk_end {
                let tail = slice_items_range(&items, g_start..chunk_end);
                append_positioned_fragments(&mut cur_frags, &mut cur_x, tail);
            }
        } else {
            append_positioned_fragments(&mut cur_frags, &mut cur_x, chunk_frags);
        }

        chunk_start = chunk_end;
    }

    // Flush the final line (also emits an empty trailing line if text ended with '\n').
    if !cur_frags.is_empty() || content.text.ends_with('\n') || raw_lines.is_empty() {
        raw_lines.push((cur_line_start..content.text.len(), false, cur_frags));
    }

    // 4. Compute shared baselines, line heights, alignment, and justification.
    let mut y = 0.0f32;
    let mut visual_lines = Vec::with_capacity(raw_lines.len());

    for (idx, (doc_range, soft_wrapped, mut fragments)) in raw_lines.into_iter().enumerate() {
        let start_x = if idx == 0 {
            (indent_start + first_line_indent).max(0.0)
        } else {
            indent_start.max(0.0)
        };
        let avail = (max_width - start_x - indent_end).max(24.0);

        let (mut max_ascent, mut max_descent) = if fragments.is_empty() {
            (default_ascent, default_descent)
        } else {
            (0.0f32, 0.0f32)
        };

        for frag in &fragments {
            match frag {
                LineFragment::Text(t) => {
                    max_ascent = max_ascent.max(t.ascent + t.baseline_shift_px);
                    max_descent = max_descent.max(t.descent - t.baseline_shift_px);
                }
                LineFragment::InlineBox(b) => match b.valign {
                    InlineVAlign::Baseline => {
                        let clamped_depth = b.depth.clamp(0.0, b.height);
                        let box_ascent = (b.height - clamped_depth).max(0.0);
                        max_ascent = max_ascent.max(box_ascent);
                        max_descent = max_descent.max(clamped_depth);
                    }
                    InlineVAlign::Middle => {
                        let half = b.height * 0.5;
                        max_ascent = max_ascent.max(half + default_ascent * 0.35);
                        max_descent = max_descent.max((half - default_ascent * 0.35).max(0.0));
                    }
                    InlineVAlign::Bottom => {
                        max_ascent = max_ascent.max((b.height - default_descent).max(0.0));
                        max_descent = max_descent.max(default_descent);
                    }
                },
                LineFragment::Tab(_) => {
                    max_ascent = max_ascent.max(default_ascent);
                    max_descent = max_descent.max(default_descent);
                }
            }
        }
        max_ascent = max_ascent.max(default_ascent * 0.5);
        max_descent = max_descent.max(default_descent * 0.5);

        let natural = max_ascent + max_descent;
        let lh = resolve_line_height(line_height_spec, natural, zoom);
        let half_leading = ((lh - natural) * 0.5).max(0.0);
        let baseline_y = y + half_leading + max_ascent;

        // Visible width excludes trailing ASCII spaces on soft-wrapped lines (computed in logical order).
        let visible_width = compute_visible_line_width(&fragments, &content.text, soft_wrapped);

        // Apply UAX #9 L1 + L2 visual reordering when the paragraph contains RTL runs or RTL base direction.
        if bidi.has_rtl && fragments.len() > 1 {
            reorder_visual_line_fragments(&mut fragments, &content.text, &bidi);
        }

        let effective_align = if bidi.base_rtl && block_style.align == TextAlign::Left {
            TextAlign::Right
        } else {
            block_style.align
        };

        let x_offset = match effective_align {
            TextAlign::Left => start_x,
            TextAlign::Center => start_x + ((avail - visible_width) * 0.5).max(0.0),
            TextAlign::Right => start_x + (avail - visible_width).max(0.0),
            TextAlign::Justify => {
                if soft_wrapped && visible_width < avail {
                    apply_justification(&mut fragments, &content.text, avail - visible_width);
                }
                start_x
            }
        };

        for frag in &mut fragments {
            let cur = frag.x();
            frag.set_x(cur + x_offset);
        }

        visual_lines.push(VisualLine {
            doc_range,
            rect: Rect::new(start_x, y, avail, lh),
            baseline_y,
            ascent: max_ascent,
            descent: max_descent,
            soft_wrapped,
            base_direction_rtl: bidi.base_rtl,
            fragments,
        });
        y += lh;
    }

    ParagraphLayout {
        width: max_width,
        height: y,
        lines: visual_lines,
        hidden_ranges: Vec::new(),
    }
}

// Shape and wrap only visible text, then map geometry and glyph indices back to source.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_source_paragraph<S: TextShaper>(
    block_id: BlockId,
    content: &InlineContent,
    ranges: &[Range<usize>],
    style: &BlockStyle,
    sheet: &StyleSheet,
    config: &DocumentLayoutConfig,
    max_width: f32,
    zoom: f32,
    shaper: &mut S,
) -> ParagraphLayout<S::Backend> {
    let text = &content.text;
    let mut seams: Vec<(usize, Range<usize>)> = Vec::new();
    let mut visible = InlineContent::empty();
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor || range.is_empty() || text.get(range.clone()).is_none() {
            continue;
        }
        visible.push_text(&text[cursor..range.start], InlineStyle::default());
        if let Some((_, previous)) = seams.last_mut()
            && previous.end == range.start
        {
            previous.end = range.end;
        } else {
            seams.push((visible.len(), range.clone()));
        }
        cursor = range.end;
    }
    visible.push_text(&text[cursor..], InlineStyle::default());
    let mut layout = layout_paragraph_for_block(
        Some(block_id),
        &visible,
        style,
        sheet,
        config,
        max_width,
        zoom,
        shaper,
    );
    let remap = |offset: usize, after: bool| {
        let index = seams.partition_point(|(seam, _)| *seam < offset);
        if let Some((seam, range)) = seams.get(index)
            && *seam == offset
        {
            return if after { range.end } else { range.start };
        }
        index.checked_sub(1).map_or(offset, |previous| {
            let (seam, range) = &seams[previous];
            range.end + (offset - seam)
        })
    };
    for line in &mut layout.lines {
        let start = remap(line.doc_range.start, line.doc_range.start != 0);
        line.doc_range = start..remap(line.doc_range.end, true);
        for fragment in &mut line.fragments {
            match fragment {
                LineFragment::Text(text) => {
                    let mut points: Vec<(usize, f32)> = Vec::with_capacity(text.x_for_byte.len());
                    let mut seam_index =
                        seams.partition_point(|(offset, _)| *offset < text.doc_range.start);
                    for (offset, x) in text.x_for_byte.drain(..) {
                        while let Some((seam, range)) = seams.get(seam_index)
                            && *seam <= offset
                        {
                            let seam_x = if *seam == offset {
                                x
                            } else if let Some(&(previous, previous_x)) = points.last() {
                                let ratio = (range.start - previous) as f32
                                    / (remap(offset, false) - previous) as f32;
                                previous_x + (x - previous_x) * ratio
                            } else {
                                x
                            };
                            points.push((range.start, seam_x));
                            if *seam < offset {
                                points.push((range.end, seam_x));
                            }
                            seam_index += 1;
                        }
                        points.push((remap(offset, true), x));
                    }
                    text.x_for_byte = points;
                    text.doc_range =
                        remap(text.doc_range.start, false)..remap(text.doc_range.end, true);
                    for run in &mut text.runs {
                        for glyph in &mut run.glyphs {
                            glyph.byte_index = remap(glyph.byte_index, true);
                        }
                    }
                }
                LineFragment::Tab(tab) => {
                    tab.doc_range =
                        remap(tab.doc_range.start, true)..remap(tab.doc_range.end, false);
                }
                LineFragment::InlineBox(_) => {}
            }
        }
    }
    layout.hidden_ranges = seams.into_iter().map(|(_, range)| range).collect();
    layout
}

fn resolve_line_height(spec: LineHeight, natural: f32, zoom: f32) -> f32 {
    match spec {
        LineHeight::Multiplier(m) => (natural * m).max(natural),
        LineHeight::ExactPt(pt) => pt_to_px(pt, zoom).max(1.0),
    }
}

fn next_tab_stop(
    abs_x: f32,
    tab_stops: &[crate::model::TabStop],
    default_interval_px: f32,
    zoom: f32,
) -> f32 {
    let mut best: Option<f32> = None;
    for stop in tab_stops {
        let px = pt_to_px(stop.position_pt, zoom);
        if px > abs_x + 0.5 {
            best = Some(match best {
                Some(cur) => cur.min(px),
                None => px,
            });
        }
    }
    if let Some(target) = best {
        target
    } else {
        let step = ((abs_x + 0.5) / default_interval_px).floor() + 1.0;
        step * default_interval_px
    }
}

fn itemize_and_shape<S: TextShaper>(
    block_id: Option<BlockId>,
    content: &InlineContent,
    base_text: &ResolvedTextStyle,
    bidi: &ParagraphBidi,
    zoom: f32,
    shaper: &mut S,
) -> Vec<ShapedItem<S::Backend>> {
    let mut items = Vec::new();
    let mut pending_text: Vec<PendingRun> = Vec::new();

    let flush_pending =
        |pending: &mut Vec<PendingRun>, out: &mut Vec<ShapedItem<S::Backend>>, shaper: &mut S| {
            let (Some(first_run), Some(last_run)) = (pending.first(), pending.last()) else {
                return;
            };
            let segment_start = first_run.doc_range.start;
            let segment_end = last_run.doc_range.end;
            let first = &first_run.resolved;
            let bidi_level = first_run.bidi_level;
            let font_size_px = pt_to_px(first.font_size_pt, zoom).max(1.0);
            let baseline_shift_px = pt_to_px(first.baseline_shift_pt, zoom);

            let run_specs: Vec<ShaperRunSpec> = pending
                .iter()
                .map(|run| {
                    ShaperRunSpec::from_resolved(run.doc_range.len(), &run.resolved)
                        .with_bidi_level(run.bidi_level)
                })
                .collect();
            let decorations: Vec<DecorationSpan> = pending
                .iter()
                .filter(|run| {
                    run.resolved.background.is_some()
                        || run.resolved.underline.is_some()
                        || run.resolved.strikethrough
                        || run.resolved.inline_code
                        || run.resolved.link.is_some()
                })
                .map(|run| DecorationSpan {
                    doc_range: run.doc_range.clone(),
                    background: run.resolved.background,
                    underline: run.resolved.underline,
                    strikethrough: run.resolved.strikethrough,
                    inline_code: run.resolved.inline_code,
                    link: run.resolved.link.clone(),
                })
                .collect();

            let text_slice = content.text.get(segment_start..segment_end).unwrap_or("");
            let segment = shaper.shape_segment(text_slice, font_size_px, &run_specs);
            out.push(ShapedItem::Text {
                doc_range: segment_start..segment_end,
                font_size_px,
                baseline_shift_px,
                bidi_level,
                segment,
                decorations,
            });
            pending.clear();
        };

    for span in &content.spans {
        let resolved = span.style.resolve(base_text);
        let mut cursor = span.range.start;
        let mut cursor_level = bidi.level_at(cursor);

        for (rel_idx, ch) in content.text[span.range.clone()].char_indices() {
            let abs_idx = span.range.start + rel_idx;
            let ch_len = ch.len_utf8();
            let object = if ch == OBJECT_REPLACEMENT_CHAR {
                content.object_at(abs_idx)
            } else {
                None
            };
            if object.is_some() || ch == '\t' || ch == '\n' {
                if abs_idx > cursor {
                    push_text_run(
                        &mut pending_text,
                        cursor..abs_idx,
                        &resolved,
                        cursor_level,
                        zoom,
                        &mut items,
                        &flush_pending,
                        shaper,
                    );
                }
                flush_pending(&mut pending_text, &mut items, shaper);
                if ch == '\t' {
                    items.push(ShapedItem::Tab {
                        doc_range: abs_idx..(abs_idx + ch_len),
                    });
                } else if ch == '\n' {
                    items.push(ShapedItem::HardBreak {
                        doc_range: abs_idx..(abs_idx + ch_len),
                    });
                } else if let Some(entry) = object {
                    let object = shaper
                        .resolve_inline_object(&entry.object)
                        .unwrap_or_else(|| entry.object.clone());
                    let raw_metrics = shaper.measure_inline_object_with_style(
                        block_id, entry.id, &object, &resolved, zoom,
                    );
                    let metrics = if raw_metrics.is_valid() {
                        raw_metrics
                    } else {
                        InlineBoxMetrics::new(
                            pt_to_px(72.0, zoom),
                            pt_to_px(18.0, zoom),
                            InlineVAlign::Middle,
                        )
                    };
                    items.push(ShapedItem::InlineBox {
                        doc_range: abs_idx..(abs_idx + ch_len),
                        object_id: entry.id,
                        object,
                        metrics,
                        font_size_pt: resolved.font_size_pt,
                        text_color: resolved.color,
                    });
                }
                cursor = abs_idx + ch_len;
                cursor_level = bidi.level_at(cursor);
            } else {
                let char_level = bidi.level_at(abs_idx);
                if abs_idx > cursor && char_level != cursor_level {
                    push_text_run(
                        &mut pending_text,
                        cursor..abs_idx,
                        &resolved,
                        cursor_level,
                        zoom,
                        &mut items,
                        &flush_pending,
                        shaper,
                    );
                    cursor = abs_idx;
                    cursor_level = char_level;
                } else if abs_idx == cursor {
                    cursor_level = char_level;
                }
            }
        }

        if cursor < span.range.end {
            push_text_run(
                &mut pending_text,
                cursor..span.range.end,
                &resolved,
                cursor_level,
                zoom,
                &mut items,
                &flush_pending,
                shaper,
            );
        }
    }

    flush_pending(&mut pending_text, &mut items, shaper);
    items
}

#[allow(clippy::too_many_arguments)]
fn push_text_run<S: TextShaper, FFlush>(
    pending: &mut Vec<PendingRun>,
    doc_range: Range<usize>,
    resolved: &ResolvedTextStyle,
    bidi_level: u8,
    zoom: f32,
    items: &mut Vec<ShapedItem<S::Backend>>,
    flush: &FFlush,
    shaper: &mut S,
) where
    FFlush: Fn(&mut Vec<PendingRun>, &mut Vec<ShapedItem<S::Backend>>, &mut S),
{
    if let Some(prev) = pending.last() {
        let same_size = quantize_tenth(pt_to_px(prev.resolved.font_size_pt, zoom))
            == quantize_tenth(pt_to_px(resolved.font_size_pt, zoom));
        let same_shift = quantize_tenth(pt_to_px(prev.resolved.baseline_shift_pt, zoom))
            == quantize_tenth(pt_to_px(resolved.baseline_shift_pt, zoom));
        let same_bidi = prev.bidi_level == bidi_level;
        let contiguous = prev.doc_range.end == doc_range.start;
        if !(same_size && same_shift && same_bidi && contiguous) {
            flush(pending, items, shaper);
        }
    }
    pending.push(PendingRun {
        doc_range,
        resolved: resolved.clone(),
        bidi_level,
    });
}

fn slice_items_range<B: GlyphBackend>(
    items: &[ShapedItem<B>],
    range: Range<usize>,
) -> Vec<LineFragment<B>> {
    let mut out = Vec::new();
    for item in items {
        let ir = item.doc_range();
        if ir.end <= range.start || ir.start >= range.end {
            continue;
        }
        let s = ir.start.max(range.start);
        let e = ir.end.min(range.end);
        if s >= e {
            continue;
        }
        match item {
            ShapedItem::Text {
                doc_range,
                font_size_px,
                baseline_shift_px,
                bidi_level,
                segment,
                decorations,
            } => {
                let local_s = s - doc_range.start;
                let local_e = e - doc_range.start;
                let x_s = segment.x_at_byte(local_s);
                let x_e = segment.x_at_byte(local_e);
                let left_x = x_s.min(x_e);
                let right_x = x_s.max(x_e);
                let width = (right_x - left_x).max(0.0);

                let mut runs = Vec::new();
                for run in &segment.runs {
                    let mut glyphs = Vec::new();
                    for g in &run.glyphs {
                        if g.byte_index >= local_s && g.byte_index < local_e {
                            glyphs.push(ShapedGlyph {
                                glyph_id: g.glyph_id,
                                x: g.x - left_x,
                                y: g.y,
                                byte_index: doc_range.start + g.byte_index,
                                is_emoji: g.is_emoji,
                            });
                        }
                    }
                    if !glyphs.is_empty() {
                        runs.push(ShapedGlyphRun {
                            font: run.font,
                            color: run.color,
                            glyphs,
                        });
                    }
                }

                let mut x_for_byte = Vec::new();
                x_for_byte.push((s, (x_s - left_x).clamp(0.0, width)));
                for &(lb, lx) in &segment.x_for_byte {
                    if lb > local_s && lb < local_e {
                        x_for_byte.push((doc_range.start + lb, (lx - left_x).clamp(0.0, width)));
                    }
                }
                x_for_byte.push((e, (x_e - left_x).clamp(0.0, width)));

                let sliced_decs = decorations
                    .iter()
                    .filter_map(|d| {
                        let ds = d.doc_range.start.max(s);
                        let de = d.doc_range.end.min(e);
                        if ds < de {
                            Some(DecorationSpan {
                                doc_range: ds..de,
                                background: d.background,
                                underline: d.underline,
                                strikethrough: d.strikethrough,
                                inline_code: d.inline_code,
                                link: d.link.clone(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                out.push(LineFragment::Text(TextFragment {
                    doc_range: s..e,
                    x: 0.0,
                    width,
                    font_size_px: *font_size_px,
                    ascent: segment.ascent,
                    descent: segment.descent,
                    baseline_shift_px: *baseline_shift_px,
                    bidi_level: *bidi_level,
                    x_for_byte,
                    runs,
                    decorations: sliced_decs,
                }));
            }
            ShapedItem::InlineBox {
                doc_range,
                object_id,
                object,
                metrics,
                font_size_pt,
                text_color,
            } => {
                out.push(LineFragment::InlineBox(InlineBoxFragment {
                    doc_range: doc_range.clone(),
                    object_id: *object_id,
                    object: object.clone(),
                    x: 0.0,
                    width: metrics.width,
                    height: metrics.height,
                    depth: metrics.depth,
                    valign: metrics.valign,
                    font_size_pt: *font_size_pt,
                    text_color: *text_color,
                }));
            }
            ShapedItem::Tab { doc_range } => {
                out.push(LineFragment::Tab(TabFragment {
                    doc_range: doc_range.clone(),
                    x: 0.0,
                    width: 0.0,
                }));
            }
            ShapedItem::HardBreak { .. } => {}
        }
    }
    out
}

fn append_positioned_fragments<B: GlyphBackend>(
    line_frags: &mut Vec<LineFragment<B>>,
    cur_x: &mut f32,
    new_frags: Vec<LineFragment<B>>,
) {
    for mut frag in new_frags {
        let w = frag.width();
        let can_merge = matches!(
            (line_frags.last(), &frag),
            (Some(LineFragment::Text(prev)), LineFragment::Text(next))
                if prev.doc_range.end == next.doc_range.start
                    && quantize_tenth(prev.font_size_px) == quantize_tenth(next.font_size_px)
                    && quantize_tenth(prev.baseline_shift_px) == quantize_tenth(next.baseline_shift_px)
                    && prev.bidi_level == next.bidi_level
        );

        if can_merge
            && let Some(LineFragment::Text(prev)) = line_frags.last_mut()
            && let LineFragment::Text(next) = frag
        {
            if prev.is_rtl() {
                // Inside an RTL fragment, higher logical offsets sit visually to the left.
                let dx = next.width;
                for run in &mut prev.runs {
                    for g in &mut run.glyphs {
                        g.x += dx;
                    }
                }
                for (_, lx) in &mut prev.x_for_byte {
                    *lx += dx;
                }
                let mut combined_runs = next.runs;
                for run in prev.runs.drain(..) {
                    if let Some(last_run) = combined_runs.last_mut()
                        && last_run.font == run.font
                        && last_run.color == run.color
                    {
                        last_run.glyphs.extend(run.glyphs);
                    } else {
                        combined_runs.push(run);
                    }
                }
                prev.runs = combined_runs;
                for &(b, lx) in next.x_for_byte.iter().skip(1) {
                    prev.x_for_byte.push((b, lx));
                }
            } else {
                let dx = prev.width;
                for mut run in next.runs {
                    for g in &mut run.glyphs {
                        g.x += dx;
                    }
                    if let Some(last_run) = prev.runs.last_mut()
                        && last_run.font == run.font
                        && last_run.color == run.color
                    {
                        last_run.glyphs.extend(run.glyphs);
                    } else {
                        prev.runs.push(run);
                    }
                }
                for &(b, lx) in next.x_for_byte.iter().skip(1) {
                    prev.x_for_byte.push((b, lx + dx));
                }
            }
            for dec in next.decorations {
                if let Some(last_dec) = prev.decorations.last_mut()
                    && last_dec.doc_range.end == dec.doc_range.start
                    && last_dec.background == dec.background
                    && last_dec.underline == dec.underline
                    && last_dec.strikethrough == dec.strikethrough
                    && last_dec.inline_code == dec.inline_code
                    && last_dec.link == dec.link
                {
                    last_dec.doc_range.end = dec.doc_range.end;
                } else {
                    prev.decorations.push(dec);
                }
            }
            prev.doc_range.end = next.doc_range.end;
            prev.width += w;
            *cur_x += w;
        } else {
            frag.set_x(*cur_x);
            *cur_x += w;
            line_frags.push(frag);
        }
    }
}

/// Applies UAX #9 Rule L1 (trailing whitespace reset) and Rule L2 (level-based reversal)
/// to convert logical line fragments into visual left-to-right order.
fn reorder_visual_line_fragments<B: GlyphBackend>(
    fragments: &mut [LineFragment<B>],
    full_text: &str,
    bidi: &ParagraphBidi,
) {
    let base_level = bidi.base_level();
    let mut levels: Vec<u8> = fragments
        .iter()
        .map(|frag| match frag {
            LineFragment::Text(t) => t.bidi_level,
            LineFragment::InlineBox(b) => bidi.level_at(b.doc_range.start),
            LineFragment::Tab(_) => base_level,
        })
        .collect();

    // UAX #9 L1: reset trailing whitespace-only fragments to the paragraph base level.
    for (frag, level) in fragments.iter().zip(levels.iter_mut()).rev() {
        let is_whitespace_only = match frag {
            LineFragment::Text(t) => full_text
                .get(t.doc_range.clone())
                .is_some_and(|slice| slice.chars().all(char::is_whitespace)),
            LineFragment::Tab(_) => true,
            LineFragment::InlineBox(_) => false,
        };
        if is_whitespace_only {
            *level = base_level;
        } else {
            break;
        }
    }

    let max_level = levels.iter().copied().max().unwrap_or(0);
    let min_odd_level = levels
        .iter()
        .copied()
        .filter(|&level| level % 2 == 1)
        .min()
        .unwrap_or(max_level.saturating_add(1));

    if max_level >= min_odd_level {
        for target_level in (min_odd_level..=max_level).rev() {
            let mut idx = 0usize;
            while idx < fragments.len() {
                if levels[idx] >= target_level {
                    let start = idx;
                    while idx < fragments.len() && levels[idx] >= target_level {
                        idx += 1;
                    }
                    fragments[start..idx].reverse();
                    levels[start..idx].reverse();
                } else {
                    idx += 1;
                }
            }
        }
    }

    let mut visual_x = 0.0f32;
    for frag in fragments.iter_mut() {
        frag.set_x(visual_x);
        visual_x += frag.width();
    }
}

fn compute_visible_line_width<B: GlyphBackend>(
    fragments: &[LineFragment<B>],
    full_text: &str,
    soft_wrapped: bool,
) -> f32 {
    let Some(last) = fragments.last() else {
        return 0.0;
    };
    let mut right = last.x() + last.width();
    if !soft_wrapped {
        return right;
    }
    for frag in fragments.iter().rev() {
        match frag {
            LineFragment::Text(t) => {
                let slice = &full_text[t.doc_range.clone()];
                let trimmed = slice.trim_end_matches(' ');
                if trimmed.len() == slice.len() {
                    return t.x + t.width;
                }
                if !trimmed.is_empty() {
                    let trim_byte = t.doc_range.start + trimmed.len();
                    return t.x + t.local_x_for_byte(trim_byte);
                }
                right = t.x;
            }
            LineFragment::InlineBox(_) | LineFragment::Tab(_) => {
                return frag.x() + frag.width();
            }
        }
    }
    right
}

fn apply_justification<B: GlyphBackend>(
    fragments: &mut [LineFragment<B>],
    full_text: &str,
    extra_width: f32,
) {
    if fragments.is_empty() || extra_width <= 0.1 {
        return;
    }
    let line_end = fragments
        .iter()
        .map(|frag| frag.doc_range().end)
        .max()
        .unwrap_or(0);
    let trimmed_end = full_text
        .get(..line_end)
        .map(|slice| slice.trim_end_matches(' ').len())
        .unwrap_or(line_end);

    let mut space_positions = Vec::new();
    for frag in fragments.iter() {
        if let LineFragment::Text(t) = frag {
            for (rel, ch) in full_text[t.doc_range.clone()].char_indices() {
                let abs = t.doc_range.start + rel;
                if ch == ' ' && abs < trimmed_end {
                    space_positions.push(abs);
                }
            }
        }
    }

    if space_positions.is_empty() {
        return;
    }
    let per_space = extra_width / (space_positions.len() as f32);

    let mut cumulative_shift = 0.0f32;

    for frag in fragments.iter_mut() {
        match frag {
            LineFragment::Text(t) => {
                t.x += cumulative_shift;
                let local_spaces: Vec<usize> = space_positions
                    .iter()
                    .copied()
                    .filter(|&sp| sp >= t.doc_range.start && sp < t.doc_range.end)
                    .collect();
                if local_spaces.is_empty() {
                    continue;
                }
                let is_rtl = t.is_rtl();
                let shift_for_byte = |b: usize| -> f32 {
                    let count = if is_rtl {
                        local_spaces.iter().filter(|&&sp| sp >= b).count()
                    } else {
                        local_spaces.iter().filter(|&&sp| sp < b).count()
                    };
                    count as f32 * per_space
                };
                for run in &mut t.runs {
                    for g in &mut run.glyphs {
                        g.x += shift_for_byte(g.byte_index);
                    }
                }
                for (b, lx) in &mut t.x_for_byte {
                    *lx += shift_for_byte(*b);
                }
                let added = local_spaces.len() as f32 * per_space;
                t.width += added;
                cumulative_shift += added;
            }
            LineFragment::InlineBox(b) => {
                b.x += cumulative_shift;
            }
            LineFragment::Tab(t) => {
                t.x += cumulative_shift;
            }
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::layout::MockShaper;

    #[test]
    fn source_preserving_layout_keeps_collapsed_boundaries_and_glyph_offsets() {
        let content = InlineContent::plain("# key=\"é\"");
        let hidden = [0..2, 6..7, 9..10];
        let style = BlockStyle::default();
        let sheet = StyleSheet::default();
        let config = DocumentLayoutConfig::default();
        let layout = layout_source_paragraph(
            BlockId(1),
            &content,
            &hidden,
            &style,
            &sheet,
            &config,
            500.0,
            1.0,
            &mut MockShaper,
        );
        let caret = |offset| layout.caret_rect(offset, Affinity::Downstream);
        for range in hidden {
            assert_eq!(caret(range.start).x, caret(range.end).x);
            assert!(layout.selection_rects(range, false).is_empty());
        }
        let line = &layout.lines[0];
        assert_eq!(
            layout
                .hit_test(Point::new(caret(2).x, line.rect.center().y))
                .offset,
            2
        );
        assert_eq!(
            layout
                .hit_test(Point::new(caret(9).x + 4.0, line.rect.center().y))
                .offset,
            9
        );
        let bytes: Vec<_> = line
            .fragments
            .iter()
            .filter_map(|fragment| match fragment {
                LineFragment::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| &text.runs)
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.byte_index)
            .collect();
        assert_eq!(bytes, vec![2, 3, 4, 5, 7]);
    }
}
