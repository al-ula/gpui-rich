//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

pub mod element;
pub mod shaper;
pub mod theme;
pub mod view;

pub use element::RichDocumentElement;
pub use shaper::{GpuiGlyphBackend, GpuiShaper, rgba_to_hsla, to_gpui_font};
pub use theme::{RichTheme, SmartChipColors, SmartChipPalette};
pub use view::{
    Backspace, BackspaceWord, ClearSelection, Copy, Cut, Delete, DeleteWord, Enter, ImePreedit,
    Indent, MiddlewareId, MoveDocEnd, MoveDocStart, MoveDown, MoveLeft, MoveLineEnd, MoveLineStart,
    MoveRight, MoveUp, MoveWordLeft, MoveWordRight, Outdent, Paste, RICH_DOCUMENT_KEY_CONTEXT,
    Redo, RichDocumentEvent, RichDocumentView, SelectAll, SelectDocEnd, SelectDocStart, SelectDown,
    SelectLeft, SelectLineEnd, SelectLineStart, SelectRight, SelectUp, SelectWordLeft,
    SelectWordRight, SoftBreak, ToggleDebugOverlay, Undo, ZoomIn, ZoomOut, ZoomReset,
    register_default_keybindings,
};
