//! GPUI implementation of [`crate::layout::TextShaper`] backed by `WindowTextSystem`.

use crate::gpui::extension::{
    BlockCallbackResult, ExtensionRegistry, InlineCallbackResult,
};
use gpui_kit::{
    Font, FontFallbacks, FontFeatures, FontId, FontStyle, FontWeight as GpuiFontWeight, GlyphId,
    Hsla, Rgba, SharedString, TextRun, Window, px,
};
use crate::layout::{
    GlyphBackend, InlineBoxMetrics, Insets, ShapedGlyph, ShapedGlyphRun, ShapedSegment,
    ShaperRunSpec, Size, TextShaper, is_valid_custom_insets, is_valid_custom_size, pt_to_px,
};
use crate::model::{
    AnchorId, BlockShapeKind, ChipKind, ComponentStore, CustomPayload, DROPDOWN_KIND, InlineObject,
    InlineVAlign, PLACEHOLDER_KIND, RgbaColor, VARIABLE_KIND, footnote_reference_id,
};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

const PREFERRED_SANS_FONTS: &[&str] = &[
    "IBM Plex Sans",
    "Inter",
    "Noto Sans",
    "Liberation Sans",
    "DejaVu Sans",
    "Ubuntu",
    "Roboto",
    "Helvetica",
    "Segoe UI",
    "Arial",
];

const PREFERRED_SERIF_FONTS: &[&str] = &[
    "Georgia",
    "Noto Serif",
    "Liberation Serif",
    "DejaVu Serif",
    "Times New Roman",
    "Times",
    "PT Serif",
    "Merriweather",
];

const PREFERRED_MONOSPACE_FONTS: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "Liberation Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
    "Hack",
    "Source Code Pro",
    "Menlo",
    "Consolas",
    "Courier New",
];

static FONT_RESOLUTION_CACHE: OnceLock<RwLock<HashMap<String, SharedString>>> = OnceLock::new();

fn font_cache() -> &'static RwLock<HashMap<String, SharedString>> {
    FONT_RESOLUTION_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Clears the cached font-family resolution table (call after registering custom fonts via
/// `text_system().add_fonts(...)`).
pub fn clear_font_resolution_cache() {
    if let Ok(mut guard) = font_cache().write() {
        guard.clear();
    }
}

/// Scores how completely `family` resolves distinct Regular, Bold, Italic, and Bold-Italic faces.
///
/// GPUI falls back to a static Regular/Normal font stack when a requested family (such as Linux's
/// default `.SystemUIFont` -> `IBM Plex Sans`) is not installed, silently dropping bold and italic.
fn font_variant_score(family: &SharedString, window: &Window) -> u8 {
    let text_system = window.text_system();
    let probe = |weight: GpuiFontWeight, style: FontStyle| {
        text_system.resolve_font(&Font {
            family: family.clone(),
            features: FontFeatures::default(),
            fallbacks: Some(FontFallbacks::default()),
            weight,
            style,
        })
    };
    let regular = probe(GpuiFontWeight::NORMAL, FontStyle::Normal);
    let bold = probe(GpuiFontWeight::BOLD, FontStyle::Normal);
    let italic = probe(GpuiFontWeight::NORMAL, FontStyle::Italic);
    let bold_italic = probe(GpuiFontWeight::BOLD, FontStyle::Italic);

    let has_bold = bold != regular;
    let has_italic = italic != regular;
    let has_bold_italic = bold_italic != regular && bold_italic != bold && bold_italic != italic;

    match (has_bold, has_italic, has_bold_italic) {
        (true, true, true) => 3,
        (true, true, false) => 2,
        (false, true, _) => 1,
        _ => 0,
    }
}

fn find_best_installed(
    preferred: &[&str],
    installed: &[String],
    substring_filter: impl Fn(&str) -> bool,
    window: &Window,
) -> Option<SharedString> {
    let mut best: Option<(u8, SharedString)> = None;

    for target in preferred {
        if let Some(found) = installed
            .iter()
            .find(|name| name.eq_ignore_ascii_case(target))
        {
            let candidate = SharedString::from(found.clone());
            let score = font_variant_score(&candidate, window);
            if score >= 3 {
                return Some(candidate);
            }
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best = Some((score, candidate));
            }
        }
    }

    if best.as_ref().is_none_or(|(score, _)| *score < 2) {
        for found in installed {
            let lower = found.to_ascii_lowercase();
            if substring_filter(&lower) {
                let candidate = SharedString::from(found.clone());
                let score = font_variant_score(&candidate, window);
                if score >= 3 {
                    return Some(candidate);
                }
                if best
                    .as_ref()
                    .is_none_or(|(best_score, _)| score > *best_score)
                {
                    best = Some((score, candidate));
                }
            }
        }
    }

    best.map(|(_, family)| family)
}

fn resolve_sans_uncached(installed: &[String], window: &Window) -> SharedString {
    let system_default = SharedString::from(".SystemUIFont");
    let default_score = font_variant_score(&system_default, window);
    if default_score >= 3 {
        return system_default;
    }
    find_best_installed(
        PREFERRED_SANS_FONTS,
        installed,
        |lower| lower.contains("sans") && !lower.contains("mono"),
        window,
    )
    .unwrap_or(system_default)
}

fn resolve_serif_uncached(installed: &[String], window: &Window) -> SharedString {
    find_best_installed(
        PREFERRED_SERIF_FONTS,
        installed,
        |lower| lower.contains("serif") && !lower.contains("sans"),
        window,
    )
    .unwrap_or_else(|| resolve_sans_uncached(installed, window))
}

fn resolve_monospace_uncached(installed: &[String], window: &Window) -> SharedString {
    find_best_installed(
        PREFERRED_MONOSPACE_FONTS,
        installed,
        |lower| lower.contains("mono"),
        window,
    )
    .unwrap_or_else(|| resolve_sans_uncached(installed, window))
}

fn metric_compatible_substitutes(lower_name: &str) -> Option<&'static [&'static str]> {
    match lower_name {
        "arial" | "helvetica" | "helvetica neue" => {
            Some(&["Liberation Sans", "Arimo", "Noto Sans", "DejaVu Sans"])
        }
        "calibri" | "carlito" | "segoe ui" | "san francisco" | "-apple-system" | "system-ui"
        | "inter" | "roboto" | "open sans" => {
            Some(&["Carlito", "Noto Sans", "Liberation Sans", "DejaVu Sans"])
        }
        "times new roman" | "times" => {
            Some(&["Liberation Serif", "Tinos", "Noto Serif", "DejaVu Serif"])
        }
        "georgia" | "cambria" | "garamond" | "palatino" | "baskerville" => {
            Some(&["Gelasio", "Caladea", "Noto Serif", "Liberation Serif", "DejaVu Serif"])
        }
        "courier new" | "courier" => {
            Some(&["Liberation Mono", "Cousine", "DejaVu Sans Mono", "Noto Sans Mono"])
        }
        "consolas" | "menlo" | "monaco" | "sf mono" | "fira code" | "jetbrains mono"
        | "source code pro" => {
            Some(&["JetBrains Mono", "Liberation Mono", "DejaVu Sans Mono", "Noto Sans Mono"])
        }
        _ => None,
    }
}

/// Resolves a model font family name (generic aliases or concrete families) to an installed GPUI
/// font family with metric-compatible cross-OS substitution and bold/italic preservation.
pub fn resolve_font_family(requested: &str, window: &Window) -> SharedString {
    let trimmed = requested.trim();
    let cache_key = trimmed.to_ascii_lowercase();

    if let Ok(guard) = font_cache().read()
        && let Some(cached) = guard.get(&cache_key)
    {
        return cached.clone();
    }

    let installed = window.text_system().all_font_names();
    let resolved = if trimmed.is_empty()
        || trimmed == ".SystemUIFont"
        || cache_key == "sans-serif"
        || cache_key == "sans"
    {
        resolve_sans_uncached(&installed, window)
    } else if cache_key == "serif" {
        resolve_serif_uncached(&installed, window)
    } else if cache_key == "monospace" || cache_key == "mono" {
        resolve_monospace_uncached(&installed, window)
    } else if let Some(exact) = installed
        .iter()
        .find(|name| name.eq_ignore_ascii_case(trimmed))
    {
        SharedString::from(exact.clone())
    } else if let Some(substitutes) = metric_compatible_substitutes(&cache_key) {
        find_best_installed(substitutes, &installed, |_| false, window)
            .unwrap_or_else(|| resolve_sans_uncached(&installed, window))
    } else {
        resolve_sans_uncached(&installed, window)
    };

    if let Ok(mut guard) = font_cache().write() {
        guard.insert(cache_key, resolved.clone());
    }
    resolved
}

/// Glyph backend binding `layout` directly to GPUI's native `FontId` and `GlyphId`.
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

/// Builds a GPUI [`Font`] from a `ShaperRunSpec`, resolving generic aliases and missing cross-OS
/// families to installed faces with bold and italic support.
pub fn to_gpui_font(run: &ShaperRunSpec, window: &Window) -> Font {
    let family = resolve_font_family(run.font_family.as_ref(), window);
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

/// Render-time font resolver allowing host applications to override `(font_family, weight, italic)`
/// mapping into a GPUI [`Font`] without mutating [`crate::model::Document`].
///
/// Return `Some(font)` to supply a custom GPUI [`Font`], or `None` to fall back to [`to_gpui_font`].
pub type FontResolver = dyn Fn(&ShaperRunSpec, &Window) -> Option<Font>;

/// Shapes uniform-size segments through GPUI's native `WindowTextSystem`.
pub struct GpuiShaper<'window, 'registry> {
    window: &'window Window,
    extensions: Option<&'registry ExtensionRegistry>,
    font_resolver: Option<&'registry FontResolver>,
    components: Option<&'registry ComponentStore>,
    footnote_numbers: Option<Arc<HashMap<AnchorId, u32>>>,
}

impl<'window> GpuiShaper<'window, 'window> {
    /// Creates a shaper borrowing the current GPUI `Window`.
    pub fn new(window: &'window Window) -> Self {
        Self {
            window,
            extensions: None,
            font_resolver: None,
            components: None,
            footnote_numbers: None,
        }
    }
}

impl<'window, 'registry> GpuiShaper<'window, 'registry> {
    /// Creates a shaper borrowing `window` and an [`ExtensionRegistry`] for custom node sizing.
    pub fn with_extensions(
        window: &'window Window,
        extensions: &'registry ExtensionRegistry,
    ) -> Self {
        Self {
            window,
            extensions: Some(extensions),
            font_resolver: None,
            components: None,
            footnote_numbers: None,
        }
    }

    /// Attaches an optional render-time [`FontResolver`] for per-variant or custom font mapping.
    pub fn with_optional_font_resolver(
        mut self,
        font_resolver: Option<&'registry FontResolver>,
    ) -> Self {
        self.font_resolver = font_resolver;
        self
    }
    /// Resolves component labels against the owning document's canonical definitions.
    pub fn with_components(mut self, components: &'registry ComponentStore) -> Self {
        self.components = Some(components);
        self
    }

}

impl TextShaper for GpuiShaper<'_, '_> {
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
                let font = self
                    .font_resolver
                    .and_then(|resolver| resolver(run, self.window))
                    .unwrap_or_else(|| to_gpui_font(run, self.window));
                color_ranges.push((byte_cursor..end, run.color));
                gpui_runs.push(TextRun {
                    len: end - byte_cursor,
                    font,
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
                font: gpui_kit::font(".SystemUIFont"),
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

        let is_rtl = runs.first().is_some_and(|run| run.bidi_level % 2 == 1);
        let total_width: f32 = shaped_line.width.into();

        if is_rtl {
            // Sort shaped clusters visually left-to-right; in RTL, each cluster's logical start
            // lies on its visual right edge while logical end (`text.len()`) lies at x = 0.
            let mut visual_clusters: Vec<(usize, f32)> = x_for_byte;
            visual_clusters.sort_by(|left, right| {
                left.1
                    .total_cmp(&right.1)
                    .then_with(|| right.0.cmp(&left.0))
            });
            visual_clusters.dedup_by_key(|(byte_offset, _)| *byte_offset);

            let mut rtl_x_for_byte: Vec<(usize, f32)> =
                Vec::with_capacity(visual_clusters.len() + 2);
            for (idx, &(byte_offset, _)) in visual_clusters.iter().enumerate() {
                let right_edge = visual_clusters
                    .get(idx + 1)
                    .map(|&(_, next_x)| next_x)
                    .unwrap_or(total_width);
                rtl_x_for_byte.push((byte_offset, right_edge));
            }
            rtl_x_for_byte.sort_by_key(|&(byte_offset, _)| byte_offset);
            rtl_x_for_byte.dedup_by_key(|(byte_offset, _)| *byte_offset);
            if rtl_x_for_byte.first().map(|&(b, _)| b) != Some(0) {
                rtl_x_for_byte.insert(0, (0, total_width));
            }
            if rtl_x_for_byte.last().map(|&(b, _)| b) != Some(text.len()) {
                rtl_x_for_byte.push((text.len(), 0.0));
            }
            x_for_byte = rtl_x_for_byte;
        } else {
            x_for_byte.sort_by_key(|&(byte_offset, _)| byte_offset);
            x_for_byte.dedup_by_key(|(byte_offset, _)| *byte_offset);
            if x_for_byte.first().map(|&(b, _)| b) != Some(0) {
                x_for_byte.insert(0, (0, 0.0));
            }
            if x_for_byte.last().map(|&(b, _)| b) != Some(text.len()) {
                x_for_byte.push((text.len(), total_width));
            }
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
                    font: gpui_kit::font(".SystemUIFont"),
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
                    font: gpui_kit::font(".SystemUIFont"),
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
            InlineObject::Custom { .. } if footnote_reference_id(object).is_some() => {
                InlineBoxMetrics {
                    width: pt_to_px(15.0, zoom),
                    height: pt_to_px(12.0, zoom),
                    valign: InlineVAlign::Baseline,
                }
            }
            InlineObject::Custom { kind, data } => {
                if let Some(renderer) = self
                    .extensions
                    .and_then(|registry| registry.get_inline(kind))
                {
                    if let InlineCallbackResult::Ok(metrics) =
                        renderer.measure(data, zoom, self.window)
                        && metrics.is_valid()
                    {
                        return metrics;
                    }
                }
                let label = kind.to_string();
                let font_size = px(pt_to_px(10.0, zoom));
                let run = TextRun {
                    len: label.len(),
                    font: gpui_kit::font(".SystemUIFont"),
                    color: rgba_to_hsla(RgbaColor::BLACK),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = self.window.text_system().shape_line(
                    SharedString::from(label),
                    font_size,
                    &[run],
                    None,
                );
                let label_width: f32 = shaped.width.into();
                InlineBoxMetrics {
                    width: label_width + pt_to_px(16.0, zoom),
                    height: pt_to_px(18.0, zoom),
                    valign: InlineVAlign::Middle,
                }
            }
        }
    }

    fn resolve_inline_object(&self, object: &InlineObject) -> Option<InlineObject> {
        if let Some(id) = footnote_reference_id(object) {
            return self.footnote_numbers.as_ref()?.get(&id)
                .map(|&index| InlineObject::FootnoteRef { index });
        }
        let InlineObject::Custom { kind, data } = object else { return None; };
        if !matches!(kind.as_ref(), DROPDOWN_KIND | VARIABLE_KIND | PLACEHOLDER_KIND) {
            return None;
        }
        let components = self.components?;
        let json = data.as_json().ok()?;
        let mut label = components.inline_fallback(kind, &json);
        if kind.as_ref() == DROPDOWN_KIND {
            label.push_str(" ▾");
        }
        Some(InlineObject::SmartChip { kind: ChipKind::Tag, label, accent: None })
    }

    fn set_footnote_numbers(&mut self, numbers: &Arc<HashMap<AnchorId, u32>>) {
        if !self.footnote_numbers.as_ref().is_some_and(|current| Arc::ptr_eq(current, numbers)) {
            self.footnote_numbers = Some(Arc::clone(numbers));
        }
    }

    fn measure_custom_block(
        &mut self,
        kind: &str,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
    ) -> Size {
        if let Some(renderer) = self
            .extensions
            .and_then(|registry| registry.get_block(kind))
            && let BlockCallbackResult::Ok(measured) =
                renderer.measure_leaf(data, max_width, zoom, self.window)
            && is_valid_custom_size(measured)
        {
            return measured;
        }
        Size::new(max_width, pt_to_px(44.0, zoom))
    }

    fn measure_custom_chrome(
        &mut self,
        shape: BlockShapeKind,
        kind: &str,
        data: &CustomPayload,
        max_width: f32,
        zoom: f32,
    ) -> Insets {
        if let Some(renderer) = self
            .extensions
            .and_then(|registry| registry.get_block(kind))
            && let BlockCallbackResult::Ok(measured) =
                renderer.measure_chrome(shape, data, max_width, zoom, self.window)
            && is_valid_custom_insets(measured)
        {
            return measured;
        }
        Insets::xy(pt_to_px(12.0, zoom), pt_to_px(8.0, zoom))
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
