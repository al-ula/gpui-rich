//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

// Ensure `gpui-rich`'s `gpui-fast` feature stays synchronized with `gpui-kit`'s resolved backend.
const _: () = assert!(
    cfg!(feature = "gpui-fast") == gpui_kit::GPUI_FAST,
    "gpui-kit has the `gpui-fast` feature enabled, so `gpui-rich/gpui-fast` must also be enabled"
);

pub mod asset;
pub mod completion;
pub mod element;
pub mod emoji;
pub mod extension;
pub mod mermaid;
pub mod overlay;
pub mod shaper;
pub mod theme;
pub mod view;

pub use asset::{
    AssetProvider, FileAssetProvider, ImageAssetData, ImageAssetError, LoadedAsset,
    MemoryAssetProvider, decode_image_asset, infer_image_format,
};
pub use completion::{
    CompletionAccept, CompletionAcceptContext, CompletionAcceptKeys, CompletionBlockScope,
    CompletionDetail, CompletionItem, CompletionMenuRenderer, CompletionMenuSnapshot,
    CompletionProvider, CompletionProviderId, CompletionQueryContext, CompletionQueryResult,
    CompletionSessionId, CompletionTrigger, CompletionTriggerKind, PrefixBoundary,
    ResolvedCompletionItem, fuzzy_match_label, inline_preview_suffix,
};
pub use element::RichDocumentElement;
pub use emoji::{
    EMOJI_PICKER_OVERLAY_ID, EmojiCategory, EmojiConfig, EmojiEntry, EmojiExtension,
    OpenEmojiPicker, default_emoji_catalog,
};
pub use extension::{
    BlockRenderContext, BlockRenderer, CustomRenderOutput, DirectPaintCallback, EditCommandContext,
    ExtensionDiagnostic, ExtensionDiagnosticKind, ExtensionNamespace, ExtensionNodeTarget,
    ExtensionPlugin, ExtensionRegistrationError, ExtensionRegistry, InlineRenderContext,
    InlineRenderer, QueryContext, ReadCommandContext, ReadCommandOutcome,
};
pub use mermaid::{MermaidConfig, MermaidExtension};
pub use overlay::{
    COMPLETION_MENU_OVERLAY_ID, OverlayAlign, OverlayAnchor, OverlayDismissPolicy, OverlayId,
    OverlayKeyAction, OverlayKeyHandler, OverlayKeyResult, OverlayOffscreenBehavior,
    OverlayPlacement, OverlayRenderContext, OverlayRenderFn, OverlaySide, OverlaySpec,
    OverlayZLayer, SMART_CHIP_POPOVER_OVERLAY_ID, is_anchor_visible_in_viewport,
    resolve_overlay_origin,
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
    ToggleUnderline, ToolbarSnapshot, ToolbarState, TriggerCompletion, Undo, ViewQuery,
    ViewSnapshot, ViewState, ViewportBounds, ZoomIn, ZoomOut, ZoomReset,
    register_default_keybindings,
};
