//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

pub mod asset;
pub mod element;
pub mod extension;
pub mod shaper;
pub mod theme;
pub mod view;

pub use asset::{
    AssetProvider, FileAssetProvider, ImageAssetData, ImageAssetError, LoadedAsset,
    MemoryAssetProvider, decode_image_asset, infer_image_format,
};
pub use element::RichDocumentElement;
pub use extension::{
    BlockRenderContext, BlockRenderer, CustomRenderOutput, DirectPaintCallback, EditCommandContext,
    ExtensionDiagnostic, ExtensionDiagnosticKind, ExtensionNamespace, ExtensionNodeTarget,
    ExtensionRegistrationError, ExtensionRegistry, InlineRenderContext, InlineRenderer,
    ReadCommandContext, ReadCommandOutcome,
};
pub use shaper::{
    FontResolver, GpuiGlyphBackend, GpuiShaper, clear_font_resolution_cache, resolve_font_family,
    rgba_to_hsla, to_gpui_font,
};
pub use theme::{RichTheme, SmartChipColors, SmartChipPalette};
pub use view::{
    ActiveSmartChipPopover, ActiveTableContext, AlignCenter, AlignJustify, AlignLeft, AlignRight,
    Backspace, BackspaceWord, ClearFormatting, ClearSelection, Copy, Cut, DecorationProviderId,
    DecreaseFontSize, Delete, DeleteTableColumn, DeleteTableRow, DeleteWord, Enter,
    ImageResizeCorner, ImePreedit, IncreaseFontSize, Indent, InsertTableColumnAfter,
    InsertTableColumnBefore, InsertTableRowAbove, InsertTableRowBelow, MergeTableCells,
    MiddlewareId, MoveDocEnd, MoveDocStart, MoveDown, MoveLeft, MoveLineEnd, MoveLineStart,
    MoveRight, MoveUp, MoveWordLeft, MoveWordRight, Outdent, Paste, PastePlainText,
    RICH_DOCUMENT_KEY_CONTEXT, Redo, RichDocumentEvent, RichDocumentView, SelectAll, SelectDocEnd,
    SelectDocStart, SelectDown, SelectLeft, SelectLineEnd, SelectLineStart, SelectRight, SelectUp,
    SelectWordLeft, SelectWordRight, SelectedInlineImage, SetHeading1, SetHeading2, SetHeading3,
    SetParagraph, SoftBreak, SplitTableCell, ToggleBold, ToggleBulletList, ToggleChecklist,
    ToggleDebugOverlay, ToggleInlineCode, ToggleItalic, ToggleStrikethrough, ToggleSubscript,
    ToggleSuperscript, ToggleUnderline, Undo, ZoomIn, ZoomOut, ZoomReset,
    register_default_keybindings,
};
