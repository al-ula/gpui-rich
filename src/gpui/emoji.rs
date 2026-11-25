//! Built-in UTF-8 emoji extension providing:
//!
//! * **`/emoji` insert menu**: registers a slash completion item dispatching the typed
//!   [`OpenEmojiPicker`] command, which opens a Layer A caret-anchored emoji grid overlay with
//!   category tabs and full keyboard (`↑↓←→`, `Tab`/`Enter`, `Esc`) and pointer selection.
//! * **`:emojiname` completion menu**: registers a prefix completion provider matching all
//!   official Unicode emoji shortcodes, display names, and aliases with MRU recency ranking.
//!
//! All emoji insertions route strictly through [`RichCommand::InsertText`] via
//! [`RichDocumentView::dispatch_command`]. On Linux systems where `Noto Color Emoji` is shipped
//! as a COLRv1 font (unsupported by GPUI's `swash` rasterizer), [`colrv1`] rasterizes shaped
//! color emoji glyphs on demand via `skrifa` + `tiny-skia`.

mod colrv1;

use crate::gpui::completion::{
    CompletionAccept, CompletionAcceptContext, CompletionBlockScope, CompletionDetail,
    CompletionItem, CompletionProvider, CompletionQueryContext, CompletionQueryResult,
    CompletionTrigger, fuzzy_match_label,
};
use crate::gpui::extension::{ExtensionPlugin, ExtensionRegistrationError, ReadCommandOutcome};
use crate::gpui::overlay::{
    OverlayAnchor, OverlayDismissPolicy, OverlayId, OverlayKeyAction, OverlayKeyResult,
    OverlayPlacement, OverlaySpec, OverlayZLayer,
};
use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::model::{ExtensionCommand, ReadOnly, RichCommand, SharedStr};
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Corners, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, RenderImage, SharedString, StatefulInteractiveElement,
    Styled, TextRun, WeakEntity, Window, canvas, div, point, px,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

/// Layer A overlay identifier for the caret-anchored `/emoji` picker grid.
pub const EMOJI_PICKER_OVERLAY_ID: &str = "gpui_rich.emoji.picker";

const PICKER_COLUMNS: usize = 8;
const PICKER_VISIBLE_ROWS: usize = 5;
const MAX_RECENT_EMOJIS: usize = 16;

/// Semantic grouping aligned with Unicode CLDR emoji groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmojiCategory {
    Smileys,
    People,
    Nature,
    Food,
    Travel,
    Activities,
    Objects,
    Symbols,
    Flags,
}

impl EmojiCategory {
    /// Returns all emoji categories in Unicode CLDR order.
    pub const ALL: [Self; 9] = [
        Self::Smileys,
        Self::People,
        Self::Nature,
        Self::Food,
        Self::Travel,
        Self::Activities,
        Self::Objects,
        Self::Symbols,
        Self::Flags,
    ];

    /// Short tab label used in the `/emoji` picker and completion badges.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Smileys => "Smileys",
            Self::People => "People",
            Self::Nature => "Nature",
            Self::Food => "Food",
            Self::Travel => "Travel",
            Self::Activities => "Activity",
            Self::Objects => "Objects",
            Self::Symbols => "Symbols",
            Self::Flags => "Flags",
        }
    }
}

/// Single UTF-8 emoji entry searchable by shortcode, name, and aliases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiEntry {
    pub glyph: SharedStr,
    pub shortcode: SharedStr,
    pub name: SharedStr,
    pub aliases: Vec<SharedStr>,
    pub category: EmojiCategory,
}

impl EmojiEntry {
    /// Creates an emoji catalog entry.
    pub fn new(
        glyph: impl Into<SharedStr>,
        shortcode: impl Into<SharedStr>,
        name: impl Into<SharedStr>,
        category: EmojiCategory,
    ) -> Self {
        Self {
            glyph: glyph.into(),
            shortcode: shortcode.into(),
            name: name.into(),
            aliases: Vec::new(),
            category,
        }
    }

    /// Attaches additional searchable aliases (for example `"+1"` for `"thumbsup"`).
    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<SharedStr>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }
}

/// Typed read-only [`ExtensionCommand`] that opens the caret-anchored `/emoji` insert menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenEmojiPicker;

impl ExtensionCommand for OpenEmojiPicker {
    const ID: &'static str = "gpui_rich.emoji.open_picker";
    type Effect = ReadOnly;
}

/// Configuration for [`EmojiExtension`].
#[derive(Clone, Debug)]
pub struct EmojiConfig {
    /// Registers the `/emoji` slash completion item opening the caret emoji picker.
    pub register_slash_completion: bool,
    /// Registers the `:emojiname` prefix completion provider.
    pub register_colon_completion: bool,
    /// Additional custom or extended emoji entries appended to the catalog.
    pub extra_entries: Vec<EmojiEntry>,
    /// When `true`, uses only [`Self::extra_entries`] instead of merging with the built-in catalog.
    pub replace_builtin_catalog: bool,
}

impl Default for EmojiConfig {
    fn default() -> Self {
        Self {
            register_slash_completion: true,
            register_colon_completion: true,
            extra_entries: Vec::new(),
            replace_builtin_catalog: false,
        }
    }
}

impl EmojiConfig {
    /// Creates a default [`EmojiConfig`] enabling both `/emoji` and `:emojiname`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables the `/emoji` slash completion item.
    pub const fn with_slash_completion(mut self, enabled: bool) -> Self {
        self.register_slash_completion = enabled;
        self
    }

    /// Enables or disables the `:emojiname` completion provider.
    pub const fn with_colon_completion(mut self, enabled: bool) -> Self {
        self.register_colon_completion = enabled;
        self
    }

    /// Appends extra emoji entries to the catalog.
    pub fn with_extra_entries(mut self, entries: Vec<EmojiEntry>) -> Self {
        self.extra_entries = entries;
        self
    }

    /// Replaces the built-in catalog entirely with `entries`.
    pub fn with_custom_catalog(mut self, entries: Vec<EmojiEntry>) -> Self {
        self.extra_entries = entries;
        self.replace_builtin_catalog = true;
        self
    }
}

/// Built-in UTF-8 emoji [`ExtensionPlugin`] registering [`OpenEmojiPicker`], `/emoji` slash
/// completion, and `:emojiname` shortcode completion onto [`RichDocumentView`].
#[derive(Clone, Debug, Default)]
pub struct EmojiExtension {
    config: EmojiConfig,
}

impl EmojiExtension {
    /// Creates an [`EmojiExtension`] with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies custom [`EmojiConfig`] options.
    pub fn with_config(mut self, config: EmojiConfig) -> Self {
        self.config = config;
        self
    }
}

impl ExtensionPlugin for EmojiExtension {
    fn install(
        self,
        view: &mut RichDocumentView,
        cx: &mut Context<RichDocumentView>,
    ) -> Result<(), ExtensionRegistrationError> {
        let mut catalog = if self.config.replace_builtin_catalog {
            Vec::new()
        } else {
            default_emoji_catalog()
        };
        catalog.extend(self.config.extra_entries);

        let shared = Rc::new(RefCell::new(EmojiRuntimeState {
            catalog,
            recent_shortcodes: Vec::new(),
            picker: EmojiPickerState::default(),
        }));

        view.register_read_command::<OpenEmojiPicker>(|_command, _context| {
            ReadCommandOutcome::Executed
        })?;

        // Observe `OpenEmojiPicker` execution to mount the Layer A overlay on `view`.
        let open_state = Rc::clone(&shared);
        cx.subscribe_self(move |this, event, cx| {
            if let crate::gpui::view::RichDocumentEvent::CommandExecuted {
                command_id,
                outcome,
                ..
            } = event
                && *command_id == OpenEmojiPicker::ID
                && *outcome == crate::model::CommandOutcome::Executed
                && this.policy().editing.enabled
            {
                open_state.borrow_mut().picker.reset();
                let spec = build_emoji_picker_overlay_spec(Rc::clone(&open_state));
                this.set_overlay(spec, cx);
            }
        })
        .detach();

        if self.config.register_slash_completion {
            view.register_completion_provider(EmojiSlashCompletionProvider::new())?;
        }
        if self.config.register_colon_completion {
            view.register_completion_provider(EmojiColonCompletionProvider::new(Rc::clone(
                &shared,
            )))?;
        }

        Ok(())
    }
}

impl RichDocumentView {
    /// Installs the built-in UTF-8 emoji extension (`/emoji` insert menu and `:emojiname` completion).
    pub fn install_emoji(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(EmojiExtension::new(), cx)
    }

    /// Installs a configured [`EmojiExtension`] onto this view.
    pub fn install_emoji_with(
        &mut self,
        extension: EmojiExtension,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(extension, cx)
    }
}

/// Rasterizes a COLRv1 emoji `glyph_id` on Linux when the system emoji font is COLRv1.
pub(crate) fn rasterize_colrv1_glyph_id(glyph_id: u32, size_px: u32) -> Option<Arc<RenderImage>> {
    colrv1::rasterize_glyph_id(glyph_id, size_px)
}

/// Renders an emoji string inside GPUI overlay UI (picker tiles and completion rows), using the
/// Linux COLRv1 rasterizer when active and falling back to native text shaping everywhere else.
pub(crate) fn render_emoji_element(emoji: &str, size_px: f32, window: &Window) -> AnyElement {
    if colrv1::is_available() {
        let target_px = (size_px * window.scale_factor()).round().max(12.0) as u32;
        let glyph_id = colrv1::lookup_single_codepoint_glyph_id(emoji).or_else(|| {
            let run = TextRun {
                len: emoji.len(),
                font: gpui_kit::font(".SystemUIFont"),
                color: gpui_kit::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(emoji.to_owned()),
                px(size_px),
                &[run],
                None,
            );
            shaped
                .runs
                .iter()
                .flat_map(|shaped_run| shaped_run.glyphs.iter())
                .find(|glyph| glyph.is_emoji)
                .map(|glyph| glyph.id.0)
        });
        if let Some(gid) = glyph_id
            && let Some(image) = colrv1::rasterize_glyph_id(gid, target_px)
        {
            return canvas(
                |_bounds, _window, _cx| (),
                move |bounds, (), window, _cx| {
                    if let Err(error) = window.paint_image(
                        bounds,
                        bounds,
                        Corners::all(px(0.0)),
                        Arc::clone(&image),
                        0,
                        false,
                    ) {
                        eprintln!("rich-gpui: failed to paint emoji image: {error}");
                    }
                },
            )
            .w(px(size_px))
            .h(px(size_px))
            .flex_shrink_0()
            .into_any_element();
        }
    }

    div()
        .text_size(px(size_px))
        .line_height(px(size_px * 1.15))
        .child(emoji.to_owned())
        .into_any_element()
}

struct EmojiRuntimeState {
    catalog: Vec<EmojiEntry>,
    recent_shortcodes: Vec<SharedStr>,
    picker: EmojiPickerState,
}

impl EmojiRuntimeState {
    fn record_recent(&mut self, shortcode: &str) {
        self.recent_shortcodes
            .retain(|existing| existing.as_ref() != shortcode);
        self.recent_shortcodes
            .insert(0, SharedStr::from(shortcode.to_owned()));
        self.recent_shortcodes.truncate(MAX_RECENT_EMOJIS);
    }

    fn recent_boost(&self, shortcode: &str) -> i32 {
        self.recent_shortcodes
            .iter()
            .position(|entry| entry.as_ref() == shortcode)
            .map_or(0, |index| (MAX_RECENT_EMOJIS - index) as i32 * 4)
    }

    fn filtered_picker_indices(&self) -> Vec<usize> {
        match self.picker.category {
            None => (0..self.catalog.len()).collect(),
            Some(category) => self
                .catalog
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| (entry.category == category).then_some(index))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct EmojiPickerState {
    category: Option<EmojiCategory>,
    selected_index: usize,
    scroll_row: usize,
}

impl EmojiPickerState {
    fn reset(&mut self) {
        self.category = None;
        self.selected_index = 0;
        self.scroll_row = 0;
    }

    fn set_category(&mut self, category: Option<EmojiCategory>) {
        if self.category != category {
            self.category = category;
            self.selected_index = 0;
            self.scroll_row = 0;
        }
    }

    fn move_grid(&mut self, delta: isize, total_items: usize) {
        if total_items == 0 {
            self.selected_index = 0;
            self.scroll_row = 0;
            return;
        }
        let next = if delta < 0 {
            let step = (-delta) as usize;
            if self.selected_index >= step {
                self.selected_index - step
            } else {
                total_items - 1
            }
        } else {
            let step = delta as usize;
            if self.selected_index + step < total_items {
                self.selected_index + step
            } else {
                0
            }
        };
        self.selected_index = next;
        self.ensure_selected_row_visible(total_items);
    }

    fn ensure_selected_row_visible(&mut self, total_items: usize) {
        let total_rows = total_items.div_ceil(PICKER_COLUMNS);
        if total_rows <= PICKER_VISIBLE_ROWS {
            self.scroll_row = 0;
            return;
        }
        let selected_row = self.selected_index / PICKER_COLUMNS;
        if selected_row < self.scroll_row {
            self.scroll_row = selected_row;
        } else if selected_row >= self.scroll_row + PICKER_VISIBLE_ROWS {
            self.scroll_row = selected_row + 1 - PICKER_VISIBLE_ROWS;
        }
        let max_scroll = total_rows.saturating_sub(PICKER_VISIBLE_ROWS);
        self.scroll_row = self.scroll_row.min(max_scroll);
    }
}

fn build_emoji_picker_overlay_spec(state: Rc<RefCell<EmojiRuntimeState>>) -> OverlaySpec {
    let render_state = Rc::clone(&state);
    let key_state = Rc::clone(&state);

    OverlaySpec::new(
        OverlayId::new(EMOJI_PICKER_OVERLAY_ID),
        OverlayAnchor::Caret,
        move |context, window, _cx| {
            build_emoji_picker_element(&render_state, context.theme, context.view.clone(), window)
        },
    )
    .with_placement(OverlayPlacement::below_start())
    .with_z_layer(OverlayZLayer::Menu)
    .with_dismiss_policy(OverlayDismissPolicy::popover())
    .with_key_handler(move |action, view, _window, cx| {
        let filtered = key_state.borrow().filtered_picker_indices();
        let total = filtered.len();
        match action {
            OverlayKeyAction::Left | OverlayKeyAction::ShiftTab => {
                key_state.borrow_mut().picker.move_grid(-1, total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Right => {
                key_state.borrow_mut().picker.move_grid(1, total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Up => {
                key_state
                    .borrow_mut()
                    .picker
                    .move_grid(-(PICKER_COLUMNS as isize), total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Down => {
                key_state
                    .borrow_mut()
                    .picker
                    .move_grid(PICKER_COLUMNS as isize, total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::PageUp => {
                key_state
                    .borrow_mut()
                    .picker
                    .move_grid(-((PICKER_COLUMNS * PICKER_VISIBLE_ROWS) as isize), total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::PageDown => {
                key_state
                    .borrow_mut()
                    .picker
                    .move_grid((PICKER_COLUMNS * PICKER_VISIBLE_ROWS) as isize, total);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Escape => {
                view.remove_overlay(&OverlayId::new(EMOJI_PICKER_OVERLAY_ID), cx);
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Tab | OverlayKeyAction::Enter => {
                let selected_entry = {
                    let runtime = key_state.borrow();
                    let clamped_idx = runtime.picker.selected_index.min(total.saturating_sub(1));
                    filtered
                        .get(clamped_idx)
                        .and_then(|&catalog_idx| runtime.catalog.get(catalog_idx))
                        .cloned()
                };
                view.remove_overlay(&OverlayId::new(EMOJI_PICKER_OVERLAY_ID), cx);
                if let Some(entry) = selected_entry {
                    insert_emoji_via_command(&key_state, &entry, view, cx);
                }
                OverlayKeyResult::Handled
            }
        }
    })
}

fn insert_emoji_via_command(
    state: &Rc<RefCell<EmojiRuntimeState>>,
    entry: &EmojiEntry,
    view: &mut RichDocumentView,
    cx: &mut Context<RichDocumentView>,
) {
    let outcome = view.dispatch_command(RichCommand::InsertText(entry.glyph.to_string()), cx);
    if outcome == crate::model::CommandOutcome::Executed {
        state.borrow_mut().record_recent(entry.shortcode.as_ref());
    }
}

fn build_emoji_picker_element(
    state: &Rc<RefCell<EmojiRuntimeState>>,
    theme: &RichTheme,
    view: Entity<RichDocumentView>,
    window: &Window,
) -> AnyElement {
    let (active_category, selected_index, scroll_row, filtered_entries) = {
        let runtime = state.borrow();
        let indices = runtime.filtered_picker_indices();
        let entries: Vec<EmojiEntry> = indices
            .into_iter()
            .filter_map(|index| runtime.catalog.get(index).cloned())
            .collect();
        let clamped_selected = runtime
            .picker
            .selected_index
            .min(entries.len().saturating_sub(1));
        (
            runtime.picker.category,
            clamped_selected,
            runtime.picker.scroll_row,
            entries,
        )
    };

    let shadow = vec![BoxShadow {
        color: theme.paper_shadow,
        offset: point(px(0.0), px(6.0)),
        blur_radius: px(16.0),
        spread_radius: px(0.0),
        inset: false,
    }];

    // Category tabs
    let mut tabs = div().flex().flex_wrap().items_center().gap(px(3.0));
    let tab_options: [(Option<EmojiCategory>, &'static str); 10] = [
        (None, "All"),
        (Some(EmojiCategory::Smileys), "Smileys"),
        (Some(EmojiCategory::People), "People"),
        (Some(EmojiCategory::Nature), "Nature"),
        (Some(EmojiCategory::Food), "Food"),
        (Some(EmojiCategory::Travel), "Travel"),
        (Some(EmojiCategory::Activities), "Activity"),
        (Some(EmojiCategory::Objects), "Objects"),
        (Some(EmojiCategory::Symbols), "Symbols"),
        (Some(EmojiCategory::Flags), "Flags"),
    ];
    for (tab_index, (category, label)) in tab_options.into_iter().enumerate() {
        let is_active = active_category == category;
        let state_for_tab = Rc::clone(state);
        let view_for_tab = view.clone();
        tabs = tabs.child(
            div()
                .id(("rich-emoji-tab", tab_index))
                .px(px(6.0))
                .py(px(2.5))
                .rounded(px(5.0))
                .cursor_pointer()
                .bg(if is_active {
                    theme.selection_background
                } else {
                    gpui_kit::transparent_black()
                })
                .text_size(px(10.5))
                .font_weight(if is_active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if is_active {
                    theme.accent
                } else {
                    theme.muted_text
                })
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    cx.stop_propagation();
                    state_for_tab.borrow_mut().picker.set_category(category);
                    view_for_tab.update(cx, |_, cx| cx.notify());
                })
                .child(label),
        );
    }

    // 8-column emoji grid window
    let start_index = scroll_row * PICKER_COLUMNS;
    let end_index =
        (start_index + PICKER_COLUMNS * PICKER_VISIBLE_ROWS).min(filtered_entries.len());
    let mut grid = div().flex().flex_col().gap(px(2.0));
    for row_start in (start_index..end_index).step_by(PICKER_COLUMNS) {
        let row_end = (row_start + PICKER_COLUMNS).min(end_index);
        let mut row = div().flex().items_center().gap(px(2.0));
        for (absolute_index, entry) in filtered_entries
            .iter()
            .enumerate()
            .take(row_end)
            .skip(row_start)
        {
            let is_selected = absolute_index == selected_index;
            let state_for_hover = Rc::clone(state);
            let view_for_hover = view.clone();
            let state_for_click = Rc::clone(state);
            let view_for_click = view.clone();
            let entry_for_click = entry.clone();

            row = row.child(
                div()
                    .id(("rich-emoji-cell", absolute_index))
                    .w(px(34.0))
                    .h(px(32.0))
                    .rounded(px(6.0))
                    .bg(if is_selected {
                        theme.selection_background
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_hover(move |hovered, _window, cx| {
                        if *hovered {
                            let changed = {
                                let mut runtime = state_for_hover.borrow_mut();
                                if runtime.picker.selected_index != absolute_index {
                                    runtime.picker.selected_index = absolute_index;
                                    true
                                } else {
                                    false
                                }
                            };
                            if changed {
                                view_for_hover.update(cx, |_, cx| cx.notify());
                            }
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                        cx.stop_propagation();
                        let entry_copy = entry_for_click.clone();
                        let state_copy = Rc::clone(&state_for_click);
                        view_for_click.update(cx, |this, cx| {
                            this.remove_overlay(&OverlayId::new(EMOJI_PICKER_OVERLAY_ID), cx);
                            insert_emoji_via_command(&state_copy, &entry_copy, this, cx);
                        });
                    })
                    .child(render_emoji_element(entry.glyph.as_ref(), 20.0, window)),
            );
        }
        grid = grid.child(row);
    }

    let footer_content: AnyElement = if let Some(entry) = filtered_entries.get(selected_index) {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(render_emoji_element(entry.glyph.as_ref(), 16.0, window))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_text)
                    .child(format!(":{}:  {}", entry.shortcode, entry.name)),
            )
            .into_any_element()
    } else {
        div()
            .text_size(px(11.0))
            .text_color(theme.muted_text)
            .child("No emojis")
            .into_any_element()
    };

    div()
        .w(px(306.0))
        .p(px(8.0))
        .rounded(px(10.0))
        .bg(theme.surface_background)
        .border_1()
        .border_color(theme.paper_border)
        .shadow(shadow)
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(2.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(format!("Insert Emoji ({})", filtered_entries.len()))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.muted_text)
                        .child("↑↓←→ · Tab/Enter · Esc"),
                ),
        )
        .child(tabs)
        .child(
            div()
                .pt(px(2.0))
                .border_t_1()
                .border_color(theme.code_border)
                .child(grid),
        )
        .child(
            div()
                .pt(px(4.0))
                .px(px(2.0))
                .border_t_1()
                .border_color(theme.code_border)
                .child(footer_content),
        )
        .into_any_element()
}

struct EmojiSlashCompletionProvider {
    triggers: [CompletionTrigger; 1],
}

impl EmojiSlashCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::prefix("/")],
        }
    }
}

impl CompletionProvider for EmojiSlashCompletionProvider {
    fn id(&self) -> &'static str {
        "gpui_rich.emoji.slash"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::ProseOnly
    }

    fn header_title(&self) -> Option<&str> {
        Some("Slash Commands")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        _view: WeakEntity<RichDocumentView>,
        _cx: &mut App,
    ) -> CompletionQueryResult {
        let label = "Emoji Picker";
        let matched = fuzzy_match_label(context.query, label).or_else(|| {
            fuzzy_match_label(context.query, "smiley").map(|(score, _)| (score - 8, Vec::new()))
        });
        let Some((score, indices)) = matched else {
            return CompletionQueryResult::Ready(Vec::new());
        };
        let item = CompletionItem::new(
            "emoji-picker",
            label,
            CompletionAccept::command(RichCommand::extension(OpenEmojiPicker)),
        )
        .with_icon("😀")
        .with_subtitle("Browse and insert any Unicode UTF-8 emoji")
        .with_badge("emoji")
        .with_category("Insert")
        .with_match(score, indices)
        .with_detail(
            CompletionDetail::new(
                "Emoji Picker",
                "Opens a categorized Unicode emoji grid at the caret. You can also type :emojiname directly.",
            )
            .with_preview("1,800+ Unicode emojis · ↑↓←→ navigate · Tab insert"),
        );
        CompletionQueryResult::Ready(vec![item])
    }
}

struct EmojiColonCompletionProvider {
    triggers: [CompletionTrigger; 1],
    state: Rc<RefCell<EmojiRuntimeState>>,
}

impl EmojiColonCompletionProvider {
    fn new(state: Rc<RefCell<EmojiRuntimeState>>) -> Self {
        Self {
            triggers: [CompletionTrigger::prefix(":")],
            state,
        }
    }
}

/// Scores an [`EmojiEntry`] against a `:emojiname` query across shortcode, aliases, and display name.
pub(crate) fn match_emoji_entry(
    query: &str,
    entry: &EmojiEntry,
    recent_boost: i32,
) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((recent_boost, Vec::new()));
    }

    let mut best: Option<(i32, Vec<usize>)> = None;

    if let Some((score, indices)) = fuzzy_match_label(query, entry.shortcode.as_ref()) {
        // Shift by 1 for the leading `:` in `:<shortcode>:`.
        let shifted = indices.into_iter().map(|idx| idx + 1).collect();
        best = Some((score + 20 + recent_boost, shifted));
    }

    for alias in &entry.aliases {
        if let Some((score, _)) = fuzzy_match_label(query, alias.as_ref()) {
            let candidate = score + 14 + recent_boost;
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| candidate > *best_score)
            {
                best = Some((candidate, Vec::new()));
            }
        }
    }

    if let Some((score, _)) = fuzzy_match_label(query, entry.name.as_ref()) {
        let candidate = score + recent_boost;
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| candidate > *best_score)
        {
            best = Some((candidate, Vec::new()));
        }
    }

    best
}

impl CompletionProvider for EmojiColonCompletionProvider {
    fn id(&self) -> &'static str {
        "gpui_rich.emoji.colon"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::ProseOnly
    }

    fn header_title(&self) -> Option<&str> {
        Some("Emoji")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        _view: WeakEntity<RichDocumentView>,
        _cx: &mut App,
    ) -> CompletionQueryResult {
        let runtime = self.state.borrow();
        let mut items = Vec::new();
        for (index, entry) in runtime.catalog.iter().enumerate() {
            let recent_boost = runtime.recent_boost(entry.shortcode.as_ref());
            if let Some((mut score, match_indices)) =
                match_emoji_entry(context.query, entry, recent_boost)
            {
                if context.query.is_empty() {
                    score += (runtime.catalog.len().saturating_sub(index)) as i32;
                }
                let label = format!(":{}:", entry.shortcode);
                items.push(
                    CompletionItem::new(
                        entry.shortcode.clone(),
                        label,
                        CompletionAccept::replace_text(entry.glyph.to_string()),
                    )
                    .with_icon(entry.glyph.clone())
                    .with_subtitle(entry.name.to_string())
                    .with_badge(entry.category.label())
                    .with_match(score, match_indices),
                );
            }
        }
        items.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.label.cmp(&right.label))
        });
        items.truncate(24);
        CompletionQueryResult::Ready(items)
    }

    fn on_accept(&self, context: &CompletionAcceptContext<'_>) {
        self.state
            .borrow_mut()
            .record_recent(context.item.id.as_ref());
    }
}

fn map_unicode_group(group: emojis::Group) -> EmojiCategory {
    match group {
        emojis::Group::SmileysAndEmotion => EmojiCategory::Smileys,
        emojis::Group::PeopleAndBody => EmojiCategory::People,
        emojis::Group::AnimalsAndNature => EmojiCategory::Nature,
        emojis::Group::FoodAndDrink => EmojiCategory::Food,
        emojis::Group::TravelAndPlaces => EmojiCategory::Travel,
        emojis::Group::Activities => EmojiCategory::Activities,
        emojis::Group::Objects => EmojiCategory::Objects,
        emojis::Group::Symbols => EmojiCategory::Symbols,
        emojis::Group::Flags => EmojiCategory::Flags,
    }
}

fn slugify_name(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_was_sep = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep && !slug.is_empty() {
            slug.push('_');
            last_was_sep = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    slug
}

static DEFAULT_CATALOG_CACHE: OnceLock<Vec<EmojiEntry>> = OnceLock::new();

/// Returns the full official Unicode UTF-8 emoji catalog (~1,850+ emojis) with GitHub shortcodes,
/// CLDR display names, aliases, and categories.
pub fn default_emoji_catalog() -> Vec<EmojiEntry> {
    DEFAULT_CATALOG_CACHE
        .get_or_init(|| {
            emojis::iter()
                .map(|emoji| {
                    let mut shortcodes = emoji.shortcodes();
                    let primary_shortcode = shortcodes
                        .next()
                        .map(str::to_owned)
                        .unwrap_or_else(|| slugify_name(emoji.name()));
                    let aliases: Vec<SharedStr> = shortcodes.map(SharedStr::from).collect();
                    EmojiEntry::new(
                        emoji.as_str(),
                        primary_shortcode,
                        emoji.name(),
                        map_unicode_group(emoji.group()),
                    )
                    .with_aliases(aliases)
                })
                .collect()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpui::completion::{PrefixBoundary, match_prefix_trigger};

    #[test]
    fn colon_trigger_matches_emojiname_and_ignores_prose_colons() {
        assert!(
            match_prefix_trigger(
                "Note: see below",
                5,
                ":",
                PrefixBoundary::LineStartOrWhitespace,
                false
            )
            .is_none()
        );
        assert!(
            match_prefix_trigger(
                "Meeting at 12:30",
                16,
                ":",
                PrefixBoundary::LineStartOrWhitespace,
                false
            )
            .is_none()
        );
        let hit = match_prefix_trigger(
            "Ship it :roc",
            12,
            ":",
            PrefixBoundary::LineStartOrWhitespace,
            false,
        );
        assert!(hit.is_some());
        if let Some(detected) = hit {
            assert_eq!(detected.trigger_start, 8);
            assert_eq!(detected.query_start, 9);
            assert_eq!(detected.caret_offset, 12);
        }
    }

    #[test]
    fn default_catalog_covers_full_unicode_emoji_dataset() {
        let catalog = default_emoji_catalog();
        assert!(catalog.len() > 1500);

        let thumbs = catalog.iter().find(|entry| {
            entry.shortcode.as_ref() == "+1"
                || entry
                    .aliases
                    .iter()
                    .any(|alias| alias.as_ref() == "thumbsup")
        });
        assert!(thumbs.is_some_and(|entry| match_emoji_entry("thumbsup", entry, 0).is_some()));

        let rocket = catalog
            .iter()
            .find(|entry| entry.shortcode.as_ref() == "rocket");
        let base = rocket.and_then(|entry| match_emoji_entry("roc", entry, 0).map(|(s, _)| s));
        let boosted = rocket.and_then(|entry| match_emoji_entry("roc", entry, 20).map(|(s, _)| s));
        assert!(base.is_some());
        assert!(boosted > base);
    }

    #[test]
    fn emoji_picker_grid_navigation_scrolls_and_wraps() {
        let mut picker = EmojiPickerState::default();
        picker.move_grid(PICKER_COLUMNS as isize, 64);
        assert_eq!(picker.selected_index, PICKER_COLUMNS);
        assert_eq!(picker.scroll_row, 0);

        picker.move_grid((PICKER_COLUMNS * 5) as isize, 64);
        assert_eq!(picker.selected_index, PICKER_COLUMNS * 6);
        assert_eq!(picker.scroll_row, 2);

        picker.reset();
        picker.move_grid(-1, 64);
        assert_eq!(picker.selected_index, 63);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn colrv1_rasterizer_renders_rocket_when_colrv1_font_present() {
        if colrv1::is_available() {
            let gid = colrv1::lookup_single_codepoint_glyph_id("🚀");
            assert!(gid.is_some());
            if let Some(glyph_id) = gid {
                let image = colrv1::rasterize_glyph_id(glyph_id, 32);
                assert!(image.is_some());
                if let Some(rendered) = image {
                    let bytes = rendered.as_bytes(0).unwrap_or(&[]);
                    assert!(bytes.iter().any(|&byte| byte != 0));
                }
            }
        }
    }
}
