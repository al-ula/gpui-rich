//! Linux COLRv0/COLRv1 color emoji rasterizer backed by `skrifa` + `tiny-skia`.
//!
//! Modern Linux distributions ship `Noto Color Emoji` as a COLRv1 vector color font without
//! `CBDT` bitmap strikes or filled base `glyf` outlines. GPUI's Linux text rasterizer (`swash`)
//! does not support COLRv1, leaving glyphs shaped from `Noto-COLRv1.ttf` invisible.
//!
//! This module detects a system COLRv1 emoji font (only when no `CBDT` strike is present),
//! rasterizes shaped glyph IDs on demand in full color into cached GPUI [`RenderImage`]s, and
//! compiles to a zero-cost no-op on non-Linux targets.

#[cfg(target_os = "linux")]
mod linux_impl {
    use gpui_kit::RenderImage;
    use image::{Frame, RgbaImage};
    use skrifa::color::{
        Brush, ColorPainter, ColorStop, CompositeMode, Extend, Transform as ColrTransform,
    };
    use skrifa::outline::{DrawSettings, OutlinePen};
    use skrifa::prelude::{GlyphId, LocationRef, Size};
    use skrifa::raw::TableProvider;
    use skrifa::{FontRef, MetadataProvider};
    use smallvec::SmallVec;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex, OnceLock};
    use tiny_skia::{
        BlendMode, Color, FillRule, GradientStop, LinearGradient, Mask, Paint, PathBuilder, Pixmap,
        PixmapPaint, Point as SkiaPoint, RadialGradient, Rect as SkiaRect, Shader, SpreadMode,
        Transform as SkiaTransform,
    };

    const MAX_CACHE_ENTRIES: usize = 512;

    struct ColrFontData {
        bytes: Vec<u8>,
        upem: f32,
        ascender: f32,
        descender: f32,
    }

    type ColrRasterCache = Mutex<HashMap<(u32, u32), Arc<RenderImage>>>;

    static COLR_FONT: OnceLock<Option<ColrFontData>> = OnceLock::new();
    static RASTER_CACHE: OnceLock<ColrRasterCache> = OnceLock::new();

    fn colr_font() -> Option<&'static ColrFontData> {
        COLR_FONT.get_or_init(load_system_colrv1_font).as_ref()
    }

    fn raster_cache() -> &'static ColrRasterCache {
        RASTER_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn try_load_colrv1_path(path: &Path) -> Option<ColrFontData> {
        let bytes = std::fs::read(path).ok()?;
        let font = FontRef::new(&bytes).ok()?;
        // If the font already contains CBDT bitmap strikes, GPUI's native `swash` rasterizer handles it.
        if font.table_data(skrifa::Tag::new(b"CBDT")).is_some() {
            return None;
        }
        let colr = font.colr().ok()?;
        let _cpal = font.cpal().ok()?;
        if colr.version() < 1 {
            return None;
        }
        let upem = f32::from(font.head().ok()?.units_per_em()).max(1.0);
        let (ascender, descender) = font
            .hhea()
            .ok()
            .map(|hhea| {
                (
                    f32::from(hhea.ascender().to_i16()),
                    f32::from(hhea.descender().to_i16()),
                )
            })
            .unwrap_or((upem * 0.9, -upem * 0.25));
        Some(ColrFontData {
            bytes,
            upem,
            ascender,
            descender,
        })
    }

    fn load_system_colrv1_font() -> Option<ColrFontData> {
        let known_paths = [
            "/usr/share/fonts/google-noto-color-emoji-fonts/Noto-COLRv1.ttf",
            "/usr/share/fonts/noto/Noto-COLRv1.ttf",
            "/usr/share/fonts/google-noto-emoji/Noto-COLRv1.ttf",
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
        ];
        for path in known_paths {
            if let Some(loaded) = try_load_colrv1_path(Path::new(path)) {
                return Some(loaded);
            }
        }

        let output = Command::new("fc-match")
            .args(["-f", "%{file}", "Noto Color Emoji"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path_str = String::from_utf8(output.stdout).ok()?;
        let trimmed = path_str.trim();
        if trimmed.is_empty() {
            return None;
        }
        try_load_colrv1_path(&PathBuf::from(trimmed))
    }

    pub(crate) fn is_available() -> bool {
        colr_font().is_some()
    }

    /// Resolves a single-codepoint (or base + variation-selector-16) emoji directly via `cmap`.
    pub(crate) fn lookup_single_codepoint_glyph_id(emoji: &str) -> Option<u32> {
        let data = colr_font()?;
        let font = FontRef::new(&data.bytes).ok()?;
        let mut chars = emoji.chars().filter(|&ch| ch != '\u{FE0F}');
        let first = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let glyph_id = font.charmap().map(first)?;
        let raw = glyph_id.to_u32();
        if raw == 0 || font.color_glyphs().get(glyph_id).is_none() {
            return None;
        }
        Some(raw)
    }

    /// Rasterizes `glyph_id` from the system COLRv1 font at `size_px` pixels square.
    pub(crate) fn rasterize_glyph_id(glyph_id: u32, size_px: u32) -> Option<Arc<RenderImage>> {
        let clamped_size = size_px.clamp(8, 128);
        let cache_key = (glyph_id, clamped_size);
        if let Ok(guard) = raster_cache().lock()
            && let Some(cached) = guard.get(&cache_key)
        {
            return Some(Arc::clone(cached));
        }

        let data = colr_font()?;
        let font = FontRef::new(&data.bytes).ok()?;
        let skrifa_gid = GlyphId::new(glyph_id);
        let color_glyph = font.color_glyphs().get(skrifa_gid)?;
        let cpal = font.cpal().ok()?;
        let palette = cpal.color_records_array()?.ok()?;

        let advance = font
            .hmtx()
            .ok()
            .map(|hmtx| f32::from(hmtx.advance(skrifa_gid).unwrap_or(data.upem as u16)))
            .unwrap_or(data.upem)
            .max(data.upem * 0.5);
        let line_height = (data.ascender - data.descender).max(data.upem);
        let extent = advance.max(line_height);
        let scale = (clamped_size as f32) / extent;
        let offset_x = ((clamped_size as f32) - advance * scale) * 0.5;
        let baseline_y =
            ((clamped_size as f32) - line_height * scale) * 0.5 + data.ascender * scale;

        let initial_transform =
            SkiaTransform::from_row(scale, 0.0, 0.0, -scale, offset_x, baseline_y);
        let base_pixmap = Pixmap::new(clamped_size, clamped_size)?;

        let mut painter = TinySkiaColorPainter {
            font: &font,
            palette,
            size_px: clamped_size,
            transforms: vec![initial_transform],
            clips: Vec::new(),
            layers: vec![(base_pixmap, BlendMode::SourceOver)],
        };

        if color_glyph
            .paint(LocationRef::default(), &mut painter)
            .is_err()
        {
            return None;
        }
        while painter.layers.len() > 1 {
            painter.pop_layer();
        }
        let (pixmap, _) = painter.layers.pop()?;
        let width = pixmap.width();
        let height = pixmap.height();
        let mut raw_bytes = pixmap.take();
        for pixel in raw_bytes.as_chunks_mut::<4>().0 {
            swap_rgba_premultiplied_to_bgra(pixel);
        }
        let buffer = RgbaImage::from_raw(width, height, raw_bytes)?;
        let rendered = Arc::new(RenderImage::new(SmallVec::from_const([Frame::new(buffer)])));

        if let Ok(mut guard) = raster_cache().lock() {
            if guard.len() >= MAX_CACHE_ENTRIES {
                guard.clear();
            }
            guard.insert(cache_key, Arc::clone(&rendered));
        }

        Some(rendered)
    }

    fn swap_rgba_premultiplied_to_bgra(pixel: &mut [u8; 4]) {
        let alpha = pixel[3];
        if alpha == 0 {
            return;
        }
        let (red, green, blue) = if alpha == 255 {
            (pixel[0], pixel[1], pixel[2])
        } else {
            let inv = 255.0 / f32::from(alpha);
            (
                (f32::from(pixel[0]) * inv).min(255.0).round() as u8,
                (f32::from(pixel[1]) * inv).min(255.0).round() as u8,
                (f32::from(pixel[2]) * inv).min(255.0).round() as u8,
            )
        };
        pixel[0] = blue;
        pixel[1] = green;
        pixel[2] = red;
    }

    struct SkiaOutlinePen {
        builder: PathBuilder,
    }

    impl SkiaOutlinePen {
        fn new() -> Self {
            Self {
                builder: PathBuilder::new(),
            }
        }

        fn finish(self) -> Option<tiny_skia::Path> {
            self.builder.finish()
        }
    }

    impl OutlinePen for SkiaOutlinePen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.builder.move_to(x, y);
        }

        fn line_to(&mut self, x: f32, y: f32) {
            self.builder.line_to(x, y);
        }

        fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
            self.builder.quad_to(cx0, cy0, x, y);
        }

        fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
            self.builder.cubic_to(cx0, cy0, cx1, cy1, x, y);
        }

        fn close(&mut self) {
            self.builder.close();
        }
    }

    struct TinySkiaColorPainter<'font> {
        font: &'font FontRef<'font>,
        palette: &'font [skrifa::color::Color],
        size_px: u32,
        transforms: Vec<SkiaTransform>,
        clips: Vec<Option<Mask>>,
        layers: Vec<(Pixmap, BlendMode)>,
    }

    impl TinySkiaColorPainter<'_> {
        fn current_transform(&self) -> SkiaTransform {
            self.transforms
                .last()
                .copied()
                .unwrap_or(SkiaTransform::identity())
        }

        fn resolve_color(&self, palette_index: u16, alpha: f32) -> Color {
            let clamped_alpha = alpha.clamp(0.0, 1.0);
            if palette_index == 0xFFFF {
                return Color::from_rgba(0.15, 0.15, 0.15, clamped_alpha).unwrap_or(Color::BLACK);
            }
            if let Some(record) = self.palette.get(usize::from(palette_index)) {
                let red = f32::from(record.red) / 255.0;
                let green = f32::from(record.green) / 255.0;
                let blue = f32::from(record.blue) / 255.0;
                let combined_alpha = (f32::from(record.alpha) / 255.0) * clamped_alpha;
                Color::from_rgba(red, green, blue, combined_alpha.clamp(0.0, 1.0))
                    .unwrap_or(Color::BLACK)
            } else {
                Color::from_rgba(0.0, 0.0, 0.0, clamped_alpha).unwrap_or(Color::BLACK)
            }
        }

        fn build_gradient_stops(&self, stops: &[ColorStop]) -> Vec<GradientStop> {
            let mut result = Vec::with_capacity(stops.len());
            for stop in stops {
                let color = self.resolve_color(stop.palette_index, stop.alpha);
                result.push(GradientStop::new(stop.offset.clamp(0.0, 1.0), color));
            }
            result
        }

        fn build_glyph_path(&self, glyph_id: GlyphId) -> Option<tiny_skia::Path> {
            let outline = self.font.outline_glyphs().get(glyph_id)?;
            let mut pen = SkiaOutlinePen::new();
            let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
            outline.draw(settings, &mut pen).ok()?;
            pen.finish()
        }

        fn push_clip_path(&mut self, path: Option<tiny_skia::Path>) {
            let Some(mut mask) = Mask::new(self.size_px, self.size_px) else {
                self.clips.push(None);
                return;
            };
            if let Some(path) = path {
                mask.fill_path(&path, FillRule::Winding, true, self.current_transform());
            }
            if let Some(Some(parent)) = self.clips.last() {
                let parent_data = parent.data();
                for (dst, &src) in mask.data_mut().iter_mut().zip(parent_data.iter()) {
                    *dst = ((u16::from(*dst) * u16::from(src)) / 255) as u8;
                }
            }
            self.clips.push(Some(mask));
        }

        fn build_paint<'a>(&self, brush: &Brush<'_>) -> Option<Paint<'a>> {
            let transform = self.current_transform();
            let shader = match brush {
                Brush::Solid {
                    palette_index,
                    alpha,
                } => Shader::SolidColor(self.resolve_color(*palette_index, *alpha)),
                Brush::LinearGradient {
                    p0,
                    p1,
                    color_stops,
                    extend,
                } => {
                    let first_color = color_stops.first().map_or(Color::TRANSPARENT, |stop| {
                        self.resolve_color(stop.palette_index, stop.alpha)
                    });
                    let stops = self.build_gradient_stops(color_stops);
                    if stops.len() <= 1 {
                        Shader::SolidColor(first_color)
                    } else {
                        LinearGradient::new(
                            SkiaPoint::from_xy(p0.x, p0.y),
                            SkiaPoint::from_xy(p1.x, p1.y),
                            stops,
                            map_extend(*extend),
                            transform,
                        )
                        .unwrap_or(Shader::SolidColor(first_color))
                    }
                }
                Brush::RadialGradient {
                    c0,
                    r0,
                    c1,
                    r1,
                    color_stops,
                    extend,
                } => {
                    let first_color = color_stops.first().map_or(Color::TRANSPARENT, |stop| {
                        self.resolve_color(stop.palette_index, stop.alpha)
                    });
                    let stops = self.build_gradient_stops(color_stops);
                    if stops.len() <= 1 {
                        Shader::SolidColor(first_color)
                    } else {
                        let radius = r1.max(*r0).max(0.01);
                        let start_pt = SkiaPoint::from_xy(c0.x, c0.y);
                        let end_pt = SkiaPoint::from_xy(c1.x, c1.y);
                        RadialGradient::new(
                            start_pt,
                            end_pt,
                            radius,
                            stops,
                            map_extend(*extend),
                            transform,
                        )
                        .unwrap_or(Shader::SolidColor(first_color))
                    }
                }
                Brush::SweepGradient { color_stops, .. } => {
                    let first_color = color_stops.first().map_or(Color::TRANSPARENT, |stop| {
                        self.resolve_color(stop.palette_index, stop.alpha)
                    });
                    Shader::SolidColor(first_color)
                }
            };
            Some(Paint {
                shader,
                blend_mode: BlendMode::SourceOver,
                anti_alias: true,
                force_hq_pipeline: false,
            })
        }
    }

    impl ColorPainter for TinySkiaColorPainter<'_> {
        fn push_transform(&mut self, transform: ColrTransform) {
            let next = SkiaTransform::from_row(
                transform.xx,
                transform.yx,
                transform.xy,
                transform.yy,
                transform.dx,
                transform.dy,
            );
            let combined = self.current_transform().pre_concat(next);
            self.transforms.push(combined);
        }

        fn pop_transform(&mut self) {
            if self.transforms.len() > 1 {
                self.transforms.pop();
            }
        }

        fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
            let path = self.build_glyph_path(glyph_id);
            self.push_clip_path(path);
        }

        fn push_clip_box(&mut self, clip_box: skrifa::metrics::BoundingBox) {
            let path = SkiaRect::from_ltrb(
                clip_box.x_min,
                clip_box.y_min,
                clip_box.x_max,
                clip_box.y_max,
            )
            .map(PathBuilder::from_rect);
            self.push_clip_path(path);
        }

        fn pop_clip(&mut self) {
            self.clips.pop();
        }

        fn fill(&mut self, brush: Brush<'_>) {
            let Some(paint) = self.build_paint(&brush) else {
                return;
            };
            let Some(full_rect) =
                SkiaRect::from_xywh(0.0, 0.0, self.size_px as f32, self.size_px as f32)
            else {
                return;
            };
            let clip_mask = self.clips.last().and_then(|mask| mask.as_ref());
            if let Some((pixmap, _)) = self.layers.last_mut() {
                pixmap.fill_rect(full_rect, &paint, SkiaTransform::identity(), clip_mask);
            }
        }

        fn fill_glyph(
            &mut self,
            glyph_id: GlyphId,
            brush_transform: Option<ColrTransform>,
            brush: Brush<'_>,
        ) {
            let Some(path) = self.build_glyph_path(glyph_id) else {
                return;
            };
            let glyph_transform = self.current_transform();
            if let Some(extra) = brush_transform {
                self.push_transform(extra);
            }
            let paint = self.build_paint(&brush);
            if brush_transform.is_some() {
                self.pop_transform();
            }
            let Some(paint) = paint else {
                return;
            };
            let clip_mask = self.clips.last().and_then(|mask| mask.as_ref());
            if let Some((pixmap, _)) = self.layers.last_mut() {
                pixmap.fill_path(&path, &paint, FillRule::Winding, glyph_transform, clip_mask);
            }
        }

        fn push_layer(&mut self, composite_mode: CompositeMode) {
            if let Some(layer_pixmap) = Pixmap::new(self.size_px, self.size_px) {
                self.layers
                    .push((layer_pixmap, map_composite_mode(composite_mode)));
            }
        }

        fn pop_layer(&mut self) {
            if self.layers.len() <= 1 {
                return;
            }
            let Some((top_pixmap, blend_mode)) = self.layers.pop() else {
                return;
            };
            let clip_mask = self.clips.last().and_then(|mask| mask.as_ref());
            if let Some((parent_pixmap, _)) = self.layers.last_mut() {
                let paint = PixmapPaint {
                    opacity: 1.0,
                    blend_mode,
                    quality: tiny_skia::FilterQuality::Bilinear,
                };
                parent_pixmap.draw_pixmap(
                    0,
                    0,
                    top_pixmap.as_ref(),
                    &paint,
                    SkiaTransform::identity(),
                    clip_mask,
                );
            }
        }
    }

    fn map_extend(extend: Extend) -> SpreadMode {
        match extend {
            Extend::Repeat => SpreadMode::Repeat,
            Extend::Reflect => SpreadMode::Reflect,
            _ => SpreadMode::Pad,
        }
    }

    fn map_composite_mode(mode: CompositeMode) -> BlendMode {
        match mode {
            CompositeMode::Clear => BlendMode::Clear,
            CompositeMode::Src => BlendMode::Source,
            CompositeMode::Dest => BlendMode::Destination,
            CompositeMode::SrcOver => BlendMode::SourceOver,
            CompositeMode::DestOver => BlendMode::DestinationOver,
            CompositeMode::SrcIn => BlendMode::SourceIn,
            CompositeMode::DestIn => BlendMode::DestinationIn,
            CompositeMode::SrcOut => BlendMode::SourceOut,
            CompositeMode::DestOut => BlendMode::DestinationOut,
            CompositeMode::SrcAtop => BlendMode::SourceAtop,
            CompositeMode::DestAtop => BlendMode::DestinationAtop,
            CompositeMode::Xor => BlendMode::Xor,
            CompositeMode::Plus => BlendMode::Plus,
            CompositeMode::Screen => BlendMode::Screen,
            CompositeMode::Overlay => BlendMode::Overlay,
            CompositeMode::Darken => BlendMode::Darken,
            CompositeMode::Lighten => BlendMode::Lighten,
            CompositeMode::ColorDodge => BlendMode::ColorDodge,
            CompositeMode::ColorBurn => BlendMode::ColorBurn,
            CompositeMode::HardLight => BlendMode::HardLight,
            CompositeMode::SoftLight => BlendMode::SoftLight,
            CompositeMode::Difference => BlendMode::Difference,
            CompositeMode::Exclusion => BlendMode::Exclusion,
            CompositeMode::Multiply => BlendMode::Multiply,
            CompositeMode::HslHue => BlendMode::Hue,
            CompositeMode::HslSaturation => BlendMode::Saturation,
            CompositeMode::HslColor => BlendMode::Color,
            CompositeMode::HslLuminosity => BlendMode::Luminosity,
            _ => BlendMode::SourceOver,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux_impl::{is_available, lookup_single_codepoint_glyph_id, rasterize_glyph_id};

#[cfg(not(target_os = "linux"))]
pub(crate) fn is_available() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn lookup_single_codepoint_glyph_id(_emoji: &str) -> Option<u32> {
    None
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn rasterize_glyph_id(
    _glyph_id: u32,
    _size_px: u32,
) -> Option<std::sync::Arc<gpui_kit::RenderImage>> {
    None
}
