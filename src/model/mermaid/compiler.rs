//! Built-in pure-Rust Mermaid compiler backed by `merman` (gated on `feature = "mermaid"`).

use crate::model::mermaid::{MermaidBackend, MermaidRenderError, MermaidSvg, MermaidThemePalette};
use merman::svg::{DiagramThemeCompiler, DiagramThemeSpec, MermaidThemeCompatibility, SvgPipeline};
use merman::{MermaidThemeId, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};

/// Pure-Rust [`MermaidBackend`] powered by `merman` with resvg-compatible SVG output.
///
/// Compiles Mermaid diagrams without a browser, JavaScript runtime, or external CLI, and works
/// identically on native desktop targets and WebAssembly.
#[derive(Clone, Debug, Default)]
pub struct BuiltInMermaidBackend;

impl BuiltInMermaidBackend {
    /// Creates a built-in `merman` backend instance.
    pub const fn new() -> Self {
        Self
    }
}

impl MermaidBackend for BuiltInMermaidBackend {
    fn render_svg(
        &self,
        source: &str,
        palette: &MermaidThemePalette,
    ) -> Result<MermaidSvg, MermaidRenderError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(MermaidRenderError::new("Mermaid diagram source is empty"));
        }

        let svg_request = SvgRequest {
            pipeline: Some(SvgPipeline::resvg_safe()),
            ..SvgRequest::default()
        };

        let mut request = RenderRequest::svg(trimmed, OperationControl::new(), svg_request);

        let theme_id = if palette.dark {
            MermaidThemeId::Dark
        } else {
            MermaidThemeId::Default
        };
        let mut compatibility = MermaidThemeCompatibility::default().with_theme_id(theme_id);
        if let Ok(updated) = compatibility.clone().with_dark_mode(palette.dark) {
            compatibility = updated;
        }
        let theme_spec = DiagramThemeSpec::new().with_mermaid_compatibility(compatibility);
        if let Ok(compiled_theme) = DiagramThemeCompiler::new().compile(theme_spec) {
            request = request.with_theme(compiled_theme);
        }

        let output = Renderer::new()
            .render(request)
            .map_err(|error| MermaidRenderError::new(error.to_string()))?;

        match output {
            RenderOutput::Svg(Some(svg_output)) => Ok(MermaidSvg::from_svg_with_fallback(
                svg_output.svg(),
                480.0,
                260.0,
            )),
            RenderOutput::Svg(None) => Err(MermaidRenderError::new(
                "Unrecognized or empty Mermaid diagram",
            )),
            _ => Err(MermaidRenderError::new(
                "Mermaid renderer returned an unexpected output format",
            )),
        }
    }
}
