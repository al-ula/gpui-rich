//! GPUI implementation of [`rich_layout::TextShaper`] backed by `WindowTextSystem`.

use gpui::{
    Font, FontFallbacks, FontFeatures, FontId, FontStyle, FontWeight as GpuiFontWeight, GlyphId,
    Hsla, Rgba, SharedString, TextRun, Window, px,
};
use rich_layout::{
    GlyphBackend, InlineBoxMetrics, ShapedGlyph, ShapedGlyphRun, ShapedSegment, ShaperRunSpec,
    TextShaper, pt_to_px,
};
use rich_model::{InlineObject, InlineVAlign, RgbaColor};
use std::sync::OnceLock;

const PREFERRED_MONOSPACE_FONTS: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
    "Hack",
    "Source Code Pro",
    "Menlo",
    "Consolas",
    "Courier New",
];

static RESOLVED_MONOSPACE_FAMILY: OnceLock<SharedString> = OnceLock::new();

fn resolve_monospace_family(window: &Window) -> SharedString {
    RESOLVED_MONOSPACE_FAMILY
        .get_or_init(|| {
            let installed = window.text_system().all_font_names();
            for preferred in PREFERRED_MONOSPACE_FONTS {
                if let Some(found) = installed.iter().find(|name| name.as_str() == *preferred) {
                    return SharedString::from(found.clone());
                }
            }
            if let Some(found) = installed
                .iter()
                .find(|name| name.to_ascii_lowercase().contains("mono"))
            {
                return SharedString::from(found.clone());
            }
            SharedString::from(".SystemUIFont")
        })
        .clone()
}

/// Glyph backend binding `rich-layout` directly to GPUI's native `FontId` and `GlyphId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct GpuiGlyphBackend;

impl GlyphBackend for GpuiGlyphBackend {
    type Font = FontId;
    type Glyph = GlyphId;
}

/// Converts a model RGBA color into GPUI `Hsla`.
pub fn rgba_to_hsla(color: RgbaColor) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
    .into()
}

/// Builds a GPUI [`Font`] from a `ShaperRunSpec`, resolving generic `"monospace"` to an installed face.
pub fn to_gpui_font(run: &ShaperRunSpec, window: &Window) -> Font {
    let family = if run.font_family.as_ref().eq_ignore_ascii_case("monospace") {
        resolve_monospace_family(window)
    } else {
        SharedString::from(run.font_family.as_ref().to_owned())
    };
    Font {
        family,
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::default()),
        weight: GpuiFontWeight(f32::from(run.weight.0)),
        style: if run.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

/// Shapes uniform-size segments through GPUI's native `WindowTextSystem`.
pub struct GpuiShaper<'window> {
    window: &'window Window,
}

impl<'window> GpuiShaper<'window> {
    /// Creates a shaper borrowing the current GPUI `Window`.
    pub fn new(window: &'window Window) -> Self {
        Self { window }
    }
}

impl TextShaper for GpuiShaper<'_> {
    type Backend = GpuiGlyphBackend;

    fn shape_segment(
        &mut self,
        text: &str,
        font_size_px: f32,
        runs: &[ShaperRunSpec],
    ) -> ShapedSegment<Self::Backend> {
        let mut gpui_runs = Vec::with_capacity(runs.len().max(1));
        let mut color_ranges: Vec<(std::ops::Range<usize>, RgbaColor)> =
            Vec::with_capacity(runs.len());

        let mut byte_cursor = 0usize;
        for run in runs {
            let end = (byte_cursor + run.len).min(text.len());
            if end > byte_cursor {
                color_ranges.push((byte_cursor..end, run.color));
                gpui_runs.push(TextRun {
                    len: end - byte_cursor,
                    font: to_gpui_font(run, self.window),
                    color: rgba_to_hsla(run.color),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
                byte_cursor = end;
            }
        }

        if gpui_runs.is_empty() && !text.is_empty() {
            color_ranges.push((0..text.len(), RgbaColor::BLACK));
            gpui_runs.push(TextRun {
                len: text.len(),
                font: gpui::font(".SystemUIFont"),
                color: rgba_to_hsla(RgbaColor::BLACK),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }

        let shaped_line = self.window.text_system().shape_line(
            SharedString::from(text.to_owned()),
            px(font_size_px),
            &gpui_runs,
            None,
        );

        // GPUI merges adjacent shaped glyphs sharing one FontId even when their colors differ,
        // so regroup glyphs by `(font_id, color_at_byte)` to preserve per-span foreground colors.
        let mut output_runs: Vec<ShapedGlyphRun<GpuiGlyphBackend>> = Vec::new();
        let mut x_for_byte: Vec<(usize, f32)> = Vec::new();

        for shaped_run in &shaped_line.runs {
            for glyph in &shaped_run.glyphs {
                let glyph_x: f32 = glyph.position.x.into();
                let glyph_y: f32 = glyph.position.y.into();
                x_for_byte.push((glyph.index, glyph_x));

                let color = color_for_byte(&color_ranges, glyph.index);
                let converted = ShapedGlyph {
                    glyph_id: glyph.id,
                    x: glyph_x,
                    y: glyph_y,
                    byte_index: glyph.index,
                    is_emoji: glyph.is_emoji,
                };

                if let Some(last_run) = output_runs.last_mut()
                    && last_run.font == shaped_run.font_id
                    && last_run.color == color
                {
                    last_run.glyphs.push(converted);
                } else {
                    output_runs.push(ShapedGlyphRun {
                        font: shaped_run.font_id,
                        color,
                        glyphs: vec![converted],
                    });
                }
            }
        }

        let total_width: f32 = shaped_line.width.into();
        x_for_byte.sort_by_key(|&(byte_offset, _)| byte_offset);
        x_for_byte.dedup_by_key(|(byte_offset, _)| *byte_offset);
        if x_for_byte.first().map(|&(b, _)| b) != Some(0) {
            x_for_byte.insert(0, (0, 0.0));
        }
        if x_for_byte.last().map(|&(b, _)| b) != Some(text.len()) {
            x_for_byte.push((text.len(), total_width));
        }

        ShapedSegment {
            font_size_px,
            ascent: shaped_line.ascent.into(),
            descent: shaped_line.descent.into(),
            width: total_width,
            x_for_byte,
            runs: output_runs,
        }
    }

    fn measure_inline_object(&mut self, object: &InlineObject, zoom: f32) -> InlineBoxMetrics {
        match object {
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
                let font_size = px(pt_to_px(10.0, zoom));
                let run = TextRun {
                    len: label.len(),
                    font: gpui::font(".SystemUIFont"),
                    color: rgba_to_hsla(RgbaColor::BLACK),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = self.window.text_system().shape_line(
                    SharedString::from(label.clone()),
                    font_size,
                    &[run],
                    None,
                );
                let label_width: f32 = shaped.width.into();
                InlineBoxMetrics {
                    width: label_width + pt_to_px(18.0, zoom),
                    height: pt_to_px(18.0, zoom),
                    valign: InlineVAlign::Middle,
                }
            }
            InlineObject::FootnoteRef { index } => {
                let text = format!("[{index}]");
                let font_size = px(pt_to_px(8.5, zoom));
                let run = TextRun {
                    len: text.len(),
                    font: gpui::font(".SystemUIFont"),
                    color: rgba_to_hsla(RgbaColor::LINK),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = self.window.text_system().shape_line(
                    SharedString::from(text),
                    font_size,
                    &[run],
                    None,
                );
                let text_width: f32 = shaped.width.into();
                InlineBoxMetrics {
                    width: text_width + pt_to_px(4.0, zoom),
                    height: pt_to_px(12.0, zoom),
                    valign: InlineVAlign::Baseline,
                }
            }
        }
    }
}

fn color_for_byte(ranges: &[(std::ops::Range<usize>, RgbaColor)], byte_offset: usize) -> RgbaColor {
    for (range, color) in ranges {
        if range.contains(&byte_offset) {
            return *color;
        }
    }
    ranges
        .last()
        .map(|(_, color)| *color)
        .unwrap_or(RgbaColor::BLACK)
}
