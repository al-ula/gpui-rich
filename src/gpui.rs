//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

// Ensure `gpui-rich`'s `gpui-fast` feature stays synchronized with `gpui-kit`'s resolved backend.
const _: () = assert!(
    cfg!(feature = "gpui-fast") == gpui_kit::GPUI_FAST,
    "gpui-kit has the `gpui-fast` feature enabled, so `gpui-rich/gpui-fast` must also be enabled"
);

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
    QueryContext, ReadCommandContext, ReadCommandOutcome,
};
pub use shaper::{
    FontResolver, GpuiGlyphBackend, GpuiShaper, clear_font_resolution_cache, resolve_font_family,
    rgba_to_hsla, to_gpui_font,
};
pub use theme::{RichTheme, SmartChipColors, SmartChipPalette};
pub use view::{
    ActiveSearchState, ActiveSmartChipPopover, ActiveTableContext, AlignCenter, AlignJustify,
    AlignLeft, AlignRight, Backspace, BackspaceWord, CanExecute, CaretBounds, ClearFormatting,
    ClearSelection, Copy, Cut, DecorationProviderId, DecreaseFontSize, Delete, DeleteTableColumn,
    DeleteTableRow, DeleteWord, Enter, ExtensionDiagnostics, ImageResizeCorner, ImePreedit,
    IncreaseFontSize, Indent, InsertTableColumnAfter, InsertTableColumnBefore, InsertTableRowAbove,
    InsertTableRowBelow, MergeTableCells, MiddlewareId, MoveDocEnd, MoveDocStart, MoveDown,
    MoveLeft, MoveLineEnd, MoveLineStart, MoveRight, MoveUp, MoveWordLeft, MoveWordRight, Outdent,
    Paste, PastePlainText, RICH_DOCUMENT_KEY_CONTEXT, Redo, RichDocumentEvent, RichDocumentView,
    SelectAll, SelectDocEnd, SelectDocStart, SelectDown, SelectLeft, SelectLineEnd,
    SelectLineStart, SelectRight, SelectUp, SelectWordLeft, SelectWordRight, SelectedInlineImage,
    SelectionBounds, SelectionRects, SetHeading1, SetHeading2, SetHeading3, SetParagraph,
    SoftBreak, SplitTableCell, ToggleBold, ToggleBulletList, ToggleChecklist, ToggleDebugOverlay,
    ToggleInlineCode, ToggleItalic, ToggleStrikethrough, ToggleSubscript, ToggleSuperscript,
    ToggleUnderline, ToolbarSnapshot, ToolbarState, Undo, ViewQuery, ViewSnapshot, ViewState,
    ViewportBounds, ZoomIn, ZoomOut, ZoomReset, register_default_keybindings,
};
