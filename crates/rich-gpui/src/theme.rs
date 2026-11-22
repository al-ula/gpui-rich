//! Visual theme tokens for [`crate::RichDocumentElement`] with Light/Dark presets and
//! `gpui-kit` [`gpui_kit::base::Theme`] synchronization.

use crate::shaper::rgba_to_hsla;
use gpui_kit::{App, Hsla, base::Theme as GpuiBaseTheme, base::ThemeAppearance};
use rich_model::{ChipKind, RgbaColor, TableBorderStyle};

/// Color tokens for a single [`ChipKind`] smart chip pill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartChipColors {
    pub background: Hsla,
    pub border: Hsla,
    pub text: Hsla,
}

/// Complete smart chip palette indexed by [`ChipKind`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartChipPalette {
    pub person: SmartChipColors,
    pub date: SmartChipColors,
    pub document: SmartChipColors,
    pub tag: SmartChipColors,
}

impl SmartChipPalette {
    /// Default light-mode smart chip palette.
    pub fn light() -> Self {
        Self {
            person: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgb(0xe8, 0xf0, 0xfe)),
                border: rgba_to_hsla(RgbaColor::rgb(0xae, 0xcb, 0xfa)),
                text: rgba_to_hsla(RgbaColor::rgb(0x17, 0x4e, 0xa6)),
            },
            date: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgb(0xfe, 0xf3, 0xc7)),
                border: rgba_to_hsla(RgbaColor::rgb(0xfc, 0xd3, 0x4d)),
                text: rgba_to_hsla(RgbaColor::rgb(0x92, 0x40, 0x0e)),
            },
            document: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgb(0xdc, 0xfc, 0xe7)),
                border: rgba_to_hsla(RgbaColor::rgb(0x86, 0xef, 0xac)),
                text: rgba_to_hsla(RgbaColor::rgb(0x16, 0x65, 0x34)),
            },
            tag: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgb(0xf3, 0xe8, 0xff)),
                border: rgba_to_hsla(RgbaColor::rgb(0xd8, 0xb4, 0xfe)),
                text: rgba_to_hsla(RgbaColor::rgb(0x6b, 0x21, 0xa8)),
            },
        }
    }

    /// Default dark-mode smart chip palette.
    pub fn dark() -> Self {
        Self {
            person: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgba(0x1f, 0x6f, 0xeb, 0x38)),
                border: rgba_to_hsla(RgbaColor::rgba(0x58, 0xa6, 0xff, 0x70)),
                text: rgba_to_hsla(RgbaColor::rgb(0xa5, 0xd6, 0xff)),
            },
            date: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgba(0xd2, 0x99, 0x22, 0x38)),
                border: rgba_to_hsla(RgbaColor::rgba(0xe3, 0xb3, 0x41, 0x70)),
                text: rgba_to_hsla(RgbaColor::rgb(0xf8, 0xe3, 0xa1)),
            },
            document: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgba(0x23, 0x86, 0x36, 0x38)),
                border: rgba_to_hsla(RgbaColor::rgba(0x3f, 0xb9, 0x50, 0x70)),
                text: rgba_to_hsla(RgbaColor::rgb(0x7e, 0xe7, 0x87)),
            },
            tag: SmartChipColors {
                background: rgba_to_hsla(RgbaColor::rgba(0x89, 0x57, 0xe5, 0x38)),
                border: rgba_to_hsla(RgbaColor::rgba(0xbc, 0x8c, 0xff, 0x70)),
                text: rgba_to_hsla(RgbaColor::rgb(0xd2, 0xa8, 0xff)),
            },
        }
    }

    /// Returns the chip color tokens for `kind`.
    pub fn for_kind(&self, kind: ChipKind) -> SmartChipColors {
        match kind {
            ChipKind::Person => self.person,
            ChipKind::Date => self.date,
            ChipKind::Document => self.document,
            ChipKind::Tag => self.tag,
        }
    }
}

/// Complete visual token set controlling all painted surfaces, chrome, text defaults,
/// and overlays in [`crate::RichDocumentElement`].
#[derive(Clone, Debug, PartialEq)]
pub struct RichTheme {
    /// Backdrop behind the infinite-length paper sheet in `HorizontalLayout::Paper` mode.
    pub desk_background: Hsla,
    /// Primary document surface (paper sheet fill in `Paper` mode, full canvas in `Fill` mode).
    pub surface_background: Hsla,
    /// Border around the continuous paper sheet.
    pub paper_border: Hsla,
    /// Drop shadow beneath the continuous paper sheet.
    pub paper_shadow: Hsla,
    /// Primary body and heading text color (remaps black text across light/dark themes).
    pub text: Hsla,
    /// Inverse text color (white in light themes, black/dark in dark themes).
    pub inverse_text: Hsla,
    /// Muted chrome color for subtitles, list markers, chevrons, code tags, and placeholders.
    pub muted_text: Hsla,
    /// Hyperlink and footnote reference color (remaps semantic default [`RgbaColor::LINK`]).
    pub link: Hsla,
    /// Translucent selection highlight painted on Layer 2.
    pub selection_background: Hsla,
    /// Caret indicator color painted on Layer 5.
    pub caret: Hsla,
    /// Primary accent fill used for checked checklist boxes.
    pub accent: Hsla,
    /// Contrasting foreground painted inside checked checklist boxes.
    pub accent_foreground: Hsla,
    /// Vertical accent rail painted for blockquotes.
    pub blockquote_rail: Hsla,
    /// Code block frame fill color.
    pub code_background: Hsla,
    /// Code block frame border color.
    pub code_border: Hsla,
    /// Inline code pill background color.
    pub inline_code_background: Hsla,
    /// Maximum alpha applied to inline text highlight backgrounds so selection and theme contrast remain legible.
    pub highlight_max_opacity: f32,
    /// Table header cell default background fill.
    pub table_header_background: Hsla,
    /// Table body cell default background fill.
    pub table_cell_background: Hsla,
    /// Default table border color when a table uses [`TableBorderStyle::default`].
    pub table_border: Hsla,
    /// Horizontal rule stroke color.
    pub rule_color: Hsla,
    /// Inline image placeholder background fill.
    pub image_placeholder_background: Hsla,
    /// Inline image placeholder border color.
    pub image_placeholder_border: Hsla,
    /// Scrollbar thumb fill color.
    pub scrollbar_thumb: Hsla,
    /// Debug overlay line bounding-box outline color.
    pub debug_line_outline: Hsla,
    /// Debug overlay baseline rule color.
    pub debug_baseline: Hsla,
    /// Smart chip color palettes.
    pub smart_chips: SmartChipPalette,
}

impl Default for RichTheme {
    fn default() -> Self {
        Self::light()
    }
}

impl RichTheme {
    /// Default light document theme.
    pub fn light() -> Self {
        Self {
            desk_background: rgba_to_hsla(RgbaColor::rgb(0xee, 0xf2, 0xf6)),
            surface_background: rgba_to_hsla(RgbaColor::WHITE),
            paper_border: rgba_to_hsla(RgbaColor::rgb(0xd8, 0xde, 0xe4)),
            paper_shadow: rgba_to_hsla(RgbaColor::rgba(0x1f, 0x23, 0x28, 0x1c)),
            text: rgba_to_hsla(RgbaColor::BLACK),
            inverse_text: rgba_to_hsla(RgbaColor::WHITE),
            muted_text: rgba_to_hsla(RgbaColor::MUTED),
            link: rgba_to_hsla(RgbaColor::LINK),
            selection_background: rgba_to_hsla(RgbaColor::rgba(0x0b, 0x57, 0xd0, 0x38)),
            caret: rgba_to_hsla(RgbaColor::LINK),
            accent: rgba_to_hsla(RgbaColor::LINK),
            accent_foreground: rgba_to_hsla(RgbaColor::WHITE),
            blockquote_rail: rgba_to_hsla(RgbaColor::rgb(0xd0, 0xd7, 0xde)),
            code_background: rgba_to_hsla(RgbaColor::rgb(0xf6, 0xf8, 0xfa)),
            code_border: rgba_to_hsla(RgbaColor::rgb(0xd0, 0xd7, 0xde)),
            inline_code_background: rgba_to_hsla(RgbaColor::INLINE_CODE_BG),
            highlight_max_opacity: 0.42,
            table_header_background: rgba_to_hsla(RgbaColor::rgb(0xf6, 0xf8, 0xfa)),
            table_cell_background: rgba_to_hsla(RgbaColor::WHITE),
            table_border: rgba_to_hsla(TableBorderStyle::default().color),
            rule_color: rgba_to_hsla(RgbaColor::rgb(0xd8, 0xde, 0xe4)),
            image_placeholder_background: rgba_to_hsla(RgbaColor::rgb(0xee, 0xf2, 0xf6)),
            image_placeholder_border: rgba_to_hsla(RgbaColor::rgb(0xbd, 0xc8, 0xd4)),
            scrollbar_thumb: rgba_to_hsla(RgbaColor::rgba(0x59, 0x63, 0x6e, 0x66)),
            debug_line_outline: rgba_to_hsla(RgbaColor::rgba(0x09, 0x69, 0xda, 0x55)),
            debug_baseline: rgba_to_hsla(RgbaColor::rgba(0xcf, 0x22, 0x2e, 0x77)),
            smart_chips: SmartChipPalette::light(),
        }
    }

    /// High-contrast dark document theme.
    pub fn dark() -> Self {
        Self {
            desk_background: rgba_to_hsla(RgbaColor::rgb(0x0d, 0x11, 0x17)),
            surface_background: rgba_to_hsla(RgbaColor::rgb(0x16, 0x1b, 0x22)),
            paper_border: rgba_to_hsla(RgbaColor::rgb(0x30, 0x36, 0x3d)),
            paper_shadow: rgba_to_hsla(RgbaColor::rgba(0x00, 0x00, 0x00, 0x66)),
            text: rgba_to_hsla(RgbaColor::rgb(0xe6, 0xed, 0xf3)),
            inverse_text: rgba_to_hsla(RgbaColor::rgb(0x16, 0x1b, 0x22)),
            muted_text: rgba_to_hsla(RgbaColor::rgb(0x8b, 0x94, 0x9e)),
            link: rgba_to_hsla(RgbaColor::rgb(0x58, 0xa6, 0xff)),
            selection_background: rgba_to_hsla(RgbaColor::rgba(0x38, 0x8b, 0xfd, 0x48)),
            caret: rgba_to_hsla(RgbaColor::rgb(0x58, 0xa6, 0xff)),
            accent: rgba_to_hsla(RgbaColor::rgb(0x58, 0xa6, 0xff)),
            accent_foreground: rgba_to_hsla(RgbaColor::rgb(0x0d, 0x11, 0x17)),
            blockquote_rail: rgba_to_hsla(RgbaColor::rgb(0x3b, 0x43, 0x4b)),
            code_background: rgba_to_hsla(RgbaColor::rgb(0x1f, 0x26, 0x30)),
            code_border: rgba_to_hsla(RgbaColor::rgb(0x30, 0x36, 0x3d)),
            inline_code_background: rgba_to_hsla(RgbaColor::rgba(0x6e, 0x76, 0x81, 0x55)),
            highlight_max_opacity: 0.32,
            table_header_background: rgba_to_hsla(RgbaColor::rgb(0x1c, 0x22, 0x2b)),
            table_cell_background: rgba_to_hsla(RgbaColor::rgb(0x16, 0x1b, 0x22)),
            table_border: rgba_to_hsla(RgbaColor::rgb(0x30, 0x36, 0x3d)),
            rule_color: rgba_to_hsla(RgbaColor::rgb(0x30, 0x36, 0x3d)),
            image_placeholder_background: rgba_to_hsla(RgbaColor::rgb(0x1f, 0x26, 0x30)),
            image_placeholder_border: rgba_to_hsla(RgbaColor::rgb(0x3b, 0x43, 0x4b)),
            scrollbar_thumb: rgba_to_hsla(RgbaColor::rgba(0x8b, 0x94, 0x9e, 0x66)),
            debug_line_outline: rgba_to_hsla(RgbaColor::rgba(0x58, 0xa6, 0xff, 0x66)),
            debug_baseline: rgba_to_hsla(RgbaColor::rgba(0xff, 0x7b, 0x72, 0x88)),
            smart_chips: SmartChipPalette::dark(),
        }
    }

    /// Derives a [`RichTheme`] from a `gpui-kit` base [`gpui_kit::base::Theme`].
    pub fn from_gpui_theme(theme: &GpuiBaseTheme) -> Self {
        let is_dark = theme.appearance == ThemeAppearance::Dark;
        let mut resolved = if is_dark {
            Self::dark()
        } else {
            Self::light()
        };
        let colors = &theme.tokens.colors;

        resolved.surface_background = colors.surface;
        resolved.desk_background = if colors.background != colors.surface || is_dark {
            colors.background
        } else {
            colors.muted
        };
        resolved.paper_border = colors.border;
        resolved.text = colors.surface_foreground;
        resolved.inverse_text = colors.surface;
        resolved.muted_text = colors.muted_foreground;
        resolved.selection_background = colors.selection;
        resolved.caret = colors.primary;
        resolved.accent = colors.primary;
        resolved.accent_foreground = colors.primary_foreground;
        resolved.blockquote_rail = colors.border;
        resolved.code_background = colors.muted;
        resolved.code_border = colors.border;
        resolved.inline_code_background = colors.muted.alpha(0.65);
        resolved.table_header_background = colors.muted;
        resolved.table_cell_background = colors.surface;
        resolved.table_border = colors.border;
        resolved.rule_color = colors.border;
        resolved.image_placeholder_background = colors.muted;
        resolved.image_placeholder_border = colors.border;
        resolved.scrollbar_thumb = colors.muted_foreground.alpha(0.4);
        resolved
    }

    /// Reads the active [`gpui_kit::base::Theme`] global from `cx` and adapts it into a [`RichTheme`].
    pub fn from_app(cx: &App) -> Self {
        let base_theme = GpuiBaseTheme::global(cx);
        Self::from_gpui_theme(&base_theme)
    }

    /// Returns true when [`Self::surface_background`] is a dark surface.
    pub fn is_dark(&self) -> bool {
        self.surface_background.l < 0.5
    }

    /// Maps document text colors into the active theme as a display-only transform:
    /// semantic defaults (`BLACK`/`#000000`, `WHITE`/`#ffffff`, `MUTED`, `LINK`) resolve to theme
    /// tokens, grayscale tones invert across the dark surface ramp in dark mode, and low-lightness
    /// chromatic colors lift lightness while preserving hue.
    pub fn resolve_text_color(&self, color: RgbaColor) -> Hsla {
        if color == RgbaColor::BLACK || color == RgbaColor::rgb(0x00, 0x00, 0x00) {
            return self.text;
        }
        if color == RgbaColor::WHITE {
            return self.inverse_text;
        }
        if color == RgbaColor::MUTED {
            return self.muted_text;
        }
        if color == RgbaColor::LINK {
            return self.link;
        }

        let mut resolved = rgba_to_hsla(color);
        if self.is_dark() {
            if resolved.s <= 0.08 {
                resolved.l = self.text.l + (self.inverse_text.l - self.text.l) * resolved.l;
            } else if resolved.l < 0.55 {
                resolved.l = (1.0 - resolved.l).clamp(0.58, 0.82);
            }
        }
        resolved
    }

    /// Maps the default inline-code background to [`Self::inline_code_background`] and caps inline
    /// highlight opacity at [`Self::highlight_max_opacity`] so selection and themed text stay legible.
    pub fn resolve_inline_background(&self, background: RgbaColor, inline_code: bool) -> Hsla {
        if inline_code && background == RgbaColor::INLINE_CODE_BG {
            self.inline_code_background
        } else {
            let mut resolved = rgba_to_hsla(background);
            resolved.a = resolved.a.min(self.highlight_max_opacity);
            resolved
        }
    }

    /// Maps the default table border color to [`Self::table_border`] while preserving custom table borders.
    pub fn resolve_table_border(&self, border_color: RgbaColor) -> Hsla {
        if border_color == TableBorderStyle::default().color {
            self.table_border
        } else {
            rgba_to_hsla(border_color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_inverts_grayscale_ramp_and_lifts_dark_chromatic_colors() {
        let light = RichTheme::light();
        let dark = RichTheme::dark();

        assert!(!light.is_dark());
        assert!(dark.is_dark());

        // Grayscale tones invert monotonically across dark theme lightness.
        let black = dark.resolve_text_color(RgbaColor::rgb(0x00, 0x00, 0x00));
        let dark_gray = dark.resolve_text_color(RgbaColor::rgb(0x33, 0x33, 0x33));
        let mid_gray = dark.resolve_text_color(RgbaColor::rgb(0x80, 0x80, 0x80));
        let light_gray = dark.resolve_text_color(RgbaColor::rgb(0xdd, 0xdd, 0xdd));
        let white = dark.resolve_text_color(RgbaColor::rgb(0xff, 0xff, 0xff));
        assert!(black.l > dark_gray.l);
        assert!(dark_gray.l > mid_gray.l);
        assert!(mid_gray.l > light_gray.l);
        assert!(light_gray.l > white.l);

        // Dark chromatic text lifts lightness in dark mode while preserving hue and literal light mode.
        let deep_green = RgbaColor::rgb(0x16, 0x65, 0x34);
        let light_green = light.resolve_text_color(deep_green);
        let dark_green = dark.resolve_text_color(deep_green);
        assert!((light_green.h - dark_green.h).abs() < 0.01);
        assert!(light_green.l < 0.35);
        assert!(dark_green.l >= 0.58);
    }
}
