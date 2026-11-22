//! Command model unifying viewer interactions, GPUI actions, and future editing operations.

use crate::block::{BlockId, Document, ListKind};
use crate::inline::InlineObject;
use crate::selection::{DocPos, DocumentSelection};
use crate::style::{
    BlockStyle, Edges, HorizontalLayout, InlineStyle, LinkTarget, NamedStyle, StyleId, TextAlign,
};
use crate::transaction::ChangeSet;
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
    SetMargins(Edges),
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
    Indent,
    Outdent,
    ToggleMark(MarkKind),
    SetInlineStyle(InlineStyle),
    ClearFormatting,
    AdjustFontSize(f32),
    SetLink(Option<LinkTarget>),
    SetBlockStyle(BlockStyle),
    UpdateNamedStyle {
        style_id: StyleId,
        style: NamedStyle,
    },
    SetHeading(Option<u8>),
    ToggleList(ListKind),
    SetTextAlign(TextAlign),
    PasteHtml {
        html: String,
        plain_text: Option<String>,
    },
    /// Host-defined custom command routed through policy gating, middleware, and `ExtensionRegistry`.
    Custom {
        id: String,
        payload: serde_json::Value,
        mutates_document: bool,
    },
}

impl RichCommand {
    /// Returns true when the command mutates document content or document styles.
    pub fn is_document_mutation(&self) -> bool {
        match self {
            Self::Undo
            | Self::Redo
            | Self::InsertText(_)
            | Self::InsertInlineObject(_)
            | Self::Delete { .. }
            | Self::SplitBlock
            | Self::InsertSoftBreak
            | Self::Indent
            | Self::Outdent
            | Self::ToggleMark(_)
            | Self::SetInlineStyle(_)
            | Self::ClearFormatting
            | Self::AdjustFontSize(_)
            | Self::SetLink(_)
            | Self::SetBlockStyle(_)
            | Self::UpdateNamedStyle { .. }
            | Self::SetHeading(_)
            | Self::ToggleList(_)
            | Self::SetTextAlign(_)
            | Self::PasteHtml { .. } => true,
            Self::Custom {
                mutates_document, ..
            } => *mutates_document,
            _ => false,
        }
    }

    /// Returns a stable dot-namespaced identifier for keymaps, palettes, and logging.
    pub fn command_id(&self) -> &str {
        match self {
            Self::SetHorizontalLayout(_) => "rich.view.set_horizontal_layout",
            Self::SetMargins(_) => "rich.view.set_margins",
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
            Self::Indent => "rich.edit.indent",
            Self::Outdent => "rich.edit.outdent",
            Self::ToggleMark(_) => "rich.format.toggle_mark",
            Self::SetInlineStyle(_) => "rich.format.set_inline_style",
            Self::ClearFormatting => "rich.format.clear_formatting",
            Self::AdjustFontSize(_) => "rich.format.adjust_font_size",
            Self::SetLink(_) => "rich.format.set_link",
            Self::SetBlockStyle(_) => "rich.format.set_block_style",
            Self::UpdateNamedStyle { .. } => "rich.format.update_named_style",
            Self::SetHeading(_) => "rich.format.set_heading",
            Self::ToggleList(_) => "rich.format.toggle_list",
            Self::SetTextAlign(_) => "rich.format.set_text_align",
            Self::PasteHtml { .. } => "rich.clipboard.paste_html",
            Self::Custom { id, .. } => id.as_str(),
        }
    }
}

/// Origin of a dispatched [`RichCommand`], used for policy gating and middleware decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum CommandSource {
    /// Initiated by direct user input (keyboard, mouse, IME, toolbar action).
    #[default]
    User,
    /// Initiated programmatically by the host application, plugins, or sync layers.
    Programmatic,
}

/// Controls which user-initiated interactions and mutations are permitted on a document view.
///
/// Commands dispatched with [`CommandSource::Programmatic`] bypass user-interaction gates while
/// still executing through middleware, transactions, layout synchronization, and host events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionPolicy {
    pub editable: bool,
    pub selectable: bool,
    pub clickable_links: bool,
    pub collapsible_headings: bool,
    pub show_caret_read_only: bool,
}

impl Default for InteractionPolicy {
    fn default() -> Self {
        Self::viewer()
    }
}

impl InteractionPolicy {
    /// Full editing preset: document mutations, selection, links, and collapsible headings enabled.
    pub const fn editable() -> Self {
        Self {
            editable: true,
            selectable: true,
            clickable_links: true,
            collapsible_headings: true,
            show_caret_read_only: false,
        }
    }

    /// Interactive read-only viewer preset: selection, copy, links, and collapsible headings enabled.
    pub const fn viewer() -> Self {
        Self {
            editable: false,
            selectable: true,
            clickable_links: true,
            collapsible_headings: true,
            show_caret_read_only: false,
        }
    }

    /// Non-interactive preview preset: scrolling and zoom remain active while editing, selection,
    /// link activation, and heading collapse are disabled for user input.
    pub const fn static_preview() -> Self {
        Self {
            editable: false,
            selectable: false,
            clickable_links: false,
            collapsible_headings: false,
            show_caret_read_only: false,
        }
    }

    /// Returns true when a collapsed caret should be visible.
    pub const fn should_show_caret(&self) -> bool {
        self.editable || self.show_caret_read_only
    }

    /// Evaluates whether `command` from `source` is permitted under this policy.
    ///
    /// Returns `Some(rejection)` when blocked, or `None` when permitted.
    pub fn check_command(
        &self,
        command: &RichCommand,
        source: CommandSource,
    ) -> Option<CommandOutcome> {
        if source == CommandSource::Programmatic {
            return None;
        }
        if command.is_document_mutation() && !self.editable {
            return Some(CommandOutcome::RejectedReadOnly);
        }
        match command {
            RichCommand::SetSelection(Some(_))
            | RichCommand::SelectAll
            | RichCommand::SelectWordAt(_)
            | RichCommand::MoveCaret { .. }
            | RichCommand::CopySelection
                if !self.selectable =>
            {
                Some(CommandOutcome::RejectedByPolicy)
            }
            RichCommand::OpenLink(_) if !self.clickable_links => {
                Some(CommandOutcome::RejectedByPolicy)
            }
            RichCommand::ToggleHeadingCollapsed(_) if !self.collapsible_headings => {
                Some(CommandOutcome::RejectedByPolicy)
            }
            _ => None,
        }
    }
}

/// Decision returned by [`CommandMiddleware::before_command`].
#[derive(Clone, Debug, PartialEq)]
pub enum CommandDecision {
    /// Proceed with the current command.
    Continue,
    /// Abort command execution and report [`CommandOutcome::Cancelled`].
    Cancel,
    /// Substitute a different command for remaining middleware and execution.
    Replace(RichCommand),
}

/// Extension hook intercepting commands before execution and observing applied transactions.
pub trait CommandMiddleware {
    /// Inspects, cancels, or replaces a command prior to policy enforcement and execution.
    fn before_command(
        &mut self,
        _command: &RichCommand,
        _source: CommandSource,
        _document: &Document,
        _selection: Option<&DocumentSelection>,
    ) -> CommandDecision {
        CommandDecision::Continue
    }

    /// Invoked after a [`crate::Transaction`] mutates the [`Document`].
    fn after_transaction(
        &mut self,
        _change_set: &ChangeSet,
        _document: &Document,
        _selection: Option<&DocumentSelection>,
    ) {
    }
}

/// Outcome returned by a command dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Executed,
    NoOp,
    RejectedReadOnly,
    RejectedByPolicy,
    Cancelled,
    Unsupported,
}
