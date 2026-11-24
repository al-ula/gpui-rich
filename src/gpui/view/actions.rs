//! GPUI actions, default keybindings, host events, and auxiliary view state types.

use super::*;

/// GPUI keymap context attached to [`RichDocumentView`].
pub const RICH_DOCUMENT_KEY_CONTEXT: &str = "RichDocument";

actions!(
    rich_document,
    [
        Copy,
        Cut,
        Paste,
        PastePlainText,
        SelectAll,
        ClearSelection,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleDebugOverlay,
        Undo,
        Redo,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveLineStart,
        MoveLineEnd,
        SelectLineStart,
        SelectLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectDocStart,
        SelectDocEnd,
        Backspace,
        Delete,
        BackspaceWord,
        DeleteWord,
        Enter,
        ExitBlock,
        SoftBreak,
        Indent,
        Outdent,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline,
        ToggleStrikethrough,
        ToggleInlineCode,
        ToggleSuperscript,
        ToggleSubscript,
        SetHeading1,
        SetHeading2,
        SetHeading3,
        SetParagraph,
        ToggleBulletList,
        ToggleOrderedList,
        ToggleChecklist,
        ToggleBlockquote,
        ToggleCodeBlock,
        InsertHorizontalRule,
        AlignLeft,
        AlignCenter,
        AlignRight,
        AlignJustify,
        ClearFormatting,
        IncreaseFontSize,
        DecreaseFontSize,
        InsertTableRowAbove,
        InsertTableRowBelow,
        DeleteTableRow,
        InsertTableColumnBefore,
        InsertTableColumnAfter,
        DeleteTableColumn,
        MergeTableCells,
        SplitTableCell,
    ]
);

/// Registers default cross-platform keybindings scoped to [`RICH_DOCUMENT_KEY_CONTEXT`].
pub fn register_default_keybindings(cx: &mut App) {
    let context = Some(RICH_DOCUMENT_KEY_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("escape", ClearSelection, context),
        KeyBinding::new("left", MoveLeft, context),
        KeyBinding::new("right", MoveRight, context),
        KeyBinding::new("up", MoveUp, context),
        KeyBinding::new("down", MoveDown, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", MoveLineStart, context),
        KeyBinding::new("end", MoveLineEnd, context),
        KeyBinding::new("shift-home", SelectLineStart, context),
        KeyBinding::new("shift-end", SelectLineEnd, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("enter", Enter, context),
        KeyBinding::new("shift-enter", SoftBreak, context),
        KeyBinding::new("tab", Indent, context),
        KeyBinding::new("shift-tab", Outdent, context),
    ];

    // Use runtime platform detection so WebAssembly builds match the browser host OS.
    if cx.platform_info().is_mac() {
        bindings.extend([
            KeyBinding::new("cmd-c", Copy, context),
            KeyBinding::new("cmd-x", Cut, context),
            KeyBinding::new("cmd-v", Paste, context),
            KeyBinding::new("cmd-shift-v", PastePlainText, context),
            KeyBinding::new("cmd-a", SelectAll, context),
            KeyBinding::new("cmd-=", ZoomIn, context),
            KeyBinding::new("cmd--", ZoomOut, context),
            KeyBinding::new("cmd-0", ZoomReset, context),
            KeyBinding::new("cmd-z", Undo, context),
            KeyBinding::new("cmd-shift-z", Redo, context),
            KeyBinding::new("cmd-y", Redo, context),
            KeyBinding::new("cmd-b", ToggleBold, context),
            KeyBinding::new("cmd-i", ToggleItalic, context),
            KeyBinding::new("cmd-u", ToggleUnderline, context),
            KeyBinding::new("cmd-e", ToggleInlineCode, context),
            KeyBinding::new("cmd-shift-x", ToggleStrikethrough, context),
            KeyBinding::new("cmd-\\", ClearFormatting, context),
            KeyBinding::new("cmd-shift-.", IncreaseFontSize, context),
            KeyBinding::new("cmd-shift-,", DecreaseFontSize, context),
            KeyBinding::new("cmd-alt-1", SetHeading1, context),
            KeyBinding::new("cmd-alt-2", SetHeading2, context),
            KeyBinding::new("cmd-alt-3", SetHeading3, context),
            KeyBinding::new("cmd-alt-0", SetParagraph, context),
            KeyBinding::new("cmd-shift-8", ToggleBulletList, context),
            KeyBinding::new("cmd-shift-7", ToggleOrderedList, context),
            KeyBinding::new("cmd-shift-9", ToggleChecklist, context),
            KeyBinding::new("cmd-enter", ExitBlock, context),
            KeyBinding::new("cmd-shift-l", AlignLeft, context),
            KeyBinding::new("cmd-shift-e", AlignCenter, context),
            KeyBinding::new("cmd-shift-r", AlignRight, context),
            KeyBinding::new("cmd-shift-j", AlignJustify, context),
            KeyBinding::new("alt-left", MoveWordLeft, context),
            KeyBinding::new("alt-right", MoveWordRight, context),
            KeyBinding::new("alt-shift-left", SelectWordLeft, context),
            KeyBinding::new("alt-shift-right", SelectWordRight, context),
            KeyBinding::new("cmd-left", MoveLineStart, context),
            KeyBinding::new("cmd-right", MoveLineEnd, context),
            KeyBinding::new("cmd-shift-left", SelectLineStart, context),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, context),
            KeyBinding::new("cmd-up", MoveDocStart, context),
            KeyBinding::new("cmd-down", MoveDocEnd, context),
            KeyBinding::new("cmd-shift-up", SelectDocStart, context),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, context),
            KeyBinding::new("alt-backspace", BackspaceWord, context),
            KeyBinding::new("alt-delete", DeleteWord, context),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-c", Copy, context),
            KeyBinding::new("ctrl-x", Cut, context),
            KeyBinding::new("ctrl-v", Paste, context),
            KeyBinding::new("ctrl-shift-v", PastePlainText, context),
            KeyBinding::new("ctrl-a", SelectAll, context),
            KeyBinding::new("ctrl-=", ZoomIn, context),
            KeyBinding::new("ctrl--", ZoomOut, context),
            KeyBinding::new("ctrl-0", ZoomReset, context),
            KeyBinding::new("ctrl-z", Undo, context),
            KeyBinding::new("ctrl-shift-z", Redo, context),
            KeyBinding::new("ctrl-y", Redo, context),
            KeyBinding::new("ctrl-b", ToggleBold, context),
            KeyBinding::new("ctrl-i", ToggleItalic, context),
            KeyBinding::new("ctrl-u", ToggleUnderline, context),
            KeyBinding::new("ctrl-e", ToggleInlineCode, context),
            KeyBinding::new("ctrl-shift-x", ToggleStrikethrough, context),
            KeyBinding::new("ctrl-\\", ClearFormatting, context),
            KeyBinding::new("ctrl-shift-.", IncreaseFontSize, context),
            KeyBinding::new("ctrl-shift-,", DecreaseFontSize, context),
            KeyBinding::new("ctrl-alt-1", SetHeading1, context),
            KeyBinding::new("ctrl-alt-2", SetHeading2, context),
            KeyBinding::new("ctrl-alt-3", SetHeading3, context),
            KeyBinding::new("ctrl-alt-0", SetParagraph, context),
            KeyBinding::new("ctrl-shift-8", ToggleBulletList, context),
            KeyBinding::new("ctrl-shift-7", ToggleOrderedList, context),
            KeyBinding::new("ctrl-shift-9", ToggleChecklist, context),
            KeyBinding::new("ctrl-enter", ExitBlock, context),
            KeyBinding::new("ctrl-shift-l", AlignLeft, context),
            KeyBinding::new("ctrl-shift-e", AlignCenter, context),
            KeyBinding::new("ctrl-shift-r", AlignRight, context),
            KeyBinding::new("ctrl-shift-j", AlignJustify, context),
            KeyBinding::new("ctrl-left", MoveWordLeft, context),
            KeyBinding::new("ctrl-right", MoveWordRight, context),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
            KeyBinding::new("ctrl-home", MoveDocStart, context),
            KeyBinding::new("ctrl-end", MoveDocEnd, context),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, context),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, context),
            KeyBinding::new("ctrl-backspace", BackspaceWord, context),
            KeyBinding::new("ctrl-delete", DeleteWord, context),
        ]);
    }

    cx.bind_keys(bindings);
}

/// Identifier returned when registering a [`CommandMiddleware`] on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MiddlewareId(pub(super) u64);

/// Identifier returned when registering a [`DecorationProvider`] on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecorationProviderId(pub(super) u64);

/// Ephemeral IME composition state held on [`RichDocumentView`] outside [`Document`] and [`History`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImePreedit {
    pub block_id: BlockId,
    /// UTF-8 byte range in `block_id` that will be replaced upon composition commit.
    pub replace_range: Range<usize>,
    /// Uncommitted composition text displayed with an inline underline.
    pub text: String,
    /// Selected byte range within [`Self::text`].
    pub selected_range: Range<usize>,
}

/// Identifies a selected inline image displaying drag-resize handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectedInlineImage {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
}

/// Corner handle of a selected inline image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub(crate) struct ImageResizeDrag {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub corner: ImageResizeCorner,
    pub start_canvas_point: Point,
    pub initial_width_pt: f32,
    pub initial_height_pt: f32,
    pub current_width_pt: f32,
    pub current_height_pt: f32,
}

/// Active floating popover anchored to a clicked [`crate::model::InlineObject::SmartChip`].
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSmartChipPopover {
    pub block_id: BlockId,
    pub object_id: InlineObjectId,
    pub kind: ChipKind,
    pub label: String,
    pub accent: Option<RgbaColor>,
}

pub(crate) struct TableColumnDrag {
    pub table_id: BlockId,
    pub column_index: usize,
    pub start_canvas_x: f32,
    pub initial_divider_x: f32,
    pub current_divider_x: f32,
    pub table_y: f32,
    pub table_height: f32,
    pub initial_widths_px: Vec<f32>,
}

/// Describes the active table cell/selection context for host toolbars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveTableContext {
    pub table_id: BlockId,
    pub row: usize,
    pub visual_col: usize,
    pub cell_index: usize,
    pub can_merge: bool,
    pub can_split: bool,
}

/// Host-observable events emitted by [`RichDocumentView`] via GPUI [`EventEmitter`].
#[derive(Clone, Debug, PartialEq)]
pub enum RichDocumentEvent {
    /// A hyperlink was activated by user click or programmatic [`RichCommand::OpenLink`].
    LinkActivated { href: String },
    /// A smart chip was clicked by the user.
    SmartChipActivated {
        block_id: BlockId,
        object_id: InlineObjectId,
        kind: ChipKind,
        label: String,
    },
    /// A code block's contents were copied via its built-in Copy button or [`RichCommand::CopyCodeBlock`].
    CodeBlockCopied { block_id: BlockId },
    /// The active selection or caret changed.
    SelectionChanged {
        selection: Option<Box<DocumentSelection>>,
    },
    /// A [`Transaction`] mutated the underlying [`Document`].
    DocumentMutated { change_set: ChangeSet },
    /// A [`RichCommand`] completed, no-oped, or was rejected/cancelled in the dispatcher.
    CommandExecuted {
        command_id: &'static str,
        effect: CommandEffectKind,
        resolution: CommandResolution,
        command: RichCommand,
        source: CommandSource,
        outcome: CommandOutcome,
    },
}
