//! Built-in pure-Rust LaTeX equation compiler backed by RaTeX (gated on `feature = "equation"`).

use crate::model::equation::{
    EquationBackend, EquationDisplayMode, EquationRenderError, EquationRenderOptions, EquationSvg,
};
use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
use ratex_parser::parse;
use ratex_svg::{SvgColorSyntax, SvgOptions, render_to_svg_with_color_syntax};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;

/// Pure-Rust [`EquationBackend`] powered by RaTeX with embedded KaTeX fonts and resvg-compatible
/// path-outlined SVG output.
///
/// Works identically on native desktop targets and WebAssembly without JavaScript, webviews, or
/// installed OS math fonts.
#[derive(Clone, Debug, Default)]
pub struct BuiltInEquationBackend;

impl BuiltInEquationBackend {
    /// Creates a built-in RaTeX backend instance.
    pub const fn new() -> Self {
        Self
    }
}

impl EquationBackend for BuiltInEquationBackend {
    fn render_svg(
        &self,
        source: &str,
        options: &EquationRenderOptions,
    ) -> Result<EquationSvg, EquationRenderError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(EquationRenderError::new("Equation source is empty"));
        }

        let font_size_pt = if options.font_size_pt.is_finite() && options.font_size_pt > 0.0 {
            options.font_size_pt
        } else {
            12.0
        };

        let ast = parse(trimmed).map_err(|error| {
            EquationRenderError::with_offset(
                error.message,
                error.loc.map(|location| location.start),
            )
        })?;

        let math_style = match options.mode {
            EquationDisplayMode::Inline => MathStyle::Text,
            EquationDisplayMode::Display => MathStyle::Display,
        };
        let color = Color::new(
            f32::from(options.color.r) / 255.0,
            f32::from(options.color.g) / 255.0,
            f32::from(options.color.b) / 255.0,
            f32::from(options.color.a) / 255.0,
        );
        let layout_options = LayoutOptions::default()
            .with_style(math_style)
            .with_color(color);

        let layout_box = layout(&ast, &layout_options);
        if let Err(error) = validate_layout(&layout_box) {
            return Err(EquationRenderError::new(error.to_string()));
        }

        let display_list = to_display_list(&layout_box);
        let em_size = f64::from(font_size_pt);
        let svg_options = SvgOptions {
            font_size: em_size,
            padding: 0.0,
            stroke_width: (em_size * 0.0375).max(0.4),
            embed_glyphs: true,
            font_dir: String::new(),
        };
        let svg = render_to_svg_with_color_syntax(&display_list, &svg_options, SvgColorSyntax::Rgb);

        let width_pt = (display_list.width as f32 * font_size_pt).max(1.0);
        let ascent_pt = (display_list.height as f32 * font_size_pt).max(0.0);
        let depth_pt = (display_list.depth as f32 * font_size_pt).max(0.0);
        let height_pt = (ascent_pt + depth_pt).max(1.0);

        Ok(EquationSvg {
            svg,
            width_pt,
            height_pt,
            depth_pt: depth_pt.min(height_pt),
        })
    }
}
