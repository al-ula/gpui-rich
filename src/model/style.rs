use crate::model::command::MarkKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub type SharedStr = Arc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const BLACK: Self = Self::rgb(0x1f, 0x23, 0x28);
    pub const WHITE: Self = Self::rgb(0xff, 0xff, 0xff);
    pub const MUTED: Self = Self::rgb(0x59, 0x63, 0x6e);
    pub const LINK: Self = Self::rgb(0x0b, 0x57, 0xd0);
    pub const INLINE_CODE_BG: Self = Self::rgba(0x9a, 0xa5, 0xb1, 0x35);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const NORMAL: Self = Self(400);
    pub const MEDIUM: Self = Self(500);
    pub const SEMIBOLD: Self = Self(600);
    pub const BOLD: Self = Self(700);
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnderlineKind {
    Solid,
    Wavy(RgbaColor),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum BaselineShift {
    #[default]
    Normal,
    Superscript,
    Subscript,
    CustomPt(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommentId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkTarget {
    pub href: String,
    pub title: Option<String>,
}

/// Partial inline formatting attributes. `None` inherits from block/named style defaults.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct InlineStyle {
    pub font_family: Option<SharedStr>,
    pub font_size_pt: Option<f32>,
    pub weight: Option<FontWeight>,
    pub italic: Option<bool>,
    pub underline: Option<UnderlineKind>,
    pub strikethrough: Option<bool>,
    pub baseline_shift: Option<BaselineShift>,
    pub color: Option<RgbaColor>,
    pub background: Option<RgbaColor>,
    pub inline_code: Option<bool>,
    pub link: Option<LinkTarget>,
    pub comment_ids: Vec<CommentId>,
}

impl InlineStyle {
    pub fn bold() -> Self {
        Self {
            weight: Some(FontWeight::BOLD),
            ..Self::default()
        }
    }

    pub fn italic() -> Self {
        Self {
            italic: Some(true),
            ..Self::default()
        }
    }

    pub fn code() -> Self {
        Self {
            inline_code: Some(true),
            ..Self::default()
        }
    }

    pub fn with_font_size(mut self, pt: f32) -> Self {
        self.font_size_pt = Some(pt);
        self
    }

    pub fn with_weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = Some(italic);
        self
    }

    pub fn with_underline(mut self, underline: UnderlineKind) -> Self {
        self.underline = Some(underline);
        self
    }

    pub fn with_strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = Some(strikethrough);
        self
    }

    pub fn with_baseline_shift(mut self, shift: BaselineShift) -> Self {
        self.baseline_shift = Some(shift);
        self
    }

    pub fn with_color(mut self, color: RgbaColor) -> Self {
        self.color = Some(color);
        self
    }

    pub fn with_background(mut self, color: RgbaColor) -> Self {
        self.background = Some(color);
        self
    }

    pub fn with_font_family(mut self, family: impl Into<SharedStr>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    pub fn with_link(mut self, href: impl Into<String>) -> Self {
        self.link = Some(LinkTarget {
            href: href.into(),
            title: None,
        });
        self
    }

    /// Merges `overlay` onto `self`, preferring `Some` values from `overlay`.
    pub fn merge(&self, overlay: &InlineStyle) -> InlineStyle {
        let mut comment_ids = self.comment_ids.clone();
        for id in &overlay.comment_ids {
            if !comment_ids.contains(id) {
                comment_ids.push(*id);
            }
        }
        InlineStyle {
            font_family: overlay
                .font_family
                .clone()
                .or_else(|| self.font_family.clone()),
            font_size_pt: overlay.font_size_pt.or(self.font_size_pt),
            weight: overlay.weight.or(self.weight),
            italic: overlay.italic.or(self.italic),
            underline: overlay.underline.or(self.underline),
            strikethrough: overlay.strikethrough.or(self.strikethrough),
            baseline_shift: overlay.baseline_shift.or(self.baseline_shift),
            color: overlay.color.or(self.color),
            background: overlay.background.or(self.background),
            inline_code: overlay.inline_code.or(self.inline_code),
            link: overlay.link.clone().or_else(|| self.link.clone()),
            comment_ids,
        }
    }

    pub fn resolve(&self, base: &ResolvedTextStyle) -> ResolvedTextStyle {
        let inline_code = self.inline_code.unwrap_or(base.inline_code);
        let base_size = self.font_size_pt.unwrap_or(base.font_size_pt);
        let baseline_shift = self.baseline_shift.unwrap_or(base.baseline_shift);

        let (font_size_pt, baseline_shift_pt) = match baseline_shift {
            BaselineShift::Normal => (base_size, 0.0),
            BaselineShift::Superscript => (base_size * 0.68, base_size * 0.36),
            BaselineShift::Subscript => (base_size * 0.68, -base_size * 0.22),
            BaselineShift::CustomPt(pt) => (base_size, pt),
        };

        let link = self.link.clone().or_else(|| base.link.clone());
        let font_family = self.font_family.clone().unwrap_or_else(|| {
            if inline_code {
                Arc::from("monospace")
            } else {
                base.font_family.clone()
            }
        });

        let color = self.color.unwrap_or_else(|| {
            if link.is_some() {
                RgbaColor::LINK
            } else {
                base.color
            }
        });

        let underline = self.underline.or_else(|| {
            if link.is_some() {
                Some(UnderlineKind::Solid)
            } else {
                base.underline
            }
        });

        let background = self.background.or_else(|| {
            if inline_code {
                Some(RgbaColor::INLINE_CODE_BG)
            } else {
                base.background
            }
        });

        ResolvedTextStyle {
            font_family,
            font_size_pt: if inline_code {
                font_size_pt * 0.92
            } else {
                font_size_pt
            },
            weight: self.weight.unwrap_or(base.weight),
            italic: self.italic.unwrap_or(base.italic),
            underline,
            strikethrough: self.strikethrough.unwrap_or(base.strikethrough),
            baseline_shift,
            baseline_shift_pt,
            color,
            background,
            inline_code,
            link,
        }
    }

    /// Returns a copy with all visual formatting overrides reset to `None` while preserving
    /// hyperlink targets and comment anchors.
    pub fn cleared_formatting(&self) -> Self {
        Self {
            link: self.link.clone(),
            comment_ids: self.comment_ids.clone(),
            ..Self::default()
        }
    }

    /// Returns whether `mark` is effectively active when `self` resolves over `base`.
    pub fn is_mark_active(&self, mark: MarkKind, base: &ResolvedTextStyle) -> bool {
        self.resolve(base).is_mark_active(mark)
    }

    /// Explicitly enables or disables `mark` on `self`, preserving clean `None` inheritance
    /// whenever the block's `base` style already matches the disabled state.
    pub fn set_mark(&mut self, mark: MarkKind, enabled: bool, base: &ResolvedTextStyle) {
        match mark {
            MarkKind::Bold => {
                self.weight = if enabled {
                    Some(FontWeight::BOLD)
                } else if base.weight >= FontWeight::BOLD {
                    Some(FontWeight::NORMAL)
                } else {
                    None
                };
            }
            MarkKind::Italic => {
                self.italic = if enabled {
                    Some(true)
                } else if base.italic {
                    Some(false)
                } else {
                    None
                };
            }
            MarkKind::Underline => {
                self.underline = if enabled {
                    Some(UnderlineKind::Solid)
                } else {
                    None
                };
            }
            MarkKind::Strikethrough => {
                self.strikethrough = if enabled {
                    Some(true)
                } else if base.strikethrough {
                    Some(false)
                } else {
                    None
                };
            }
            MarkKind::InlineCode => {
                self.inline_code = if enabled {
                    Some(true)
                } else if base.inline_code {
                    Some(false)
                } else {
                    None
                };
            }
            MarkKind::Superscript => {
                self.baseline_shift = if enabled {
                    Some(BaselineShift::Superscript)
                } else if base.baseline_shift == BaselineShift::Superscript {
                    Some(BaselineShift::Normal)
                } else {
                    None
                };
            }
            MarkKind::Subscript => {
                self.baseline_shift = if enabled {
                    Some(BaselineShift::Subscript)
                } else if base.baseline_shift == BaselineShift::Subscript {
                    Some(BaselineShift::Normal)
                } else {
                    None
                };
            }
        }
    }
}

/// Fully resolved typographic style ready for layout and shaping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTextStyle {
    pub font_family: SharedStr,
    pub font_size_pt: f32,
    pub weight: FontWeight,
    pub italic: bool,
    pub underline: Option<UnderlineKind>,
    pub strikethrough: bool,
    pub baseline_shift: BaselineShift,
    pub baseline_shift_pt: f32,
    pub color: RgbaColor,
    pub background: Option<RgbaColor>,
    pub inline_code: bool,
    pub link: Option<LinkTarget>,
}

impl ResolvedTextStyle {
    /// Returns true when `mark` is active in this resolved text style.
    pub fn is_mark_active(&self, mark: MarkKind) -> bool {
        match mark {
            MarkKind::Bold => self.weight >= FontWeight::BOLD,
            MarkKind::Italic => self.italic,
            MarkKind::Underline => self.underline.is_some(),
            MarkKind::Strikethrough => self.strikethrough,
            MarkKind::InlineCode => self.inline_code,
            MarkKind::Superscript => self.baseline_shift == BaselineShift::Superscript,
            MarkKind::Subscript => self.baseline_shift == BaselineShift::Subscript,
        }
    }
}

impl Default for ResolvedTextStyle {
    fn default() -> Self {
        Self {
            font_family: Arc::from(".SystemUIFont"),
            font_size_pt: 11.0,
            weight: FontWeight::NORMAL,
            italic: false,
            underline: None,
            strikethrough: false,
            baseline_shift: BaselineShift::Normal,
            baseline_shift_pt: 0.0,
            color: RgbaColor::BLACK,
            background: None,
            inline_code: false,
            link: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LineHeight {
    Multiplier(f32),
    ExactPt(f32),
}

impl Default for LineHeight {
    fn default() -> Self {
        Self::Multiplier(1.35)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabStopAlign {
    Left,
    Center,
    Right,
    Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabStop {
    pub position_pt: f32,
    pub align: TabStopAlign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StyleId(pub u32);

impl StyleId {
    pub const NORMAL: Self = Self(0);
    pub const TITLE: Self = Self(1);
    pub const SUBTITLE: Self = Self(2);
    pub const HEADING_1: Self = Self(11);
    pub const HEADING_2: Self = Self(12);
    pub const HEADING_3: Self = Self(13);
    pub const HEADING_4: Self = Self(14);
    pub const HEADING_5: Self = Self(15);
    pub const HEADING_6: Self = Self(16);
    pub const CODE_BLOCK: Self = Self(20);

    pub fn heading(level: u8) -> Self {
        match level {
            1 => Self::HEADING_1,
            2 => Self::HEADING_2,
            3 => Self::HEADING_3,
            4 => Self::HEADING_4,
            5 => Self::HEADING_5,
            _ => Self::HEADING_6,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedStyle {
    pub id: StyleId,
    pub name: String,
    pub text: InlineStyle,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    pub line_height: LineHeight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleSheet {
    pub styles: HashMap<StyleId, NamedStyle>,
}

impl Default for StyleSheet {
    fn default() -> Self {
        let mut styles = HashMap::new();
        let insert = |map: &mut HashMap<StyleId, NamedStyle>, s: NamedStyle| {
            map.insert(s.id, s);
        };

        insert(
            &mut styles,
            NamedStyle {
                id: StyleId::NORMAL,
                name: "Normal".into(),
                text: InlineStyle::default().with_font_size(11.0),
                space_before_pt: 0.0,
                space_after_pt: 8.0,
                line_height: LineHeight::Multiplier(1.38),
            },
        );
        insert(
            &mut styles,
            NamedStyle {
                id: StyleId::TITLE,
                name: "Title".into(),
                text: InlineStyle::default()
                    .with_font_size(26.0)
                    .with_weight(FontWeight::BOLD),
                space_before_pt: 0.0,
                space_after_pt: 4.0,
                line_height: LineHeight::Multiplier(1.2),
            },
        );
        insert(
            &mut styles,
            NamedStyle {
                id: StyleId::SUBTITLE,
                name: "Subtitle".into(),
                text: InlineStyle::default()
                    .with_font_size(14.0)
                    .with_color(RgbaColor::MUTED),
                space_before_pt: 0.0,
                space_after_pt: 14.0,
                line_height: LineHeight::Multiplier(1.3),
            },
        );
        for (level, size, before, after) in [
            (1u8, 20.0f32, 18.0f32, 6.0f32),
            (2, 16.0, 16.0, 5.0),
            (3, 13.5, 14.0, 4.0),
            (4, 12.0, 12.0, 4.0),
            (5, 11.0, 10.0, 4.0),
            (6, 10.5, 10.0, 4.0),
        ] {
            insert(
                &mut styles,
                NamedStyle {
                    id: StyleId::heading(level),
                    name: format!("Heading {level}"),
                    text: InlineStyle::default()
                        .with_font_size(size)
                        .with_weight(FontWeight::SEMIBOLD),
                    space_before_pt: before,
                    space_after_pt: after,
                    line_height: LineHeight::Multiplier(1.25),
                },
            );
        }
        insert(
            &mut styles,
            NamedStyle {
                id: StyleId::CODE_BLOCK,
                name: "Code Block".into(),
                text: InlineStyle::default()
                    .with_font_family("monospace")
                    .with_font_size(10.0),
                space_before_pt: 8.0,
                space_after_pt: 10.0,
                line_height: LineHeight::Multiplier(1.45),
            },
        );

        Self { styles }
    }
}

impl StyleSheet {
    pub fn get(&self, id: StyleId) -> Option<&NamedStyle> {
        self.styles.get(&id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockStyle {
    pub named_style: Option<StyleId>,
    pub align: TextAlign,
    pub direction: TextDirection,
    pub indent_start_pt: f32,
    pub indent_end_pt: f32,
    pub first_line_indent_pt: f32,
    pub line_height: Option<LineHeight>,
    pub space_before_pt: Option<f32>,
    pub space_after_pt: Option<f32>,
    pub tab_stops: Vec<TabStop>,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            named_style: Some(StyleId::NORMAL),
            align: TextAlign::Left,
            direction: TextDirection::Auto,
            indent_start_pt: 0.0,
            indent_end_pt: 0.0,
            first_line_indent_pt: 0.0,
            line_height: None,
            space_before_pt: None,
            space_after_pt: None,
            tab_stops: Vec::new(),
        }
    }
}

impl BlockStyle {
    pub fn named(id: StyleId) -> Self {
        Self {
            named_style: Some(id),
            ..Self::default()
        }
    }

    pub fn with_align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    pub fn with_indent(mut self, start_pt: f32, first_line_pt: f32) -> Self {
        self.indent_start_pt = start_pt;
        self.first_line_indent_pt = first_line_pt;
        self
    }

    pub fn resolved_line_height(&self, sheet: &StyleSheet) -> LineHeight {
        self.line_height.unwrap_or_else(|| {
            self.named_style
                .and_then(|id| sheet.get(id))
                .map(|s| s.line_height)
                .unwrap_or_default()
        })
    }

    pub fn resolved_space_before(&self, sheet: &StyleSheet) -> f32 {
        self.space_before_pt.unwrap_or_else(|| {
            self.named_style
                .and_then(|id| sheet.get(id))
                .map(|s| s.space_before_pt)
                .unwrap_or(0.0)
        })
    }

    pub fn resolved_space_after(&self, sheet: &StyleSheet) -> f32 {
        self.space_after_pt.unwrap_or_else(|| {
            self.named_style
                .and_then(|id| sheet.get(id))
                .map(|s| s.space_after_pt)
                .unwrap_or(8.0)
        })
    }

    pub fn resolved_base_text(
        &self,
        sheet: &StyleSheet,
        doc_default: &ResolvedTextStyle,
    ) -> ResolvedTextStyle {
        if let Some(named) = self.named_style.and_then(|id| sheet.get(id)) {
            named.text.resolve(doc_default)
        } else {
            doc_default.clone()
        }
    }
}

/// Physical geometry of a continuous, infinite-length paper sheet in typographic points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperSpec {
    pub width_pt: f32,
    pub margins_pt: Edges,
    pub desk_margin_pt: f32,
}

impl PaperSpec {
    /// US Letter width (8.5 in = 612pt) with standard 1 in (72pt) margins.
    pub const LETTER: Self = Self {
        width_pt: 612.0,
        margins_pt: Edges::MARGINS_NORMAL,
        desk_margin_pt: 24.0,
    };

    /// Compact reading sheet (520pt) with 0.5 in (36pt) margins.
    pub const NARROW: Self = Self {
        width_pt: 520.0,
        margins_pt: Edges::MARGINS_NARROW,
        desk_margin_pt: 24.0,
    };

    /// Wide sheet (760pt) with 0.75 in (54pt) margins.
    pub const WIDE: Self = Self {
        width_pt: 760.0,
        margins_pt: Edges::MARGINS_MODERATE,
        desk_margin_pt: 24.0,
    };

    /// Returns a copy of this paper specification with updated page `margins_pt`.
    pub const fn with_margins(mut self, margins_pt: Edges) -> Self {
        self.margins_pt = margins_pt;
        self
    }
}

/// Separates fluid viewport-filling layout from fixed-width infinite-length paper layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum HorizontalLayout {
    /// Edge-to-edge fluid canvas that reflows dynamically to the viewport width.
    Fill,
    /// Continuous infinite-length paper sheet with fixed typographic width.
    /// Centers inside wider viewports and scrolls horizontally when narrower or zoomed in.
    Paper(PaperSpec),
}

impl Default for HorizontalLayout {
    fn default() -> Self {
        Self::Paper(PaperSpec::LETTER)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlockWidth {
    #[default]
    Column,
    Wide,
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentLayoutConfig {
    pub horizontal_layout: HorizontalLayout,
    /// Document margins in typographic points used when [`HorizontalLayout::Fill`] is active.
    pub fill_margins_pt: Edges,
    pub default_tab_interval_pt: f32,
    pub default_style: ResolvedTextStyle,
}

impl Default for DocumentLayoutConfig {
    fn default() -> Self {
        Self {
            horizontal_layout: HorizontalLayout::default(),
            fill_margins_pt: Edges::xy(36.0, 28.0),
            default_tab_interval_pt: 36.0,
            default_style: ResolvedTextStyle::default(),
        }
    }
}

impl DocumentLayoutConfig {
    /// Returns the active document margins in typographic points for the current layout mode.
    pub fn active_margins_pt(&self) -> Edges {
        match self.horizontal_layout {
            HorizontalLayout::Fill => self.fill_margins_pt,
            HorizontalLayout::Paper(spec) => spec.margins_pt,
        }
    }

    /// Updates the active document margins in typographic points for the current layout mode.
    pub fn set_active_margins_pt(&mut self, margins_pt: Edges) {
        let clamped = margins_pt.clamped_non_negative();
        match &mut self.horizontal_layout {
            HorizontalLayout::Fill => self.fill_margins_pt = clamped,
            HorizontalLayout::Paper(spec) => spec.margins_pt = clamped,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    /// Narrow document margins (0.5 in = 36pt on all sides).
    pub const MARGINS_NARROW: Self = Self::all(36.0);
    /// Moderate document margins (0.75 in = 54pt on all sides).
    pub const MARGINS_MODERATE: Self = Self::all(54.0);
    /// Standard Google Docs Normal margins (1.0 in = 72pt on all sides).
    pub const MARGINS_NORMAL: Self = Self::all(72.0);
    /// Wide document margins (1.5 in = 108pt left/right, 1.0 in = 72pt top/bottom).
    pub const MARGINS_WIDE: Self = Self::xy(108.0, 72.0);

    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn xy(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Clamps every edge to a finite value in `0.0..=288.0` pt (0 to 4 inches).
    pub fn clamped_non_negative(self) -> Self {
        let clamp = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, 288.0)
            } else {
                0.0
            }
        };
        Self {
            top: clamp(self.top),
            right: clamp(self.right),
            bottom: clamp(self.bottom),
            left: clamp(self.left),
        }
    }
}
