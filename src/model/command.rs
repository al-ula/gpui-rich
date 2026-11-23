//! Command model unifying viewer interactions, GPUI actions, editing operations, and typed
//! downstream extension commands.
//!
//! # Breaking changes and wire compatibility
//!
//! * Built-in [`RichCommand`] variants preserve their existing Serde wire format.
//! * The raw `RichCommand::Custom { id, payload, mutates_document }` variant has been removed.
//!   Old raw custom command JSON fails deserialization, and [`RichCommand::Extension`] invocations
//!   return a serialization error. Extension commands are process-local typed values; consumers
//!   that need a wire protocol decode their own DTO and construct a typed command via
//!   [`RichCommand::extension`].

use crate::model::block::{BlockId, ColumnSpec, Document, ListKind};
use crate::model::clipboard::ClipboardFragment;
use crate::model::inline::{InlineObject, InlineObjectId};
use crate::model::selection::{DocPos, DocumentSelection};
use crate::model::style::{
    BlockStyle, Edges, HorizontalLayout, InlineStyle, LinkTarget, NamedStyle, StyleId, TextAlign,
};
use crate::model::transaction::ChangeSet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::any::{Any, TypeId, type_name};
use std::fmt;
use std::sync::Arc;

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
    Paragraph,
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

/// Runtime effect classification for a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandEffectKind {
    /// Does not mutate document content or styles.
    ReadOnly,
    /// Mutates document content or styles through transactions.
    Edit,
}

mod command_effect_sealed {
    pub trait Sealed {}
}

/// Sealed trait classifying an [`ExtensionCommand`] as [`ReadOnly`] or [`Edit`].
pub trait CommandEffect: command_effect_sealed::Sealed + 'static {
    const KIND: CommandEffectKind;
}

/// Marker [`CommandEffect`] for commands that only inspect state and cannot return transactions.
pub struct ReadOnly;

/// Marker [`CommandEffect`] for commands that plan optional document [`crate::model::Transaction`]s.
pub struct Edit;

impl command_effect_sealed::Sealed for ReadOnly {}
impl command_effect_sealed::Sealed for Edit {}

impl CommandEffect for ReadOnly {
    const KIND: CommandEffectKind = CommandEffectKind::ReadOnly;
}

impl CommandEffect for Edit {
    const KIND: CommandEffectKind = CommandEffectKind::Edit;
}

/// Typed downstream extension command routed through the unified dispatcher, middleware, and policy.
///
/// One Rust type defines a command's stable namespaced [`Self::ID`], sealed [`Self::Effect`], and
/// in-process argument payload. Serde is not required for in-process command arguments.
pub trait ExtensionCommand: Any + Send + Sync + fmt::Debug + 'static {
    /// Stable dot-namespaced command identifier (for example `"acme.status.rename"`).
    const ID: &'static str;
    /// Sealed effect type ([`ReadOnly`] or [`Edit`]).
    type Effect: CommandEffect;
}

trait ErasedCommandArgs: Any + Send + Sync + fmt::Debug {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Send + Sync + fmt::Debug + 'static> ErasedCommandArgs for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Opaque, type-checked in-process invocation of an [`ExtensionCommand`].
///
/// Constructed exclusively via [`ExtensionInvocation::new`] or [`RichCommand::extension`].
#[derive(Clone)]
pub struct ExtensionInvocation {
    id: &'static str,
    effect: CommandEffectKind,
    type_id: TypeId,
    type_name: &'static str,
    args: Arc<dyn ErasedCommandArgs>,
}

impl ExtensionInvocation {
    /// Wraps a typed [`ExtensionCommand`] value in an opaque invocation.
    pub fn new<C: ExtensionCommand>(command: C) -> Self {
        Self {
            id: C::ID,
            effect: C::Effect::KIND,
            type_id: TypeId::of::<C>(),
            type_name: type_name::<C>(),
            args: Arc::new(command),
        }
    }

    /// Returns the command's stable identifier.
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// Returns the command's declared effect ([`CommandEffectKind::ReadOnly`] or [`CommandEffectKind::Edit`]).
    pub const fn effect(&self) -> CommandEffectKind {
        self.effect
    }

    /// Returns the concrete Rust [`TypeId`] of the wrapped command type.
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Returns the Rust type name of the wrapped command type for diagnostics.
    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Performs a checked borrow of the wrapped arguments as `C`.
    pub fn args<C: ExtensionCommand>(&self) -> Option<&C> {
        if self.id != C::ID
            || self.effect != C::Effect::KIND
            || self.type_id != TypeId::of::<C>()
        {
            return None;
        }
        (*self.args).as_any().downcast_ref::<C>()
    }
}

impl fmt::Debug for ExtensionInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionInvocation")
            .field("id", &self.id)
            .field("effect", &self.effect)
            .field("type_name", &self.type_name)
            .field("args", &self.args)
            .finish()
    }
}

impl PartialEq for ExtensionInvocation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.effect == other.effect
            && self.type_id == other.type_id
            && Arc::ptr_eq(&self.args, &other.args)
    }
}

impl Serialize for ExtensionInvocation {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom(format!(
            "typed extension command invocation '{}' cannot be serialized",
            self.id
        )))
    }
}

impl<'de> Deserialize<'de> for ExtensionInvocation {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "extension command invocations cannot be deserialized directly",
        ))
    }
}

/// Resolution state of a [`RichCommand`] against the active extension registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandResolution {
    /// Built-in [`RichCommand`] variant handled directly by the editor.
    BuiltIn,
    /// Extension command matching a registered handler and its concrete Rust type.
    RegisteredExtension,
    /// Extension command whose identifier is not registered on the view.
    UnregisteredExtension,
    /// Extension command whose identifier is registered for a different Rust type or effect.
    MismatchedExtension,
}

/// Resolved metadata describing a command prior to middleware, policy, and completion events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResolvedCommandMeta {
    pub id: &'static str,
    pub effect: CommandEffectKind,
    pub resolution: CommandResolution,
}

/// Read-only view of a resolved [`RichCommand`] passed to [`CommandMiddleware::before_command`].
pub struct ResolvedCommand<'a> {
    command: &'a RichCommand,
    meta: ResolvedCommandMeta,
}

impl<'a> ResolvedCommand<'a> {
    /// Constructs a resolved command wrapper from `command` and its lookup `resolution`.
    pub fn new(command: &'a RichCommand, resolution: CommandResolution) -> Self {
        let meta = ResolvedCommandMeta {
            id: command.command_id(),
            effect: command.effect(),
            resolution,
        };
        Self { command, meta }
    }

    /// Borrows the underlying [`RichCommand`].
    pub fn command(&self) -> &'a RichCommand {
        self.command
    }

    /// Returns the resolved command identifier.
    pub const fn id(&self) -> &'static str {
        self.meta.id
    }

    /// Returns the resolved command effect.
    pub const fn effect(&self) -> CommandEffectKind {
        self.meta.effect
    }

    /// Returns how this command resolved against the view's command registry.
    pub const fn resolution(&self) -> CommandResolution {
        self.meta.resolution
    }

    /// Returns the full resolved command metadata.
    pub const fn meta(&self) -> ResolvedCommandMeta {
        self.meta
    }

    /// Returns `true` when this command is an edit command.
    pub const fn is_document_mutation(&self) -> bool {
        matches!(self.meta.effect, CommandEffectKind::Edit)
    }

    /// Performs a checked borrow of typed extension arguments as `C`.
    pub fn args<C: ExtensionCommand>(&self) -> Option<&'a C> {
        self.command.extension_args::<C>()
    }
}

/// Unified command representation for toolbar controls, keybindings, command palettes,
/// mouse interactions, editing mutations, and typed downstream extension commands.
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
    CopyCodeBlock(BlockId),
    OpenLink(String),
    ToggleHeadingCollapsed(BlockId),

    // History & Phase 2/3 document editing commands
    Undo,
    Redo,
    InsertText(String),
    InsertInlineObject(InlineObject),
    ToggleChecklistItem(BlockId),
    ResizeInlineImage {
        block_id: BlockId,
        object_id: InlineObjectId,
        width_pt: f32,
        height_pt: f32,
    },
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
    /// Same-process structured rich clipboard paste preserving custom node shapes and payloads.
    PasteFragment {
        fragment: ClipboardFragment,
        html_fallback: Option<String>,
        plain_text: Option<String>,
    },
    PasteText(String),

    // Phase 3 interactive table commands
    SetTableColumns {
        table_id: BlockId,
        columns: Vec<ColumnSpec>,
    },
    InsertTableRow {
        after: bool,
    },
    DeleteTableRow,
    InsertTableColumn {
        after: bool,
    },
    DeleteTableColumn,
    MergeTableCells,
    SplitTableCell,
    NavigateTableCell {
        forward: bool,
    },

    Navigation(crate::model::navigation::NavigationCommand),
    Grid(crate::model::edit::table::GridCommand),
    Component(crate::model::components::ComponentCommand),
    Structure(crate::model::edit::structure::StructureCommand),
    Anchor(crate::model::anchors::AnchorCommand),

    /// Opaque, type-checked downstream extension command routed through the unified dispatcher.
    Extension(ExtensionInvocation),
}

impl RichCommand {
    /// Wraps a typed [`ExtensionCommand`] inside the unified [`RichCommand`] pipeline.
    pub fn extension<C: ExtensionCommand>(command: C) -> Self {
        Self::Extension(ExtensionInvocation::new(command))
    }

    /// Performs a checked borrow of typed extension arguments when this command wraps `C`.
    pub fn extension_args<C: ExtensionCommand>(&self) -> Option<&C> {
        match self {
            Self::Extension(invocation) => invocation.args::<C>(),
            _ => None,
        }
    }

    /// Returns the effect classification of this command.
    pub fn effect(&self) -> CommandEffectKind {
        if self.is_document_mutation() {
            CommandEffectKind::Edit
        } else {
            CommandEffectKind::ReadOnly
        }
    }

    /// Returns true when the command mutates document content or document styles.
    pub fn is_document_mutation(&self) -> bool {
        match self {
            Self::Undo
            | Self::Redo
            | Self::InsertText(_)
            | Self::InsertInlineObject(_)
            | Self::ToggleChecklistItem(_)
            | Self::ResizeInlineImage { .. }
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
            | Self::PasteHtml { .. }
            | Self::PasteFragment { .. }
            | Self::PasteText(_)
            | Self::SetTableColumns { .. }
            | Self::InsertTableRow { .. }
            | Self::DeleteTableRow
            | Self::InsertTableColumn { .. }
            | Self::DeleteTableColumn
            | Self::MergeTableCells
            | Self::SplitTableCell => true,
            Self::Navigation(command) => command.is_document_mutation(),
            Self::Grid(command) => command.is_document_mutation(),
            Self::Component(command) => command.is_document_mutation(),
            Self::Structure(command) => command.is_document_mutation(),
            Self::Anchor(command) => command.is_document_mutation(),
            Self::Extension(invocation) => invocation.effect() == CommandEffectKind::Edit,
            _ => false,
        }
    }

    /// Returns a stable dot-namespaced identifier for keymaps, palettes, middleware, and logging.
    pub fn command_id(&self) -> &'static str {
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
            Self::CopyCodeBlock(_) => "rich.clipboard.copy_code_block",
            Self::OpenLink(_) => "rich.navigation.open_link",
            Self::ToggleHeadingCollapsed(_) => "rich.view.toggle_heading_collapsed",
            Self::Undo => "rich.history.undo",
            Self::Redo => "rich.history.redo",
            Self::InsertText(_) => "rich.edit.insert_text",
            Self::InsertInlineObject(_) => "rich.edit.insert_inline_object",
            Self::ToggleChecklistItem(_) => "rich.edit.toggle_checklist_item",
            Self::ResizeInlineImage { .. } => "rich.edit.resize_inline_image",
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
            Self::PasteFragment { .. } => "rich.clipboard.paste_fragment",
            Self::PasteText(_) => "rich.clipboard.paste_text",
            Self::SetTableColumns { .. } => "rich.table.set_columns",
            Self::InsertTableRow { .. } => "rich.table.insert_row",
            Self::DeleteTableRow => "rich.table.delete_row",
            Self::InsertTableColumn { .. } => "rich.table.insert_column",
            Self::DeleteTableColumn => "rich.table.delete_column",
            Self::MergeTableCells => "rich.table.merge_cells",
            Self::SplitTableCell => "rich.table.split_cell",
            Self::NavigateTableCell { .. } => "rich.table.navigate_cell",
            Self::Navigation(command) => command.command_id(),
            Self::Grid(command) => command.command_id(),
            Self::Component(command) => command.command_id(),
            Self::Structure(command) => command.command_id(),
            Self::Anchor(command) => command.command_id(),
            Self::Extension(invocation) => invocation.id(),
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

/// Reader interactions. Turning off `enabled` masks every child without changing its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReadingFeatures {
    pub enabled: bool,
    pub selection: bool,
    pub links: bool,
    pub heading_collapse: bool,
    pub selection_copy: bool,
    pub code_copy: bool,
    pub smart_chips: bool,
    pub read_only_caret: bool,
}

impl ReadingFeatures {
    const DEFAULT: Self = Self {
        enabled: true,
        selection: true,
        links: true,
        heading_collapse: true,
        selection_copy: true,
        code_copy: true,
        smart_chips: true,
        read_only_caret: false,
    };
}

/// Formatting commands. `enabled` masks all formatting children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormattingFeatures {
    pub enabled: bool,
    /// Bold, italic, strikethrough, and inline code.
    pub inline_marks: bool,
    /// Arbitrary styles, alignment, font size, underline, superscript, and subscript.
    pub rich_styles: bool,
    pub headings: bool,
    pub lists: bool,
    pub links: bool,
    pub clear: bool,
}

impl FormattingFeatures {
    const DEFAULT: Self = Self {
        enabled: true,
        inline_marks: true,
        rich_styles: true,
        headings: true,
        lists: true,
        links: true,
        clear: true,
    };
}

/// Table authoring controls. Turning these off does not hide tables or prevent ordinary cell typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableEditingFeatures {
    pub enabled: bool,
    pub column_resize: bool,
    /// Row and column operations, cell merges and splits, and structural grid commands.
    pub structure: bool,
    /// Clearing or pasting into a rectangular grid selection, and TSV or table paste into a cell.
    pub grid_editing: bool,
}

impl TableEditingFeatures {
    const DEFAULT: Self = Self {
        enabled: true,
        column_resize: true,
        structure: true,
        grid_editing: true,
    };
}

/// Editor interactions. Disabling `enabled` makes user commands read-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditingFeatures {
    pub enabled: bool,
    pub formatting: FormattingFeatures,
    pub tables: TableEditingFeatures,
    pub history: bool,
    /// Automatic list markers while typing. Also requires list formatting.
    pub input_rules: bool,
    pub plain_paste: bool,
    pub rich_paste: bool,
    pub inline_objects: bool,
    pub image_resize: bool,
    pub checklist_toggle: bool,
    pub components: bool,
    pub anchors: bool,
    pub find_replace: bool,
}

impl EditingFeatures {
    const DEFAULT: Self = Self {
        enabled: false,
        formatting: FormattingFeatures::DEFAULT,
        tables: TableEditingFeatures::DEFAULT,
        history: true,
        input_rules: false,
        plain_paste: true,
        rich_paste: true,
        inline_objects: true,
        image_resize: true,
        checklist_toggle: true,
        components: true,
        anchors: true,
        find_replace: true,
    };
}

/// Per-view feature configuration. Switches govern user interactions, not document content.
///
/// Disabling a parent masks its children; re-enabling it restores their configured values.
/// [`CommandSource::Programmatic`] bypasses the user gates, so this is not a permission boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionPolicy {
    pub reading: ReadingFeatures,
    pub editing: EditingFeatures,
}

impl Default for InteractionPolicy {
    fn default() -> Self {
        Self::viewer()
    }
}

impl InteractionPolicy {
    /// Full editor preset. Automatic list markers remain opt-in.
    pub const fn editable() -> Self {
        Self {
            editing: EditingFeatures {
                enabled: true,
                ..EditingFeatures::DEFAULT
            },
            ..Self::viewer()
        }
    }

    /// Read-only viewer with reader interactions enabled.
    pub const fn viewer() -> Self {
        Self {
            reading: ReadingFeatures::DEFAULT,
            editing: EditingFeatures::DEFAULT,
        }
    }

    /// Preview with reader and editor interactions disabled. Scrolling and zoom are still active.
    pub const fn static_preview() -> Self {
        Self {
            reading: ReadingFeatures {
                enabled: false,
                ..ReadingFeatures::DEFAULT
            },
            ..Self::viewer()
        }
    }

    pub const fn can_select(&self) -> bool {
        self.reading.enabled && self.reading.selection
    }

    pub const fn can_open_links(&self) -> bool {
        self.reading.enabled && self.reading.links
    }

    pub const fn can_collapse_headings(&self) -> bool {
        self.reading.enabled && self.reading.heading_collapse
    }

    pub const fn can_copy_code(&self) -> bool {
        self.reading.enabled && self.reading.code_copy
    }

    pub const fn can_activate_smart_chips(&self) -> bool {
        self.reading.enabled && self.reading.smart_chips
    }

    pub const fn can_toggle_checklists(&self) -> bool {
        self.editing.enabled && self.editing.checklist_toggle
    }

    pub const fn can_resize_images(&self) -> bool {
        self.editing.enabled && self.editing.image_resize
    }

    pub const fn can_resize_table_columns(&self) -> bool {
        self.editing.enabled && self.editing.tables.enabled && self.editing.tables.column_resize
    }

    pub const fn can_edit_table_structure(&self) -> bool {
        self.editing.enabled && self.editing.tables.enabled && self.editing.tables.structure
    }

    pub const fn can_edit_grid(&self) -> bool {
        self.editing.enabled && self.editing.tables.enabled && self.editing.tables.grid_editing
    }

    pub const fn can_paste_rich_text(&self) -> bool {
        self.editing.enabled && self.editing.rich_paste
    }

    pub const fn can_format_lists(&self) -> bool {
        self.editing.enabled && self.editing.formatting.enabled && self.editing.formatting.lists
    }

    pub const fn can_apply_rich_styles(&self) -> bool {
        self.editing.enabled
            && self.editing.formatting.enabled
            && self.editing.formatting.rich_styles
    }

    pub const fn can_apply_input_rules(&self) -> bool {
        self.editing.input_rules && self.can_format_lists()
    }

    /// Returns true when a collapsed caret should be visible.
    pub const fn should_show_caret(&self) -> bool {
        self.editing.enabled || (self.reading.enabled && self.reading.read_only_caret)
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
        if command.is_document_mutation() && !self.editing.enabled {
            return Some(CommandOutcome::RejectedReadOnly);
        }
        let formatting = &self.editing.formatting;
        let permitted = match command {
            RichCommand::SetSelection(Some(_))
            | RichCommand::SelectAll
            | RichCommand::SelectWordAt(_)
            | RichCommand::MoveCaret { .. }
            | RichCommand::NavigateTableCell { .. } => self.can_select(),
            RichCommand::CopySelection => self.reading.enabled && self.reading.selection_copy,
            RichCommand::CopyCodeBlock(_) => self.can_copy_code(),
            RichCommand::OpenLink(_) => self.can_open_links(),
            RichCommand::ToggleHeadingCollapsed(_) => self.can_collapse_headings(),
            RichCommand::Undo | RichCommand::Redo => self.editing.history,
            RichCommand::InsertInlineObject(_) => self.editing.inline_objects,
            RichCommand::ToggleChecklistItem(_) => self.can_toggle_checklists(),
            RichCommand::ResizeInlineImage { .. } => self.can_resize_images(),
            RichCommand::ToggleMark(
                MarkKind::Bold | MarkKind::Italic | MarkKind::Strikethrough | MarkKind::InlineCode,
            ) => formatting.enabled && formatting.inline_marks,
            RichCommand::ToggleMark(_)
            | RichCommand::SetInlineStyle(_)
            | RichCommand::AdjustFontSize(_)
            | RichCommand::SetBlockStyle(_)
            | RichCommand::UpdateNamedStyle { .. }
            | RichCommand::SetTextAlign(_) => self.can_apply_rich_styles(),
            RichCommand::ClearFormatting => formatting.enabled && formatting.clear,
            RichCommand::SetLink(_) => formatting.enabled && formatting.links,
            RichCommand::SetHeading(_) => formatting.enabled && formatting.headings,
            RichCommand::ToggleList(_)
            | RichCommand::Structure(
                crate::model::StructureCommand::RestartOrderedList { .. }
                | crate::model::StructureCommand::ContinueOrderedList { .. },
            ) => self.can_format_lists(),
            RichCommand::PasteHtml { .. } | RichCommand::PasteFragment { .. } => {
                self.can_paste_rich_text()
            }
            RichCommand::PasteText(_) => self.editing.plain_paste,
            RichCommand::SetTableColumns { .. } => self.can_resize_table_columns(),
            RichCommand::InsertTableRow { .. }
            | RichCommand::DeleteTableRow
            | RichCommand::InsertTableColumn { .. }
            | RichCommand::DeleteTableColumn
            | RichCommand::MergeTableCells
            | RichCommand::SplitTableCell => self.can_edit_table_structure(),
            RichCommand::Grid(crate::model::GridCommand::ClearSelection) => self.can_edit_grid(),
            RichCommand::Grid(crate::model::GridCommand::PasteHtml { .. }) => {
                self.can_edit_grid() && self.can_paste_rich_text()
            }
            RichCommand::Grid(crate::model::GridCommand::PasteTsv { .. }) => {
                self.can_edit_grid() && self.editing.plain_paste
            }
            RichCommand::Grid(_) => self.can_edit_table_structure(),
            RichCommand::Component(_) => self.editing.components,
            RichCommand::Navigation(_) if command.is_document_mutation() => {
                self.editing.find_replace
            }
            RichCommand::Anchor(_) if command.is_document_mutation() => self.editing.anchors,
            RichCommand::Navigation(_) | RichCommand::Anchor(_) => self.can_select(),
            _ => true,
        };
        (!permitted).then_some(CommandOutcome::RejectedByPolicy)
    }
}

/// Decision returned by [`CommandMiddleware::before_command`].
#[derive(Clone, Debug, PartialEq)]
pub enum CommandDecision {
    /// Proceed with the current command.
    Continue,
    /// Abort command execution and report [`CommandOutcome::Cancelled`].
    Cancel,
    /// Substitute a different command for remaining middleware, policy, and execution.
    Replace(RichCommand),
}

/// Extension hook intercepting commands before execution and observing applied transactions.
pub trait CommandMiddleware {
    /// Inspects, cancels, or replaces a resolved command prior to policy enforcement and execution.
    fn before_command(
        &mut self,
        _command: &ResolvedCommand<'_>,
        _source: CommandSource,
        _document: &Document,
        _selection: Option<&DocumentSelection>,
    ) -> CommandDecision {
        CommandDecision::Continue
    }

    /// Invoked after a [`crate::model::Transaction`] mutates the [`Document`].
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
    /// Dispatched an extension command whose identifier is not registered on the view.
    UnregisteredCommand,
    /// Dispatched an extension command whose identifier is registered for another Rust type or effect.
    MismatchedCommandType,
    TargetHidden,
}
