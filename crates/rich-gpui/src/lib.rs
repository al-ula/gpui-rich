//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

pub mod element;
pub mod extension;
pub mod shaper;
pub mod theme;
pub mod view;

pub use element::RichDocumentElement;
pub use extension::{
    CustomBlockRenderContext, CustomCommandHandler, CustomCommandResult, CustomInlineRenderContext,
    CustomRenderOutput, DirectPaintCallback, ExtensionRegistry, NodeExtension,
};
pub use shaper::{
    FontResolver, GpuiGlyphBackend, GpuiShaper, clear_font_resolution_cache, resolve_font_family,
    rgba_to_hsla, to_gpui_font,
};
pub use theme::{RichTheme, SmartChipColors, SmartChipPalette};
pub use view::{
    AlignCenter, AlignJustify, AlignLeft, AlignRight, Backspace, BackspaceWord, ClearFormatting,
    ClearSelection, Copy, Cut, DecorationProviderId, DecreaseFontSize, Delete, DeleteWord, Enter,
    ImePreedit, IncreaseFontSize, Indent, MiddlewareId, MoveDocEnd, MoveDocStart, MoveDown,
    MoveLeft, MoveLineEnd, MoveLineStart, MoveRight, MoveUp, MoveWordLeft, MoveWordRight, Outdent,
    Paste, PastePlainText, RICH_DOCUMENT_KEY_CONTEXT, Redo, RichDocumentEvent, RichDocumentView,
    SelectAll, SelectDocEnd, SelectDocStart, SelectDown, SelectLeft, SelectLineEnd,
    SelectLineStart, SelectRight, SelectUp, SelectWordLeft, SelectWordRight, SetHeading1,
    SetHeading2, SetHeading3, SetParagraph, SoftBreak, ToggleBold, ToggleBulletList,
    ToggleChecklist, ToggleDebugOverlay, ToggleInlineCode, ToggleItalic, ToggleOrderedList,
    ToggleStrikethrough, ToggleSubscript, ToggleSuperscript, ToggleUnderline, Undo, ZoomIn,
    ZoomOut, ZoomReset, register_default_keybindings,
};
