//! Layer A: unified anchored overlay system for caret menus, selection bubble toolbars,
//! hover previews, and inline-object popovers.
//!
//! Overlays resolve their anchor geometry inside [`crate::gpui::RichDocumentElement`]'s prepaint
//! pass for zero-lag scroll and zoom tracking, measure their intrinsic size via GPUI layout,
//! auto-flip and clamp to bounds, and paint in GPUI's deferred draw pass outside the document
//! viewport clip mask.

use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::layout::{Point, Rect, Size};
use crate::model::{Affinity, BlockId, DocPos, InlineObjectId, SharedStr};
use gpui_kit::{AnyElement, App, Bounds, Context, Entity, Pixels, Window};
use std::cell::RefCell;
use std::rc::Rc;

/// Built-in overlay identifier for clicked smart-chip and dropdown popovers.
pub const SMART_CHIP_POPOVER_OVERLAY_ID: &str = "gpui_rich.smart_chip_popover";

/// Built-in overlay identifier for the Layer B completion and action menu.
pub const COMPLETION_MENU_OVERLAY_ID: &str = "gpui_rich.completion_menu";

/// Stable identifier for a registered anchored overlay on [`RichDocumentView`].
///
/// Updating an overlay with the same [`OverlayId`] replaces it in place without flicker.
/// Extensions should use dot-namespaced identifiers (for example, `"acme.math.editor"`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OverlayId(SharedStr);

impl OverlayId {
    /// Creates a namespaced overlay identifier.
    pub fn new(id: impl Into<SharedStr>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying string identifier.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl From<&'static str> for OverlayId {
    fn from(value: &'static str) -> Self {
        Self(SharedStr::from(value))
    }
}

impl From<String> for OverlayId {
    fn from(value: String) -> Self {
        Self(SharedStr::from(value))
    }
}

/// Stacking layer for anchored overlays painted above the document canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayZLayer {
    /// Passive popovers, hover cards, link previews, and inline smart-chip popovers.
    Passive,
    /// Floating interactive surfaces such as selection formatting toolbars.
    Interactive,
    /// Caret-anchored completion and action menus.
    Menu,
}

impl OverlayZLayer {
    /// Returns the GPUI deferred-draw priority for this stacking layer.
    pub const fn deferred_priority(self) -> usize {
        match self {
            Self::Passive => 10,
            Self::Interactive => 20,
            Self::Menu => 30,
        }
    }
}

/// Document geometry target that an anchored overlay tracks each frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayAnchor {
    /// Tracks the active caret (or selection focus edge).
    Caret,
    /// Tracks a pinned document offset (such as a `/` or `@` trigger character).
    DocPos { pos: DocPos, affinity: Affinity },
    /// Tracks the bounding box of the active non-collapsed selection.
    Selection,
    /// Tracks a specific inline atomic object inside a block.
    InlineObject {
        block_id: BlockId,
        object_id: InlineObjectId,
    },
}

/// Preferred vertical side relative to the anchor rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlaySide {
    Below,
    Above,
}

/// Horizontal alignment relative to the anchor rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAlign {
    Start,
    Center,
    End,
}

/// Behavior when an overlay's anchor scrolls outside the visible document viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayOffscreenBehavior {
    /// Hide while offscreen and reappear automatically when scrolled back into view.
    Hide,
    /// Dismiss permanently once the anchor leaves the visible viewport.
    Dismiss,
}

/// Positioning, flip, clamping, and offscreen rules for an anchored overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayPlacement {
    pub preferred_side: OverlaySide,
    pub align: OverlayAlign,
    pub gap_px: f32,
    pub edge_padding_px: f32,
    pub auto_flip: bool,
    pub clamp_to_window: bool,
    pub offscreen: OverlayOffscreenBehavior,
}

impl Default for OverlayPlacement {
    fn default() -> Self {
        Self::below_start()
    }
}

impl OverlayPlacement {
    /// Standard menu/popover placement: below the anchor, aligned to its leading edge.
    pub const fn below_start() -> Self {
        Self {
            preferred_side: OverlaySide::Below,
            align: OverlayAlign::Start,
            gap_px: 6.0,
            edge_padding_px: 8.0,
            auto_flip: true,
            clamp_to_window: true,
            offscreen: OverlayOffscreenBehavior::Hide,
        }
    }

    /// Standard selection bubble toolbar placement: centered above the active selection.
    pub const fn above_center() -> Self {
        Self {
            preferred_side: OverlaySide::Above,
            align: OverlayAlign::Center,
            gap_px: 8.0,
            edge_padding_px: 8.0,
            auto_flip: true,
            clamp_to_window: true,
            offscreen: OverlayOffscreenBehavior::Hide,
        }
    }

    /// Overrides the vertical gap between the anchor and overlay in pixels.
    pub const fn with_gap_px(mut self, gap_px: f32) -> Self {
        self.gap_px = gap_px;
        self
    }

    /// Controls whether the overlay clamps to the full window or stays inside the editor viewport.
    pub const fn with_clamp_to_window(mut self, clamp_to_window: bool) -> Self {
        self.clamp_to_window = clamp_to_window;
        self
    }

    /// Sets the offscreen anchor behavior.
    pub const fn with_offscreen(mut self, offscreen: OverlayOffscreenBehavior) -> Self {
        self.offscreen = offscreen;
        self
    }
}

/// Lifecycle hooks describing which user interactions automatically dismiss an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OverlayDismissPolicy {
    pub on_outside_click: bool,
    pub on_blur: bool,
    pub on_scroll: bool,
    pub on_selection_change: bool,
    pub on_document_edit: bool,
}

impl OverlayDismissPolicy {
    /// Dismisses on outside click, blur, selection change, and document edit.
    pub const fn popover() -> Self {
        Self {
            on_outside_click: true,
            on_blur: true,
            on_scroll: false,
            on_selection_change: true,
            on_document_edit: true,
        }
    }

    /// Stays open across selection updates and formatting edits, dismissing on blur.
    pub const fn selection_toolbar() -> Self {
        Self {
            on_outside_click: false,
            on_blur: true,
            on_scroll: false,
            on_selection_change: false,
            on_document_edit: false,
        }
    }

    /// Dismisses on outside click and blur while letting the menu controller manage edits.
    pub const fn menu() -> Self {
        Self {
            on_outside_click: true,
            on_blur: true,
            on_scroll: false,
            on_selection_change: false,
            on_document_edit: false,
        }
    }
}

/// Navigation and confirmation keys that a key-capturing overlay may intercept while focus
/// remains on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayKeyAction {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Tab,
    ShiftTab,
    Enter,
    Escape,
}

/// Result returned by an overlay key handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKeyResult {
    /// The overlay consumed the key; do not run the default document action.
    Handled,
    /// The overlay did not consume the key; continue with the default document action.
    PassThrough,
}

/// Read-only context supplied when rendering an anchored overlay element.
pub struct OverlayRenderContext<'render> {
    pub id: &'render OverlayId,
    pub view: Entity<RichDocumentView>,
    pub anchor_bounds: Bounds<Pixels>,
    pub viewport_bounds: Bounds<Pixels>,
    pub theme: &'render RichTheme,
    pub zoom: f32,
}

/// Render callback constructing the GPUI element for an anchored overlay.
pub type OverlayRenderFn =
    Rc<dyn Fn(&OverlayRenderContext<'_>, &mut Window, &mut App) -> AnyElement>;

/// Key-intercept callback invoked for the top-most key-capturing overlay when no IME preedit is active.
pub type OverlayKeyHandler = Rc<
    RefCell<
        dyn FnMut(
            OverlayKeyAction,
            &mut RichDocumentView,
            &mut Window,
            &mut Context<RichDocumentView>,
        ) -> OverlayKeyResult,
    >,
>;

/// Full registration descriptor for an anchored overlay on [`RichDocumentView`].
#[derive(Clone)]
pub struct OverlaySpec {
    pub id: OverlayId,
    pub anchor: OverlayAnchor,
    pub placement: OverlayPlacement,
    pub z_layer: OverlayZLayer,
    pub dismiss: OverlayDismissPolicy,
    pub render: OverlayRenderFn,
    pub key_handler: Option<OverlayKeyHandler>,
}

impl OverlaySpec {
    /// Creates an anchored overlay specification with default placement, passive z-layer,
    /// popover dismiss policy, and no key interception.
    pub fn new(
        id: impl Into<OverlayId>,
        anchor: OverlayAnchor,
        render: impl Fn(&OverlayRenderContext<'_>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            anchor,
            placement: OverlayPlacement::default(),
            z_layer: OverlayZLayer::Passive,
            dismiss: OverlayDismissPolicy::popover(),
            render: Rc::new(render),
            key_handler: None,
        }
    }

    /// Sets the overlay placement strategy.
    pub const fn with_placement(mut self, placement: OverlayPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Sets the stacking layer for paint order and key-capture priority.
    pub const fn with_z_layer(mut self, z_layer: OverlayZLayer) -> Self {
        self.z_layer = z_layer;
        self
    }

    /// Sets the automatic dismissal rules.
    pub const fn with_dismiss_policy(mut self, dismiss: OverlayDismissPolicy) -> Self {
        self.dismiss = dismiss;
        self
    }

    /// Attaches a focus-preserving key-intercept handler to this overlay.
    pub fn with_key_handler(
        mut self,
        handler: impl FnMut(
            OverlayKeyAction,
            &mut RichDocumentView,
            &mut Window,
            &mut Context<RichDocumentView>,
        ) -> OverlayKeyResult
        + 'static,
    ) -> Self {
        self.key_handler = Some(Rc::new(RefCell::new(handler)));
        self
    }
}

/// Stores active keyed overlays in deterministic z-layer and insertion order.
#[derive(Default)]
pub(crate) struct OverlayRegistry {
    entries: Vec<(u64, OverlaySpec)>,
    next_order: u64,
}

impl OverlayRegistry {
    pub(crate) fn set(&mut self, spec: OverlaySpec) {
        if let Some((_, existing)) = self
            .entries
            .iter_mut()
            .find(|(_, entry)| entry.id == spec.id)
        {
            *existing = spec;
            return;
        }
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        self.entries.push((order, spec));
    }

    pub(crate) fn remove(&mut self, id: &OverlayId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(_, entry)| &entry.id != id);
        self.entries.len() != before
    }

    pub(crate) fn contains(&self, id: &OverlayId) -> bool {
        self.entries.iter().any(|(_, entry)| &entry.id == id)
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.entries.clear();
        true
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns overlays sorted from lowest to highest z-layer (then insertion order).
    pub(crate) fn sorted_specs(&self) -> Vec<OverlaySpec> {
        let mut ordered: Vec<(OverlayZLayer, u64, OverlaySpec)> = self
            .entries
            .iter()
            .map(|(order, spec)| (spec.z_layer, *order, spec.clone()))
            .collect();
        ordered.sort_by_key(|(z_layer, order, _)| (*z_layer, *order));
        ordered.into_iter().map(|(_, _, spec)| spec).collect()
    }

    /// Returns the top-most registered key-capturing handler (highest z-layer, latest inserted).
    pub(crate) fn top_key_handler(&self) -> Option<OverlayKeyHandler> {
        self.entries
            .iter()
            .filter_map(|(order, spec)| {
                spec.key_handler
                    .as_ref()
                    .map(|handler| (spec.z_layer, *order, Rc::clone(handler)))
            })
            .max_by_key(|(z_layer, order, _)| (*z_layer, *order))
            .map(|(_, _, handler)| handler)
    }

    pub(crate) fn retain(&mut self, mut predicate: impl FnMut(&OverlaySpec) -> bool) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(_, entry)| predicate(entry));
        self.entries.len() != before
    }
}

/// Returns true when `anchor` intersects `viewport`.
pub fn is_anchor_visible_in_viewport(anchor: Rect, viewport: Rect) -> bool {
    anchor.right() >= viewport.x
        && anchor.x <= viewport.right()
        && anchor.bottom() >= viewport.y
        && anchor.y <= viewport.bottom()
}

/// Computes the clamped window-space origin for an overlay of `overlay_size` anchored to `anchor`
/// within `clamp_bounds`, applying `placement`'s alignment, gap, and vertical auto-flip rules.
pub fn resolve_overlay_origin(
    anchor: Rect,
    overlay_size: Size,
    clamp_bounds: Rect,
    placement: OverlayPlacement,
) -> Point {
    let padding = placement.edge_padding_px.max(0.0);
    let gap = placement.gap_px.max(0.0);

    let raw_x = match placement.align {
        OverlayAlign::Start => anchor.x,
        OverlayAlign::Center => anchor.x + (anchor.width - overlay_size.width) * 0.5,
        OverlayAlign::End => anchor.right() - overlay_size.width,
    };
    let min_x = clamp_bounds.x + padding;
    let max_x = (clamp_bounds.right() - overlay_size.width - padding).max(min_x);
    let x = raw_x.clamp(min_x, max_x);

    let below_y = anchor.bottom() + gap;
    let above_y = anchor.y - overlay_size.height - gap;
    let min_y = clamp_bounds.y + padding;
    let max_y = (clamp_bounds.bottom() - overlay_size.height - padding).max(min_y);

    let fits_below = below_y + overlay_size.height <= clamp_bounds.bottom() - padding;
    let fits_above = above_y >= min_y;

    let preferred_y = match placement.preferred_side {
        OverlaySide::Below => {
            if fits_below || !placement.auto_flip || !fits_above {
                below_y
            } else {
                above_y
            }
        }
        OverlaySide::Above => {
            if fits_above || !placement.auto_flip || !fits_below {
                above_y
            } else {
                below_y
            }
        }
    };

    let y = preferred_y.clamp(min_y, max_y);
    Point::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_flips_above_when_bottom_overflows() {
        let viewport = Rect::new(0.0, 0.0, 800.0, 400.0);
        let anchor = Rect::new(100.0, 340.0, 2.0, 20.0);
        let overlay = Size::new(240.0, 140.0);
        let origin =
            resolve_overlay_origin(anchor, overlay, viewport, OverlayPlacement::below_start());
        assert_eq!(origin.x, 100.0);
        assert_eq!(origin.y, 340.0 - 140.0 - 6.0);
    }

    #[test]
    fn placement_flips_below_when_top_overflows_for_selection_toolbar() {
        let viewport = Rect::new(0.0, 0.0, 800.0, 400.0);
        let anchor = Rect::new(200.0, 14.0, 120.0, 20.0);
        let overlay = Size::new(180.0, 36.0);
        let origin =
            resolve_overlay_origin(anchor, overlay, viewport, OverlayPlacement::above_center());
        assert_eq!(origin.x, 170.0);
        assert_eq!(origin.y, 14.0 + 20.0 + 8.0);
    }

    #[test]
    fn placement_clamps_horizontally_inside_bounds() {
        let viewport = Rect::new(20.0, 10.0, 400.0, 300.0);
        let anchor = Rect::new(390.0, 80.0, 2.0, 18.0);
        let overlay = Size::new(220.0, 100.0);
        let origin =
            resolve_overlay_origin(anchor, overlay, viewport, OverlayPlacement::below_start());
        assert_eq!(origin.x, 20.0 + 400.0 - 220.0 - 8.0);
        assert_eq!(origin.y, 80.0 + 18.0 + 6.0);
    }
}
