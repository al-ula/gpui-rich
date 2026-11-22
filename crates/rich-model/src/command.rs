//! Command model unifying viewer interactions, GPUI actions, and future editing operations.

use crate::block::BlockId;
use crate::inline::InlineObject;
use crate::selection::{DocPos, DocumentSelection};
use crate::style::{BlockStyle, HorizontalLayout, InlineStyle, TextAlign};
use serde::{Deserialize, Serialize};

/// Cardinal direction for caret navigation and deletion commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MoveDirection {
    Backward,
    Forward,
    Up,
    Down,
}

/// Granularity for caret navigation and deletion commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MoveUnit {
    Grapheme,
    Word,
    Line,
    LineBoundary,
    DocumentBoundary,
}

/// Toggleable inline formatting marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkKind {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    InlineCode,
    Superscript,
    Subscript,
}

/// Unified command representation for toolbar controls, keybindings, command palettes,
/// mouse interactions, and Phase 2 editing mutations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RichCommand {
    // View & canvas commands
    SetHorizontalLayout(HorizontalLayout),
    SetZoom(f32),
    AdjustZoom(f32),
    ResetZoom,
    ScrollBy { delta_x: f32, delta_y: f32 },
    ToggleDebugOverlay,

    // Navigation, selection, and reader interactions
    SetSelection(Option<DocumentSelection>),
    ClearSelection,
    SelectAll,
    SelectWordAt(DocPos),
    MoveCaret {
        direction: MoveDirection,
        unit: MoveUnit,
        extend_selection: bool,
    },
    CopySelection,
    OpenLink(String),
    ToggleHeadingCollapsed(BlockId),

    // History & Phase 2 document editing commands
    Undo,
    Redo,
    InsertText(String),
    InsertInlineObject(InlineObject),
    Delete {
        direction: MoveDirection,
        unit: MoveUnit,
    },
    SplitBlock,
    InsertSoftBreak,
    ToggleMark(MarkKind),
    SetInlineStyle(InlineStyle),
    SetBlockStyle(BlockStyle),
    SetTextAlign(TextAlign),
}

impl RichCommand {
    /// Returns true when the command mutates document content or document styles.
    pub fn is_document_mutation(&self) -> bool {
        matches!(
            self,
            Self::Undo
                | Self::Redo
                | Self::InsertText(_)
                | Self::InsertInlineObject(_)
                | Self::Delete { .. }
                | Self::SplitBlock
                | Self::InsertSoftBreak
                | Self::ToggleMark(_)
                | Self::SetInlineStyle(_)
                | Self::SetBlockStyle(_)
                | Self::SetTextAlign(_)
        )
    }

    /// Returns a stable dot-namespaced identifier for keymaps, palettes, and logging.
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::SetHorizontalLayout(_) => "rich.view.set_horizontal_layout",
            Self::SetZoom(_) => "rich.view.set_zoom",
            Self::AdjustZoom(_) => "rich.view.adjust_zoom",
            Self::ResetZoom => "rich.view.reset_zoom",
            Self::ScrollBy { .. } => "rich.view.scroll_by",
            Self::ToggleDebugOverlay => "rich.view.toggle_debug_overlay",
            Self::SetSelection(_) => "rich.selection.set",
            Self::ClearSelection => "rich.selection.clear",
            Self::SelectAll => "rich.selection.select_all",
            Self::SelectWordAt(_) => "rich.selection.select_word",
            Self::MoveCaret { .. } => "rich.selection.move_caret",
            Self::CopySelection => "rich.clipboard.copy",
            Self::OpenLink(_) => "rich.navigation.open_link",
            Self::ToggleHeadingCollapsed(_) => "rich.view.toggle_heading_collapsed",
            Self::Undo => "rich.history.undo",
            Self::Redo => "rich.history.redo",
            Self::InsertText(_) => "rich.edit.insert_text",
            Self::InsertInlineObject(_) => "rich.edit.insert_inline_object",
            Self::Delete { .. } => "rich.edit.delete",
            Self::SplitBlock => "rich.edit.split_block",
            Self::InsertSoftBreak => "rich.edit.insert_soft_break",
            Self::ToggleMark(_) => "rich.format.toggle_mark",
            Self::SetInlineStyle(_) => "rich.format.set_inline_style",
            Self::SetBlockStyle(_) => "rich.format.set_block_style",
            Self::SetTextAlign(_) => "rich.format.set_text_align",
        }
    }
}

/// Outcome returned by a command dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Executed,
    NoOp,
    RejectedReadOnly,
    Unsupported,
}
