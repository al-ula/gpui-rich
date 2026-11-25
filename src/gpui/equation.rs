//! Built-in GPUI LaTeX equation extension implementing [`ExtensionPlugin`], [`BlockRenderer`]
//! (for [`EquationBlockSpec`]), and [`InlineRenderer`] (for [`InlineEquationSpec`]) with two-tier
//! async SVG compilation and rasterization, stale-while-revalidate zooming, exact TeX baseline
//! depth alignment, in-place block editing, and Layer A anchored popover editing for inline math.

mod editor;

use self::editor::{EquationBlockSourceEditor, InlineEquationPopoverEditor};
use crate::gpui::completion::{
    CompletionAccept, CompletionBlockScope, CompletionDetail, CompletionItem, CompletionProvider,
    CompletionQueryContext, CompletionQueryResult, CompletionTrigger, fuzzy_match_label,
};
use crate::gpui::extension::{
    BlockRenderContext, BlockRenderer, CustomRenderOutput, ExtensionPlugin,
    ExtensionRegistrationError, InlineMeasureContext, InlineRenderContext, InlineRenderer,
    ReadCommandOutcome,
};
use crate::gpui::overlay::{
    OverlayAnchor, OverlayDismissPolicy, OverlayId, OverlayPlacement, OverlaySpec, OverlayZLayer,
};
use crate::gpui::shaper::rgba_to_hsla;
use crate::gpui::theme::RichTheme;
use crate::gpui::view::{RichDocumentEvent, RichDocumentView};
use crate::layout::{InlineBoxMetrics, Size, pt_to_px};
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, CloseInlineEquationEditor, CommandOutcome,
    DocumentSelection, EquationBackend, EquationBlockData, EquationBlockSpec, EquationBlockStatus,
    EquationBlockStatusQuery, EquationDisplayMode, EquationRenderError, EquationRenderOptions,
    EquationRenderStatus, EquationViewMode, ExtensionCommand, HistoryMergeGroup, InlineContent,
    InlineEquationData, InlineEquationSpec, InlineObject, InlineObjectId, InlineStyle,
    InsertEquationBlock, InsertInlineEquation, InteractionPolicy, OpenInlineEquationEditor,
    RgbaColor, RichCommand, SetEquationNumbering, SetEquationViewMode, Step,
    ToggleEquationViewMode, Transaction, UpdateEquationBlock, UpdateInlineEquation,
    hash_equation_source,
};
use gpui_kit::{
    AnyElement, App, AppContext, AssetSource, AsyncApp, ClipboardItem, Context, Corners,
    DevicePixels, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, ParsedSvg, RenderImage, Rgba, Size as GpuiSize, Styled, SvgRenderer, SvgSize,
    Task, WeakEntity, Window, canvas, div, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Layer A overlay identifier for the anchored inline equation popover editor.
pub const INLINE_EQUATION_EDITOR_OVERLAY_ID: &str = "gpui_rich.equation.inline_editor";

const DEFAULT_BLOCK_FONT_SIZE_PT: f32 = 13.5;
const DEFAULT_CACHE_CAPACITY: usize = 128;
const BLOCK_HEADER_HEIGHT_PT: f32 = 24.0;
const BLOCK_PAD_X_PT: f32 = 12.0;
const BLOCK_PAD_Y_PT: f32 = 10.0;
const BLOCK_NUMBERING_GUTTER_PT: f32 = 44.0;
const BLOCK_PLACEHOLDER_HEIGHT_PT: f32 = 42.0;
const BLOCK_ERROR_HEIGHT_PT: f32 = 46.0;
const BLOCK_EDITOR_LINE_HEIGHT_PT: f32 = 18.0;
const BLOCK_EDITOR_VERT_PAD_PT: f32 = 14.0;
const INLINE_PAD_X_PT: f32 = 2.0;
const COPY_FEEDBACK_DURATION: Duration = Duration::from_millis(1400);
const LIVE_COMPILE_DEBOUNCE: Duration = Duration::from_millis(90);

/// Configuration options for [`EquationExtension`].
#[derive(Clone)]
pub struct EquationConfig {
    /// Optional host-supplied [`AssetSource`] used by GPUI's [`SvgRenderer`].
    ///
    /// When `None`, the extension uses `cx.svg_renderer()`.
    pub asset_source: Option<Arc<dyn AssetSource>>,
    /// Default base font size (in typographic points) for display equation blocks when
    /// [`EquationBlockData::font_size_pt`] is `None`.
    pub default_block_font_size_pt: f32,
    /// Maximum number of entries retained in each tier of the LRU compile and raster caches.
    pub cache_capacity: usize,
    /// Registers built-in `/equation` and `/math` slash completion items when installing.
    pub register_slash_completion: bool,
}

impl Default for EquationConfig {
    fn default() -> Self {
        Self {
            asset_source: None,
            default_block_font_size_pt: DEFAULT_BLOCK_FONT_SIZE_PT,
            cache_capacity: DEFAULT_CACHE_CAPACITY,
            register_slash_completion: true,
        }
    }
}

impl EquationConfig {
    /// Creates an [`EquationConfig`] with default sizing and cache capacity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the [`AssetSource`] used to construct GPUI's [`SvgRenderer`].
    pub fn with_asset_source(mut self, asset_source: Arc<dyn AssetSource>) -> Self {
        self.asset_source = Some(asset_source);
        self
    }

    /// Sets the default display equation base font size in typographic points.
    pub fn with_default_block_font_size_pt(mut self, font_size_pt: f32) -> Self {
        self.default_block_font_size_pt = font_size_pt.clamp(8.0, 48.0);
        self
    }

    /// Sets the per-tier LRU cache capacity.
    pub fn with_cache_capacity(mut self, cache_capacity: usize) -> Self {
        self.cache_capacity = cache_capacity.max(8);
        self
    }

    /// Enables or disables automatic `/equation` and `/math` slash completion registration.
    pub const fn with_slash_completion(mut self, enabled: bool) -> Self {
        self.register_slash_completion = enabled;
        self
    }
}

/// Built-in LaTeX equation [`ExtensionPlugin`] registering both [`EquationBlockSpec`] and
/// [`InlineEquationSpec`] renderers, editing commands, anchored inline popover editing, status
/// queries, and slash completions onto a [`RichDocumentView`].
pub struct EquationExtension {
    backend: Arc<dyn EquationBackend>,
    config: EquationConfig,
}

impl EquationExtension {
    /// Creates a built-in [`EquationExtension`] using [`crate::model::BuiltInEquationBackend`].
    #[cfg(feature = "equation")]
    pub fn builtin() -> Self {
        Self::with_backend(crate::model::BuiltInEquationBackend::new())
    }

    /// Creates an [`EquationExtension`] backed by a custom [`EquationBackend`].
    pub fn with_backend(backend: impl EquationBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            config: EquationConfig::default(),
        }
    }

    /// Creates an [`EquationExtension`] from a shared [`EquationBackend`] trait object.
    pub fn with_shared_backend(backend: Arc<dyn EquationBackend>) -> Self {
        Self {
            backend,
            config: EquationConfig::default(),
        }
    }

    /// Applies custom [`EquationConfig`] options.
    pub fn with_config(mut self, config: EquationConfig) -> Self {
        self.config = config;
        self
    }
}

impl ExtensionPlugin for EquationExtension {
    fn install(
        self,
        view: &mut RichDocumentView,
        cx: &mut Context<RichDocumentView>,
    ) -> Result<(), ExtensionRegistrationError> {
        let svg_renderer = match &self.config.asset_source {
            Some(asset_source) => SvgRenderer::new(Arc::clone(asset_source)),
            None => cx.svg_renderer(),
        };
        let register_slash = self.config.register_slash_completion;
        let state = Rc::new(RefCell::new(EquationRuntimeState::new(
            self.backend,
            svg_renderer,
            self.config,
            cx.entity().downgrade(),
        )));

        view.register_block_renderer(
            EquationBlockRenderer {
                state: Rc::clone(&state),
            },
            cx,
        )?;

        view.register_inline_renderer(
            InlineEquationRenderer {
                state: Rc::clone(&state),
            },
            cx,
        )?;

        view.register_edit_command::<InsertEquationBlock>(|command, context| {
            let mut data = EquationBlockData::new(command.source.clone());
            data.numbering = command.numbering.clone();
            data.label = command.label.clone();
            let block_id = context.reserve_block_id();
            let block = BlockNode::new(
                block_id,
                BlockKind::leaf::<EquationBlockSpec>(data),
                BlockStyle {
                    space_before_pt: Some(8.0),
                    space_after_pt: Some(10.0),
                    ..BlockStyle::default()
                },
            );
            let insert_index = context
                .selection()
                .and_then(|selection| match selection {
                    DocumentSelection::Text { focus, .. } => focus.top_level_block_id(),
                    DocumentSelection::TableCells { table_id, .. } => Some(*table_id),
                })
                .and_then(|active_id| {
                    context
                        .document()
                        .blocks
                        .iter()
                        .position(|candidate| candidate.id == active_id)
                })
                .map(|position| position + 1)
                .unwrap_or_else(|| context.document().blocks.len());
            Some(Transaction::new(vec![Step::InsertBlock {
                index: insert_index,
                block,
            }]))
        })?;

        view.register_edit_command::<UpdateEquationBlock>(|command, context| {
            let block = context.document().find_block(command.block_id)?;
            let mut data = block
                .kind
                .read_data::<EquationBlockSpec>()
                .ok()?
                .into_owned();
            if data.source == command.source {
                return None;
            }
            data.source = command.source.clone();
            context.set_history_group(HistoryMergeGroup::Typing {
                block_id: command.block_id,
            });
            Some(Transaction::new(vec![Step::update_block_data::<
                EquationBlockSpec,
            >(command.block_id, data)]))
        })?;

        view.register_edit_command::<SetEquationNumbering>(|command, context| {
            let block = context.document().find_block(command.block_id)?;
            let mut data = block
                .kind
                .read_data::<EquationBlockSpec>()
                .ok()?
                .into_owned();
            if data.numbering == command.numbering {
                return None;
            }
            data.numbering = command.numbering.clone();
            Some(Transaction::new(vec![Step::update_block_data::<
                EquationBlockSpec,
            >(command.block_id, data)]))
        })?;

        view.register_edit_command::<InsertInlineEquation>(|command, context| {
            let DocumentSelection::Text { anchor, focus, .. } = context.selection()? else {
                return None;
            };
            let block_id = focus.leaf_block_id()?;
            let block = context.document().find_block(block_id)?;
            let inline = block.kind.inline_content()?;
            let mut steps = Vec::new();
            let insert_offset = if anchor.leaf_block_id() == Some(block_id) {
                let start = anchor.offset.min(focus.offset).min(inline.len());
                let end = anchor.offset.max(focus.offset).min(inline.len());
                if end > start {
                    steps.push(Step::DeleteRange {
                        block_id,
                        range: start..end,
                    });
                }
                start
            } else {
                focus.offset.min(inline.len())
            };
            let inherited_style: InlineStyle = inline.style_at_caret(insert_offset);
            let mut content = InlineContent::empty();
            content.push_object(
                InlineObject::custom::<InlineEquationSpec>(InlineEquationData::new(
                    command.source.clone(),
                )),
                inherited_style,
            );
            steps.push(Step::InsertContent {
                block_id,
                offset: insert_offset,
                content,
            });
            Some(Transaction::new(steps))
        })?;

        view.register_edit_command::<UpdateInlineEquation>(|command, context| {
            let block = context.document().find_block(command.block_id)?;
            let inline = block.kind.inline_content()?;
            let entry = inline.object_by_id(command.object_id)?;
            let mut data = entry
                .object
                .read_data::<InlineEquationSpec>()
                .ok()?
                .into_owned();
            if data.source == command.source {
                return None;
            }
            data.source = command.source.clone();
            context.set_history_group(HistoryMergeGroup::Typing {
                block_id: command.block_id,
            });
            Some(Transaction::new(vec![Step::update_inline_data::<
                InlineEquationSpec,
            >(
                command.block_id,
                command.object_id,
                data,
            )]))
        })?;

        let set_mode_state = Rc::clone(&state);
        view.register_read_command::<SetEquationViewMode>(move |command, context| {
            let Some(block) = context.document().find_block(command.block_id) else {
                return ReadCommandOutcome::NoOp;
            };
            if block.kind.read_data::<EquationBlockSpec>().is_err() {
                return ReadCommandOutcome::NoOp;
            }
            let mut runtime = set_mode_state.borrow_mut();
            if runtime.view_mode(command.block_id) == command.mode {
                return ReadCommandOutcome::NoOp;
            }
            runtime.set_view_mode(command.block_id, command.mode);
            context.invalidate_block_layout(command.block_id);
            ReadCommandOutcome::Executed
        })?;

        let toggle_mode_state = Rc::clone(&state);
        view.register_read_command::<ToggleEquationViewMode>(move |command, context| {
            let Some(block) = context.document().find_block(command.block_id) else {
                return ReadCommandOutcome::NoOp;
            };
            if block.kind.read_data::<EquationBlockSpec>().is_err() {
                return ReadCommandOutcome::NoOp;
            }
            let mut runtime = toggle_mode_state.borrow_mut();
            let next = match runtime.view_mode(command.block_id) {
                EquationViewMode::Rendered => EquationViewMode::Edit,
                EquationViewMode::Edit => EquationViewMode::Rendered,
            };
            runtime.set_view_mode(command.block_id, next);
            context.invalidate_block_layout(command.block_id);
            ReadCommandOutcome::Executed
        })?;

        let open_inline_state = Rc::clone(&state);
        view.register_read_command::<OpenInlineEquationEditor>(move |command, context| {
            let Some(block) = context.document().find_block(command.block_id) else {
                return ReadCommandOutcome::NoOp;
            };
            let Some(inline) = block.kind.inline_content() else {
                return ReadCommandOutcome::NoOp;
            };
            let Some(entry) = inline.object_by_id(command.object_id) else {
                return ReadCommandOutcome::NoOp;
            };
            if entry.object.read_data::<InlineEquationSpec>().is_err() {
                return ReadCommandOutcome::NoOp;
            }
            open_inline_state.borrow_mut().pending_inline_editor =
                Some((command.block_id, command.object_id));
            ReadCommandOutcome::Executed
        })?;

        let close_inline_state = Rc::clone(&state);
        view.register_read_command::<CloseInlineEquationEditor>(move |_command, _context| {
            let mut runtime = close_inline_state.borrow_mut();
            let had_open =
                runtime.active_inline_editor.is_some() || runtime.pending_inline_editor.is_some();
            runtime.active_inline_editor = None;
            runtime.pending_inline_editor = None;
            if had_open {
                ReadCommandOutcome::Executed
            } else {
                ReadCommandOutcome::NoOp
            }
        })?;

        let overlay_state = Rc::clone(&state);
        cx.subscribe_self(move |this, event: &RichDocumentEvent, cx| {
            if let RichDocumentEvent::CommandExecuted {
                command_id,
                outcome,
                ..
            } = event
            {
                if *command_id == OpenInlineEquationEditor::ID
                    && *outcome == CommandOutcome::Executed
                    && this.policy().editing.enabled
                {
                    let target = overlay_state.borrow_mut().pending_inline_editor.take();
                    if let Some((block_id, object_id)) = target {
                        let spec = build_inline_equation_overlay_spec(
                            block_id,
                            object_id,
                            Rc::clone(&overlay_state),
                        );
                        this.set_overlay(spec, cx);
                    }
                } else if *command_id == CloseInlineEquationEditor::ID {
                    overlay_state.borrow_mut().active_inline_editor = None;
                    this.remove_overlay(&OverlayId::from(INLINE_EQUATION_EDITOR_OVERLAY_ID), cx);
                }
            }
        })
        .detach();

        let query_state = Rc::clone(&state);
        view.register_query::<EquationBlockStatusQuery>(move |query, context| {
            let block = context.document().find_block(query.block_id)?;
            let data = block.kind.read_data::<EquationBlockSpec>().ok()?;
            let runtime = query_state.borrow();
            Some(runtime.block_status(query.block_id, &data))
        })?;

        if register_slash {
            view.register_completion_provider(EquationSlashCompletionProvider::new())?;
        }

        Ok(())
    }
}

impl RichDocumentView {
    /// Installs the built-in RaTeX equation extension with default configuration.
    #[cfg(feature = "equation")]
    pub fn install_equation(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(EquationExtension::builtin(), cx)
    }

    /// Installs a configured [`EquationExtension`] onto this view.
    pub fn install_equation_with(
        &mut self,
        extension: EquationExtension,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(extension, cx)
    }
}

struct EquationSlashCompletionProvider {
    triggers: [CompletionTrigger; 1],
}

impl EquationSlashCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::prefix("/")],
        }
    }
}

impl CompletionProvider for EquationSlashCompletionProvider {
    fn id(&self) -> &'static str {
        "gpui_rich.equation.slash"
    }

    fn triggers(&self) -> &[CompletionTrigger] {
        &self.triggers
    }

    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::ProseOnly
    }

    fn header_title(&self) -> Option<&str> {
        Some("Slash Commands")
    }

    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        _view: WeakEntity<RichDocumentView>,
        _cx: &mut App,
    ) -> CompletionQueryResult {
        const DEFAULT_DISPLAY: &str = r"\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}";
        const DEFAULT_INLINE: &str = r"E = mc^2";
        let mut items = Vec::new();

        if let Some((score, indices)) = fuzzy_match_label(context.query, "Equation Block")
            .or_else(|| {
                fuzzy_match_label(context.query, "math").map(|(score, _)| (score - 5, Vec::new()))
            })
            .or_else(|| {
                fuzzy_match_label(context.query, "latex").map(|(score, _)| (score - 8, Vec::new()))
            })
        {
            items.push(
                CompletionItem::new(
                    "equation-block",
                    "Equation Block",
                    CompletionAccept::command(RichCommand::extension(InsertEquationBlock::new(
                        DEFAULT_DISPLAY,
                    ))),
                )
                .with_subtitle("Insert a centered LaTeX display equation")
                .with_badge("math")
                .with_category("Math")
                .with_match(score, indices)
                .with_detail(
                    CompletionDetail::new(
                        "Display Equation",
                        "Inserts a centered LaTeX display block rendered with RaTeX.",
                    )
                    .with_preview(DEFAULT_DISPLAY),
                ),
            );
        }

        if let Some((score, indices)) = fuzzy_match_label(context.query, "Inline Equation")
            .or_else(|| {
                fuzzy_match_label(context.query, "inline math")
                    .map(|(score, _)| (score - 5, Vec::new()))
            })
            .or_else(|| {
                fuzzy_match_label(context.query, "formula")
                    .map(|(score, _)| (score - 8, Vec::new()))
            })
        {
            items.push(
                CompletionItem::new(
                    "inline-equation",
                    "Inline Equation",
                    CompletionAccept::command(RichCommand::extension(InsertInlineEquation::new(
                        DEFAULT_INLINE,
                    ))),
                )
                .with_subtitle("Insert an inline LaTeX formula aligned to prose")
                .with_badge("inline")
                .with_category("Math")
                .with_match(score, indices)
                .with_detail(
                    CompletionDetail::new(
                        "Inline Equation",
                        "Inserts a baseline-aligned inline LaTeX formula. Click it to edit.",
                    )
                    .with_preview(DEFAULT_INLINE),
                ),
            );
        }

        CompletionQueryResult::Ready(items)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MetricsKey {
    source_hash: u64,
    mode: EquationDisplayMode,
    font_size_tenth: i32,
}

impl MetricsKey {
    fn new(source_hash: u64, mode: EquationDisplayMode, font_size_pt: f32) -> Self {
        Self {
            source_hash,
            mode,
            font_size_tenth: (font_size_pt * 10.0).round() as i32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CompileKey {
    source_hash: u64,
    options_hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RasterKey {
    source_hash: u64,
    options_hash: u64,
    target_width_dev: u32,
    target_height_dev: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum InFlightTarget {
    Block(BlockId),
    Inline(BlockId, InlineObjectId),
}

enum CompiledEquation {
    Ready {
        width_pt: f32,
        height_pt: f32,
        depth_pt: f32,
        parsed_svg: Arc<ParsedSvg>,
    },
    Error(EquationRenderError),
}

struct CompileCacheEntry {
    last_used: u64,
    equation: CompiledEquation,
}

struct RasterCacheEntry {
    last_used: u64,
    image: Arc<RenderImage>,
}

#[derive(Clone)]
struct LastValidEquation {
    image: Arc<RenderImage>,
    width_pt: f32,
    height_pt: f32,
    depth_pt: f32,
}

struct ActiveInlineEditor {
    block_id: BlockId,
    object_id: InlineObjectId,
    editor: Entity<InlineEquationPopoverEditor>,
}

enum BackgroundRenderOutcome {
    Ready {
        metrics_key: MetricsKey,
        compile_key: CompileKey,
        raster_key: RasterKey,
        width_pt: f32,
        height_pt: f32,
        depth_pt: f32,
        parsed_svg: Arc<ParsedSvg>,
        image: Arc<RenderImage>,
    },
    Error {
        compile_key: CompileKey,
        error: EquationRenderError,
    },
}

struct EquationRuntimeState {
    backend: Arc<dyn EquationBackend>,
    svg_renderer: SvgRenderer,
    config: EquationConfig,
    parent_view: WeakEntity<RichDocumentView>,
    clock: u64,
    metrics_cache: HashMap<MetricsKey, (f32, f32, f32)>,
    compile_cache: HashMap<CompileKey, CompileCacheEntry>,
    raster_cache: HashMap<RasterKey, RasterCacheEntry>,
    in_flight: HashMap<InFlightTarget, (RasterKey, Task<()>)>,
    last_valid_block: HashMap<BlockId, LastValidEquation>,
    last_valid_inline: HashMap<(BlockId, InlineObjectId), LastValidEquation>,
    block_modes: HashMap<BlockId, EquationViewMode>,
    block_options_hash: HashMap<BlockId, u64>,
    inline_last_error: HashMap<(BlockId, InlineObjectId), String>,
    block_editors: HashMap<BlockId, Entity<EquationBlockSourceEditor>>,
    pending_inline_editor: Option<(BlockId, InlineObjectId)>,
    active_inline_editor: Option<ActiveInlineEditor>,
    copied_block: Option<BlockId>,
    copy_reset_task: Option<Task<()>>,
}

impl EquationRuntimeState {
    fn new(
        backend: Arc<dyn EquationBackend>,
        svg_renderer: SvgRenderer,
        config: EquationConfig,
        parent_view: WeakEntity<RichDocumentView>,
    ) -> Self {
        Self {
            backend,
            svg_renderer,
            config,
            parent_view,
            clock: 1,
            metrics_cache: HashMap::new(),
            compile_cache: HashMap::new(),
            raster_cache: HashMap::new(),
            in_flight: HashMap::new(),
            last_valid_block: HashMap::new(),
            last_valid_inline: HashMap::new(),
            block_modes: HashMap::new(),
            block_options_hash: HashMap::new(),
            inline_last_error: HashMap::new(),
            block_editors: HashMap::new(),
            pending_inline_editor: None,
            active_inline_editor: None,
            copied_block: None,
            copy_reset_task: None,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn view_mode(&self, block_id: BlockId) -> EquationViewMode {
        self.block_modes
            .get(&block_id)
            .copied()
            .unwrap_or(EquationViewMode::Rendered)
    }

    fn set_view_mode(&mut self, block_id: BlockId, mode: EquationViewMode) {
        if mode == EquationViewMode::Rendered {
            self.block_modes.remove(&block_id);
            self.block_editors.remove(&block_id);
        } else {
            self.block_modes.insert(block_id, mode);
        }
    }

    fn lookup_compiled(&mut self, key: CompileKey) -> Option<&CompiledEquation> {
        let tick = self.next_tick();
        let entry = self.compile_cache.get_mut(&key)?;
        entry.last_used = tick;
        Some(&entry.equation)
    }

    fn peek_compiled(&self, key: CompileKey) -> Option<&CompiledEquation> {
        self.compile_cache.get(&key).map(|entry| &entry.equation)
    }

    fn insert_compiled(&mut self, key: CompileKey, equation: CompiledEquation) {
        let tick = self.next_tick();
        self.compile_cache.insert(
            key,
            CompileCacheEntry {
                last_used: tick,
                equation,
            },
        );
        self.evict_compile_lru();
    }

    fn lookup_raster(&mut self, key: RasterKey) -> Option<Arc<RenderImage>> {
        let tick = self.next_tick();
        let entry = self.raster_cache.get_mut(&key)?;
        entry.last_used = tick;
        Some(Arc::clone(&entry.image))
    }

    fn stale_raster_for(&self, source_hash: u64, options_hash: u64) -> Option<Arc<RenderImage>> {
        self.raster_cache
            .iter()
            .filter(|(key, _)| key.source_hash == source_hash && key.options_hash == options_hash)
            .max_by_key(|(_, entry)| entry.last_used)
            .map(|(_, entry)| Arc::clone(&entry.image))
    }

    fn insert_raster(&mut self, key: RasterKey, image: Arc<RenderImage>) {
        let tick = self.next_tick();
        self.raster_cache.insert(
            key,
            RasterCacheEntry {
                last_used: tick,
                image,
            },
        );
        self.evict_raster_lru();
    }

    fn insert_metrics(&mut self, key: MetricsKey, metrics: (f32, f32, f32)) {
        if self.metrics_cache.len() >= self.config.cache_capacity * 4 {
            self.metrics_cache.clear();
        }
        self.metrics_cache.insert(key, metrics);
    }

    fn evict_compile_lru(&mut self) {
        while self.compile_cache.len() > self.config.cache_capacity {
            let oldest = self
                .compile_cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            if let Some(key) = oldest {
                self.compile_cache.remove(&key);
            } else {
                break;
            }
        }
    }

    fn evict_raster_lru(&mut self) {
        while self.raster_cache.len() > self.config.cache_capacity {
            let oldest = self
                .raster_cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            if let Some(key) = oldest {
                self.raster_cache.remove(&key);
            } else {
                break;
            }
        }
    }

    fn block_font_size_pt(&self, data: &EquationBlockData) -> f32 {
        data.font_size_pt
            .filter(|size| size.is_finite() && *size > 0.0)
            .unwrap_or(self.config.default_block_font_size_pt)
            .clamp(8.0, 48.0)
    }

    fn block_status(&self, block_id: BlockId, data: &EquationBlockData) -> EquationBlockStatus {
        let mode = self.view_mode(block_id);
        let source_hash = hash_equation_source(&data.source);
        let default_options =
            EquationRenderOptions::display(self.block_font_size_pt(data), RgbaColor::BLACK);
        let options_hash = self
            .block_options_hash
            .get(&block_id)
            .copied()
            .unwrap_or_else(|| default_options.fingerprint());
        let compile_key = CompileKey {
            source_hash,
            options_hash,
        };
        let render = if let Some(compiled) = self.peek_compiled(compile_key) {
            match compiled {
                CompiledEquation::Ready {
                    width_pt,
                    height_pt,
                    depth_pt,
                    ..
                } => EquationRenderStatus::Ready {
                    width_pt: *width_pt,
                    height_pt: *height_pt,
                    depth_pt: *depth_pt,
                },
                CompiledEquation::Error(error) => EquationRenderStatus::Error(error.clone()),
            }
        } else if self
            .in_flight
            .contains_key(&InFlightTarget::Block(block_id))
        {
            EquationRenderStatus::Rendering
        } else {
            EquationRenderStatus::Idle
        };
        EquationBlockStatus { mode, render }
    }
}

enum EquationVisual {
    Image {
        image: Arc<RenderImage>,
        display_width: f32,
        display_height: f32,
        inline_error: Option<EquationRenderError>,
    },
    Loading,
    Error(EquationRenderError),
}

fn hsla_to_rgba(color: Hsla) -> RgbaColor {
    let rgba: Rgba = color.into();
    RgbaColor::rgba(
        (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn error_foreground(theme: &RichTheme) -> Hsla {
    if theme.is_dark() {
        rgba_to_hsla(RgbaColor::rgb(0xff, 0x7b, 0x72))
    } else {
        rgba_to_hsla(RgbaColor::rgb(0xcf, 0x22, 0x2e))
    }
}

fn error_background(theme: &RichTheme) -> Hsla {
    if theme.is_dark() {
        rgba_to_hsla(RgbaColor::rgba(0xf8, 0x51, 0x49, 0x26))
    } else {
        rgba_to_hsla(RgbaColor::rgba(0xcf, 0x22, 0x2e, 0x14))
    }
}

fn quantize_device_px(value: f32) -> u32 {
    let clamped = value.max(1.0);
    if clamped <= 64.0 {
        clamped.round() as u32
    } else {
        (((clamped / 4.0).round() as u32) * 4).max(1)
    }
}

fn fit_display_equation(width_pt: f32, height_pt: f32, max_width_px: f32, zoom: f32) -> (f32, f32) {
    let natural_w = pt_to_px(width_pt.max(1.0), zoom);
    let natural_h = pt_to_px(height_pt.max(1.0), zoom);
    let max_w = max_width_px.max(32.0);
    if natural_w > max_w {
        let scale = max_w / natural_w;
        (max_w, (natural_h * scale).max(1.0))
    } else {
        (natural_w, natural_h)
    }
}

fn measure_block_editor_height(source: &str, width_px: f32, zoom: f32) -> f32 {
    let inner_w = (width_px - pt_to_px(20.0, zoom)).max(60.0);
    let chars_per_line = (inner_w / pt_to_px(6.1, zoom)).floor().max(18.0) as usize;
    let lines = source
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(chars_per_line))
        .sum::<usize>()
        .clamp(1, 10) as f32;
    pt_to_px(BLOCK_EDITOR_VERT_PAD_PT, zoom) + lines * pt_to_px(BLOCK_EDITOR_LINE_HEIGHT_PT, zoom)
}

fn measure_block_preview_height(
    runtime: &EquationRuntimeState,
    block_id: BlockId,
    data: &EquationBlockData,
    max_width_px: f32,
    zoom: f32,
    edit_mode: bool,
) -> f32 {
    let pad_x = pt_to_px(BLOCK_PAD_X_PT, zoom);
    let pad_y = pt_to_px(BLOCK_PAD_Y_PT, zoom);
    let numbering_gutter = if data
        .numbering
        .as_deref()
        .is_some_and(|tag| !tag.trim().is_empty())
    {
        pt_to_px(BLOCK_NUMBERING_GUTTER_PT, zoom)
    } else {
        0.0
    };
    let available_w = (max_width_px - 2.0 * pad_x - 2.0 * numbering_gutter).max(48.0);
    let font_size_pt = runtime.block_font_size_pt(data);
    let source_hash = hash_equation_source(&data.source);
    let metrics_key = MetricsKey::new(source_hash, EquationDisplayMode::Display, font_size_pt);

    let formula_h = if let Some(&(width_pt, height_pt, _)) = runtime.metrics_cache.get(&metrics_key)
    {
        fit_display_equation(width_pt, height_pt, available_w, zoom).1
    } else if let Some(last_valid) = runtime.last_valid_block.get(&block_id) {
        let base_h =
            fit_display_equation(last_valid.width_pt, last_valid.height_pt, available_w, zoom).1;
        if edit_mode {
            base_h + pt_to_px(16.0, zoom)
        } else {
            base_h
        }
    } else if let Some(options_hash) = runtime.block_options_hash.get(&block_id).copied()
        && matches!(
            runtime.peek_compiled(CompileKey {
                source_hash,
                options_hash
            }),
            Some(CompiledEquation::Error(_))
        )
    {
        pt_to_px(BLOCK_ERROR_HEIGHT_PT, zoom)
    } else {
        pt_to_px(BLOCK_PLACEHOLDER_HEIGHT_PT, zoom)
    };

    2.0 * pad_y + formula_h.max(pt_to_px(24.0, zoom))
}

struct EquationBlockRenderer {
    state: Rc<RefCell<EquationRuntimeState>>,
}

impl EquationBlockRenderer {
    #[allow(clippy::too_many_arguments)]
    fn prepare_visual(
        &self,
        block_id: BlockId,
        source: &str,
        options: EquationRenderOptions,
        metrics_key: MetricsKey,
        compile_key: CompileKey,
        max_formula_w: f32,
        zoom: f32,
        scale_factor: f32,
        preserve_last_valid_on_error: bool,
        cx: &mut App,
    ) -> EquationVisual {
        let mut runtime = self.state.borrow_mut();

        let (compiled_parsed, display_width, display_height) =
            match runtime.lookup_compiled(compile_key) {
                Some(CompiledEquation::Error(error)) => {
                    let error = error.clone();
                    if preserve_last_valid_on_error
                        && let Some(last_valid) = runtime.last_valid_block.get(&block_id).cloned()
                    {
                        let (width, height) = fit_display_equation(
                            last_valid.width_pt,
                            last_valid.height_pt,
                            max_formula_w,
                            zoom,
                        );
                        return EquationVisual::Image {
                            image: last_valid.image,
                            display_width: width,
                            display_height: height,
                            inline_error: Some(error),
                        };
                    }
                    return EquationVisual::Error(error);
                }
                Some(CompiledEquation::Ready {
                    width_pt,
                    height_pt,
                    depth_pt,
                    parsed_svg,
                }) => {
                    let (width, height) =
                        fit_display_equation(*width_pt, *height_pt, max_formula_w, zoom);
                    (
                        Some((Arc::clone(parsed_svg), *width_pt, *height_pt, *depth_pt)),
                        width,
                        height,
                    )
                }
                None => (
                    None,
                    max_formula_w.min(pt_to_px(180.0, zoom)),
                    pt_to_px(BLOCK_PLACEHOLDER_HEIGHT_PT, zoom),
                ),
            };

        let target_width_dev = quantize_device_px(display_width * scale_factor);
        let target_height_dev = ((display_height * scale_factor).round() as u32).max(1);
        let raster_key = RasterKey {
            source_hash: compile_key.source_hash,
            options_hash: compile_key.options_hash,
            target_width_dev,
            target_height_dev,
        };

        if let Some(image) = runtime.lookup_raster(raster_key) {
            if let Some((_, width_pt, height_pt, depth_pt)) = compiled_parsed {
                runtime.last_valid_block.insert(
                    block_id,
                    LastValidEquation {
                        image: Arc::clone(&image),
                        width_pt,
                        height_pt,
                        depth_pt,
                    },
                );
            }
            return EquationVisual::Image {
                image,
                display_width,
                display_height,
                inline_error: None,
            };
        }

        let stale_same_source =
            runtime.stale_raster_for(compile_key.source_hash, compile_key.options_hash);
        let last_valid_fallback = runtime.last_valid_block.get(&block_id).cloned();

        let target = InFlightTarget::Block(block_id);
        let already_in_flight = runtime
            .in_flight
            .get(&target)
            .is_some_and(|(active_key, _)| *active_key == raster_key);
        if !already_in_flight {
            let backend = Arc::clone(&runtime.backend);
            let svg_renderer = runtime.svg_renderer.clone();
            let owned_source = source.to_owned();
            let state_weak = Rc::downgrade(&self.state);
            let parent_view = runtime.parent_view.clone();
            let debounce = if preserve_last_valid_on_error
                && compiled_parsed.is_none()
                && last_valid_fallback.is_some()
            {
                Some(LIVE_COMPILE_DEBOUNCE)
            } else {
                None
            };

            let task = cx.spawn(async move |async_app: &mut AsyncApp| {
                if let Some(delay) = debounce {
                    async_app.background_executor().timer(delay).await;
                }
                let outcome = async_app
                    .background_executor()
                    .spawn(async move {
                        run_background_compile_and_raster(
                            compiled_parsed,
                            owned_source,
                            options,
                            metrics_key,
                            compile_key,
                            Some(max_formula_w),
                            zoom,
                            scale_factor,
                            backend,
                            svg_renderer,
                        )
                    })
                    .await;

                async_app.update(|cx| {
                    let Some(state_rc) = state_weak.upgrade() else {
                        return;
                    };
                    let needs_relayout = {
                        let mut runtime = state_rc.borrow_mut();
                        runtime.in_flight.remove(&target);
                        match outcome {
                            BackgroundRenderOutcome::Ready {
                                metrics_key,
                                compile_key,
                                raster_key,
                                width_pt,
                                height_pt,
                                depth_pt,
                                parsed_svg,
                                image,
                            } => {
                                let prev_metrics = runtime.metrics_cache.get(&metrics_key).copied();
                                runtime
                                    .insert_metrics(metrics_key, (width_pt, height_pt, depth_pt));
                                runtime.last_valid_block.insert(
                                    block_id,
                                    LastValidEquation {
                                        image: Arc::clone(&image),
                                        width_pt,
                                        height_pt,
                                        depth_pt,
                                    },
                                );
                                runtime.insert_compiled(
                                    compile_key,
                                    CompiledEquation::Ready {
                                        width_pt,
                                        height_pt,
                                        depth_pt,
                                        parsed_svg,
                                    },
                                );
                                runtime.insert_raster(raster_key, image);
                                prev_metrics != Some((width_pt, height_pt, depth_pt))
                            }
                            BackgroundRenderOutcome::Error { compile_key, error } => {
                                runtime
                                    .insert_compiled(compile_key, CompiledEquation::Error(error));
                                true
                            }
                        }
                    };

                    if let Some(view) = parent_view.upgrade() {
                        view.update(cx, |view, cx| {
                            if needs_relayout {
                                view.invalidate_block_layout(block_id, cx);
                            } else {
                                cx.notify();
                            }
                        });
                    }
                });
            });

            runtime.in_flight.insert(target, (raster_key, task));
        }

        if let Some(image) = stale_same_source {
            return EquationVisual::Image {
                image,
                display_width,
                display_height,
                inline_error: None,
            };
        }

        if let Some(last_valid) = last_valid_fallback {
            let (width, height) = fit_display_equation(
                last_valid.width_pt,
                last_valid.height_pt,
                max_formula_w,
                zoom,
            );
            return EquationVisual::Image {
                image: last_valid.image,
                display_width: width,
                display_height: height,
                inline_error: None,
            };
        }

        EquationVisual::Loading
    }
}

impl BlockRenderer for EquationBlockRenderer {
    type Spec = EquationBlockSpec;

    fn measure_leaf(
        &self,
        block_id: BlockId,
        data: &EquationBlockData,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        let runtime = self.state.borrow();
        let width = max_width.max(120.0);
        let mode = runtime.view_mode(block_id);
        let preview_h = measure_block_preview_height(
            &runtime,
            block_id,
            data,
            width,
            zoom,
            mode == EquationViewMode::Edit,
        );
        let total_height = match mode {
            EquationViewMode::Rendered => preview_h,
            EquationViewMode::Edit => {
                let header_h = pt_to_px(BLOCK_HEADER_HEIGHT_PT, zoom);
                let editor_h = measure_block_editor_height(&data.source, width, zoom);
                header_h + preview_h + editor_h + 1.0
            }
        };
        Size::new(width, total_height.max(pt_to_px(32.0, zoom)))
    }

    fn render(
        &self,
        context: BlockRenderContext<'_, EquationBlockData>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let block_id = context.block_id;
        let zoom = context.zoom;
        let theme = context.theme;
        let ink_color = hsla_to_rgba(theme.text);
        let font_size_pt = self.state.borrow().block_font_size_pt(context.data);
        let options = EquationRenderOptions::display(font_size_pt, ink_color);
        let options_hash = options.fingerprint();
        let source_hash = hash_equation_source(&context.data.source);
        let metrics_key = MetricsKey::new(source_hash, EquationDisplayMode::Display, font_size_pt);
        let compile_key = CompileKey {
            source_hash,
            options_hash,
        };

        let (policy, parent_view, mode, copied) = {
            let mut runtime = self.state.borrow_mut();
            runtime.block_options_hash.insert(block_id, options_hash);
            let parent = runtime.parent_view.clone();
            let policy = parent
                .upgrade()
                .map(|view| view.read(cx).policy())
                .unwrap_or_else(InteractionPolicy::viewer);
            let mode = runtime.view_mode(block_id);
            let copied = runtime.copied_block == Some(block_id);
            (policy, parent, mode, copied)
        };

        let total_w: f32 = context.bounds.size.width.into();
        let pad_x = pt_to_px(BLOCK_PAD_X_PT, zoom);
        let pad_y = pt_to_px(BLOCK_PAD_Y_PT, zoom);
        let numbering_tag = context
            .data
            .numbering
            .as_deref()
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| format!("({tag})"));
        let numbering_gutter = if numbering_tag.is_some() {
            pt_to_px(BLOCK_NUMBERING_GUTTER_PT, zoom)
        } else {
            0.0
        };
        let max_formula_w = (total_w - 2.0 * pad_x - 2.0 * numbering_gutter).max(48.0);
        let scale_factor = window.scale_factor().max(1.0);

        let visual = self.prepare_visual(
            block_id,
            &context.data.source,
            options,
            metrics_key,
            compile_key,
            max_formula_w,
            zoom,
            scale_factor,
            mode == EquationViewMode::Edit,
            cx,
        );

        let formula_body = build_equation_preview_element(
            visual,
            numbering_tag.as_deref(),
            zoom,
            theme,
            pad_x,
            pad_y,
        );

        if mode == EquationViewMode::Rendered {
            let can_edit = policy.editing.enabled;
            let parent_for_click = parent_view.clone();
            let mut rendered_card = div()
                .id(("equation-block-card", block_id.0))
                .relative()
                .size_full()
                .rounded(px(pt_to_px(4.0, zoom)))
                .flex()
                .items_center()
                .justify_center()
                .hover(|style| style.bg(theme.code_background.alpha(0.65)))
                .child(formula_body);

            if can_edit {
                rendered_card = rendered_card.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    move |_event, _window, cx| {
                        if let Some(view) = parent_for_click.upgrade() {
                            view.update(cx, |view, cx| {
                                view.dispatch_extension_command(
                                    SetEquationViewMode {
                                        block_id,
                                        mode: EquationViewMode::Edit,
                                    },
                                    cx,
                                );
                            });
                        }
                        cx.stop_propagation();
                    },
                );
            }

            return Some(CustomRenderOutput::Element(
                rendered_card.into_any_element(),
            ));
        }

        let (editor_entity, newly_created) = {
            let mut runtime = self.state.borrow_mut();
            if let Some(existing) = runtime.block_editors.get(&block_id).cloned() {
                (existing, false)
            } else {
                let initial_source = context.data.source.clone();
                let initial_theme = theme.clone();
                let editable = policy.editing.enabled;
                let parent_for_editor = parent_view.clone();
                let entity = cx.new(|cx| {
                    EquationBlockSourceEditor::new(
                        block_id,
                        &initial_source,
                        &initial_theme,
                        editable,
                        zoom,
                        parent_for_editor,
                        cx,
                    )
                });
                runtime.block_editors.insert(block_id, entity.clone());
                (entity, true)
            }
        };
        editor_entity.update(cx, |editor, cx| {
            editor.sync(
                &context.data.source,
                theme,
                policy.editing.enabled,
                zoom,
                cx,
            );
            if newly_created && policy.editing.enabled {
                editor.focus_editor(window, cx);
            }
        });

        let header = build_block_edit_header(
            block_id,
            &context.data.source,
            copied,
            zoom,
            theme,
            Rc::clone(&self.state),
            parent_view,
        );
        let editor_h = measure_block_editor_height(&context.data.source, total_w, zoom);

        let edit_card = div()
            .id(("equation-block-edit-card", block_id.0))
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .rounded(px(pt_to_px(5.0, zoom)))
            .border_1()
            .border_color(theme.accent.alpha(0.55))
            .bg(theme.code_background)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .bg(theme.surface_background)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(formula_body),
            )
            .child(
                div()
                    .w_full()
                    .h(px(editor_h))
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.code_border)
                    .overflow_hidden()
                    .child(editor_entity),
            )
            .into_any_element();

        Some(CustomRenderOutput::Element(edit_card))
    }

    fn plain_text(&self, data: &EquationBlockData) -> Option<String> {
        Some(format!("$$\n{}\n$$", data.source))
    }

    fn html(&self, data: &EquationBlockData) -> Option<String> {
        let escaped = escape_html(&data.source);
        Some(format!(
            "<div class=\"math-display\" data-latex=\"{escaped}\">$${escaped}$$</div>"
        ))
    }
}

struct InlineEquationRenderer {
    state: Rc<RefCell<EquationRuntimeState>>,
}

impl InlineEquationRenderer {
    #[allow(clippy::too_many_arguments)]
    fn prepare_inline_visual(
        &self,
        block_id: BlockId,
        object_id: InlineObjectId,
        source: &str,
        options: EquationRenderOptions,
        metrics_key: MetricsKey,
        compile_key: CompileKey,
        zoom: f32,
        scale_factor: f32,
        cx: &mut App,
    ) -> EquationVisual {
        let mut runtime = self.state.borrow_mut();

        let (compiled_parsed, display_width, display_height, clear_error) =
            match runtime.lookup_compiled(compile_key) {
                Some(CompiledEquation::Error(error)) => {
                    let error = error.clone();
                    runtime
                        .inline_last_error
                        .insert((block_id, object_id), error.to_string());
                    if let Some(last_valid) = runtime
                        .last_valid_inline
                        .get(&(block_id, object_id))
                        .cloned()
                    {
                        return EquationVisual::Image {
                            image: last_valid.image,
                            display_width: pt_to_px(last_valid.width_pt, zoom),
                            display_height: pt_to_px(last_valid.height_pt, zoom),
                            inline_error: Some(error),
                        };
                    }
                    return EquationVisual::Error(error);
                }
                Some(CompiledEquation::Ready {
                    width_pt,
                    height_pt,
                    depth_pt,
                    parsed_svg,
                }) => (
                    Some((Arc::clone(parsed_svg), *width_pt, *height_pt, *depth_pt)),
                    pt_to_px(*width_pt, zoom),
                    pt_to_px(*height_pt, zoom),
                    true,
                ),
                None => (
                    None,
                    estimate_inline_width_px(source, options.font_size_pt, zoom),
                    pt_to_px(options.font_size_pt * 1.15, zoom),
                    false,
                ),
            };
        if clear_error {
            runtime.inline_last_error.remove(&(block_id, object_id));
        }

        let target_width_dev = quantize_device_px(display_width * scale_factor);
        let target_height_dev = ((display_height * scale_factor).round() as u32).max(1);
        let raster_key = RasterKey {
            source_hash: compile_key.source_hash,
            options_hash: compile_key.options_hash,
            target_width_dev,
            target_height_dev,
        };

        if let Some(image) = runtime.lookup_raster(raster_key) {
            if let Some((_, width_pt, height_pt, depth_pt)) = compiled_parsed {
                runtime.last_valid_inline.insert(
                    (block_id, object_id),
                    LastValidEquation {
                        image: Arc::clone(&image),
                        width_pt,
                        height_pt,
                        depth_pt,
                    },
                );
            }
            return EquationVisual::Image {
                image,
                display_width,
                display_height,
                inline_error: None,
            };
        }

        let stale_same_source =
            runtime.stale_raster_for(compile_key.source_hash, compile_key.options_hash);
        let last_valid_fallback = runtime
            .last_valid_inline
            .get(&(block_id, object_id))
            .cloned();

        let target = InFlightTarget::Inline(block_id, object_id);
        let already_in_flight = runtime
            .in_flight
            .get(&target)
            .is_some_and(|(active_key, _)| *active_key == raster_key);
        if !already_in_flight {
            let backend = Arc::clone(&runtime.backend);
            let svg_renderer = runtime.svg_renderer.clone();
            let owned_source = source.to_owned();
            let state_weak = Rc::downgrade(&self.state);
            let parent_view = runtime.parent_view.clone();
            let is_editing_this = runtime
                .active_inline_editor
                .as_ref()
                .is_some_and(|active| active.block_id == block_id && active.object_id == object_id);
            let debounce =
                if is_editing_this && compiled_parsed.is_none() && last_valid_fallback.is_some() {
                    Some(LIVE_COMPILE_DEBOUNCE)
                } else {
                    None
                };

            let task = cx.spawn(async move |async_app: &mut AsyncApp| {
                if let Some(delay) = debounce {
                    async_app.background_executor().timer(delay).await;
                }
                let outcome = async_app
                    .background_executor()
                    .spawn(async move {
                        run_background_compile_and_raster(
                            compiled_parsed,
                            owned_source,
                            options,
                            metrics_key,
                            compile_key,
                            None,
                            zoom,
                            scale_factor,
                            backend,
                            svg_renderer,
                        )
                    })
                    .await;

                async_app.update(|cx| {
                    let Some(state_rc) = state_weak.upgrade() else {
                        return;
                    };
                    let needs_relayout = {
                        let mut runtime = state_rc.borrow_mut();
                        runtime.in_flight.remove(&target);
                        match outcome {
                            BackgroundRenderOutcome::Ready {
                                metrics_key,
                                compile_key,
                                raster_key,
                                width_pt,
                                height_pt,
                                depth_pt,
                                parsed_svg,
                                image,
                            } => {
                                let prev_metrics = runtime.metrics_cache.get(&metrics_key).copied();
                                runtime.inline_last_error.remove(&(block_id, object_id));
                                runtime
                                    .insert_metrics(metrics_key, (width_pt, height_pt, depth_pt));
                                runtime.last_valid_inline.insert(
                                    (block_id, object_id),
                                    LastValidEquation {
                                        image: Arc::clone(&image),
                                        width_pt,
                                        height_pt,
                                        depth_pt,
                                    },
                                );
                                runtime.insert_compiled(
                                    compile_key,
                                    CompiledEquation::Ready {
                                        width_pt,
                                        height_pt,
                                        depth_pt,
                                        parsed_svg,
                                    },
                                );
                                runtime.insert_raster(raster_key, image);
                                prev_metrics != Some((width_pt, height_pt, depth_pt))
                            }
                            BackgroundRenderOutcome::Error { compile_key, error } => {
                                runtime
                                    .inline_last_error
                                    .insert((block_id, object_id), error.to_string());
                                runtime
                                    .insert_compiled(compile_key, CompiledEquation::Error(error));
                                true
                            }
                        }
                    };

                    if let Some(view) = parent_view.upgrade() {
                        view.update(cx, |view, cx| {
                            if needs_relayout {
                                view.invalidate_block_layout(block_id, cx);
                            } else {
                                cx.notify();
                            }
                        });
                    }
                });
            });

            runtime.in_flight.insert(target, (raster_key, task));
        }

        if let Some(image) = stale_same_source {
            return EquationVisual::Image {
                image,
                display_width,
                display_height,
                inline_error: None,
            };
        }

        if let Some(last_valid) = last_valid_fallback {
            return EquationVisual::Image {
                image: last_valid.image,
                display_width: pt_to_px(last_valid.width_pt, zoom),
                display_height: pt_to_px(last_valid.height_pt, zoom),
                inline_error: None,
            };
        }

        EquationVisual::Loading
    }
}

fn estimate_inline_width_px(source: &str, font_size_pt: f32, zoom: f32) -> f32 {
    let char_count = source.trim().chars().count().clamp(2, 24) as f32;
    pt_to_px(char_count * font_size_pt * 0.52, zoom)
}

impl InlineRenderer for InlineEquationRenderer {
    type Spec = InlineEquationSpec;

    fn measure_with_context(
        &self,
        context: InlineMeasureContext<'_, InlineEquationData>,
        _window: &Window,
    ) -> InlineBoxMetrics {
        let runtime = self.state.borrow();
        let font_size_pt = if context.font_size_pt.is_finite() && context.font_size_pt > 0.0 {
            context.font_size_pt
        } else {
            11.0
        };
        let source_hash = hash_equation_source(&context.data.source);
        let metrics_key = MetricsKey::new(source_hash, EquationDisplayMode::Inline, font_size_pt);
        let pad_x = pt_to_px(INLINE_PAD_X_PT, context.zoom);

        if let Some(&(width_pt, height_pt, depth_pt)) = runtime.metrics_cache.get(&metrics_key) {
            return InlineBoxMetrics::baseline_with_depth(
                pt_to_px(width_pt, context.zoom) + 2.0 * pad_x,
                pt_to_px(height_pt, context.zoom),
                pt_to_px(depth_pt, context.zoom),
            );
        }

        if let Some(block_id) = context.block_id
            && let Some(last_valid) = runtime
                .last_valid_inline
                .get(&(block_id, context.object_id))
        {
            return InlineBoxMetrics::baseline_with_depth(
                pt_to_px(last_valid.width_pt, context.zoom) + 2.0 * pad_x,
                pt_to_px(last_valid.height_pt, context.zoom),
                pt_to_px(last_valid.depth_pt, context.zoom),
            );
        }

        let estimated_w =
            estimate_inline_width_px(&context.data.source, font_size_pt, context.zoom)
                + 2.0 * pad_x;
        let estimated_h = pt_to_px(font_size_pt * 1.15, context.zoom);
        let estimated_d = pt_to_px(font_size_pt * 0.2, context.zoom);
        InlineBoxMetrics::baseline_with_depth(estimated_w, estimated_h, estimated_d)
    }

    fn render(
        &self,
        context: InlineRenderContext<'_, InlineEquationData>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let block_id = context.block_id;
        let object_id = context.object_id;
        let zoom = context.zoom;
        let theme = context.theme;
        let font_size_pt = if context.font_size_pt.is_finite() && context.font_size_pt > 0.0 {
            context.font_size_pt
        } else {
            11.0
        };
        let resolved_color = theme.resolve_text_color(context.text_color);
        let ink_color = hsla_to_rgba(resolved_color);
        let options = EquationRenderOptions::inline(font_size_pt, ink_color);
        let options_hash = options.fingerprint();
        let source_hash = hash_equation_source(&context.data.source);
        let metrics_key = MetricsKey::new(source_hash, EquationDisplayMode::Inline, font_size_pt);
        let compile_key = CompileKey {
            source_hash,
            options_hash,
        };
        let scale_factor = window.scale_factor().max(1.0);

        let visual = self.prepare_inline_visual(
            block_id,
            object_id,
            &context.data.source,
            options,
            metrics_key,
            compile_key,
            zoom,
            scale_factor,
            cx,
        );

        let (parent_view, is_active_editor, error_message, active_editor_entity) = {
            let runtime = self.state.borrow();
            let parent = runtime.parent_view.clone();
            let active = runtime
                .active_inline_editor
                .as_ref()
                .filter(|entry| entry.block_id == block_id && entry.object_id == object_id)
                .map(|entry| entry.editor.clone());
            let err = runtime
                .inline_last_error
                .get(&(block_id, object_id))
                .cloned();
            (parent, active.is_some(), err, active)
        };

        if let Some(editor_entity) = active_editor_entity {
            editor_entity.update(cx, |editor, cx| {
                editor.sync(&context.data.source, error_message, theme, zoom, cx);
            });
        }

        let editable = parent_view
            .upgrade()
            .map(|view| view.read(cx).policy().editing.enabled)
            .unwrap_or(false);

        let pad_x = pt_to_px(INLINE_PAD_X_PT, zoom);
        let content: AnyElement = match visual {
            EquationVisual::Image {
                image,
                display_width,
                display_height,
                inline_error,
            } => {
                let image_for_paint = Arc::clone(&image);
                let mut wrapper = div().w(px(display_width)).h(px(display_height)).child(
                    canvas(
                        |_bounds, _window, _cx| {},
                        move |bounds, _state, window, _cx| {
                            if let Err(error) = window.paint_image(
                                bounds,
                                bounds,
                                Corners::all(px(0.0)),
                                image_for_paint,
                                0,
                                false,
                            ) {
                                eprintln!(
                                    "rich-gpui: failed to paint inline equation image: {error}"
                                );
                            }
                        },
                    )
                    .size_full(),
                );
                if inline_error.is_some() {
                    wrapper = wrapper.border_b_1().border_color(error_foreground(theme));
                }
                wrapper.into_any_element()
            }
            EquationVisual::Loading => div()
                .text_size(px(pt_to_px(font_size_pt * 0.85, zoom)))
                .text_color(theme.muted_text)
                .child(context.data.source.clone())
                .into_any_element(),
            EquationVisual::Error(_) => div()
                .px(px(pt_to_px(3.0, zoom)))
                .rounded(px(pt_to_px(3.0, zoom)))
                .bg(error_background(theme))
                .text_size(px(pt_to_px(font_size_pt * 0.82, zoom)))
                .text_color(error_foreground(theme))
                .child(context.data.source.clone())
                .into_any_element(),
        };

        let mut pill = div()
            .id(("inline-equation-node", (block_id.0 << 20) ^ object_id.0))
            .size_full()
            .px(px(pad_x))
            .rounded(px(pt_to_px(3.0, zoom)))
            .flex()
            .items_center()
            .justify_center()
            .child(content);

        if is_active_editor {
            pill = pill
                .bg(theme.accent.alpha(0.12))
                .border_1()
                .border_color(theme.accent.alpha(0.55));
        } else if editable {
            pill = pill
                .cursor_pointer()
                .hover(|style| style.bg(theme.accent.alpha(0.08)));
        }

        if editable {
            let parent_for_click = parent_view;
            pill = pill.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                if let Some(view) = parent_for_click.upgrade() {
                    view.update(cx, |view, cx| {
                        view.dispatch_extension_command(
                            OpenInlineEquationEditor {
                                block_id,
                                object_id,
                            },
                            cx,
                        );
                    });
                }
                cx.stop_propagation();
            });
        }

        Some(CustomRenderOutput::Element(pill.into_any_element()))
    }

    fn plain_text(&self, data: &InlineEquationData) -> Option<String> {
        Some(format!("${}$", data.source))
    }

    fn html(&self, data: &InlineEquationData) -> Option<String> {
        let escaped = escape_html(&data.source);
        Some(format!(
            "<span class=\"math-inline\" data-latex=\"{escaped}\">${escaped}$</span>"
        ))
    }
}

fn build_inline_equation_overlay_spec(
    block_id: BlockId,
    object_id: InlineObjectId,
    state: Rc<RefCell<EquationRuntimeState>>,
) -> OverlaySpec {
    OverlaySpec::new(
        INLINE_EQUATION_EDITOR_OVERLAY_ID,
        OverlayAnchor::InlineObject {
            block_id,
            object_id,
        },
        move |overlay_context, window, cx| {
            let source = overlay_context
                .view
                .read(cx)
                .document
                .find_block(block_id)
                .and_then(|block| block.kind.inline_content())
                .and_then(|inline| inline.object_by_id(object_id))
                .and_then(|entry| entry.object.read_data::<InlineEquationSpec>().ok())
                .map(|data| data.source.clone())
                .unwrap_or_default();

            let (editor_entity, error_message) = {
                let mut runtime = state.borrow_mut();
                let error = runtime
                    .inline_last_error
                    .get(&(block_id, object_id))
                    .cloned();
                let existing = runtime
                    .active_inline_editor
                    .as_ref()
                    .filter(|active| active.block_id == block_id && active.object_id == object_id)
                    .map(|active| active.editor.clone());
                let entity = match existing {
                    Some(entity) => entity,
                    None => {
                        let parent_weak = overlay_context.view.downgrade();
                        let initial_theme = overlay_context.theme.clone();
                        let zoom = overlay_context.zoom;
                        let entity = cx.new(|cx| {
                            InlineEquationPopoverEditor::new(
                                block_id,
                                object_id,
                                &source,
                                &initial_theme,
                                zoom,
                                parent_weak,
                                window,
                                cx,
                            )
                        });
                        runtime.active_inline_editor = Some(ActiveInlineEditor {
                            block_id,
                            object_id,
                            editor: entity.clone(),
                        });
                        entity
                    }
                };
                (entity, error)
            };

            editor_entity.update(cx, |editor, cx| {
                editor.sync(
                    &source,
                    error_message,
                    overlay_context.theme,
                    overlay_context.zoom,
                    cx,
                );
                editor.focus_editor(window, cx);
            });

            editor_entity.into_any_element()
        },
    )
    .with_placement(OverlayPlacement::below_start().with_gap_px(6.0))
    .with_z_layer(OverlayZLayer::Interactive)
    .with_dismiss_policy(OverlayDismissPolicy {
        on_outside_click: true,
        on_blur: false,
        on_scroll: false,
        on_selection_change: false,
        on_document_edit: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_background_compile_and_raster(
    existing_parsed: Option<(Arc<ParsedSvg>, f32, f32, f32)>,
    source: String,
    options: EquationRenderOptions,
    metrics_key: MetricsKey,
    compile_key: CompileKey,
    max_width_px: Option<f32>,
    zoom: f32,
    scale_factor: f32,
    backend: Arc<dyn EquationBackend>,
    svg_renderer: SvgRenderer,
) -> BackgroundRenderOutcome {
    let (parsed_svg, width_pt, height_pt, depth_pt) = match existing_parsed {
        Some(existing) => existing,
        None => {
            let compiled = match backend.render_svg(&source, &options) {
                Ok(svg) => svg,
                Err(error) => {
                    return BackgroundRenderOutcome::Error { compile_key, error };
                }
            };
            let parsed = match svg_renderer.parse_svg(compiled.svg.as_bytes()) {
                Ok(parsed) => Arc::new(parsed),
                Err(error) => {
                    return BackgroundRenderOutcome::Error {
                        compile_key,
                        error: EquationRenderError::new(format!(
                            "Failed to parse compiled equation SVG: {error}"
                        )),
                    };
                }
            };
            (
                parsed,
                compiled.width_pt,
                compiled.height_pt,
                compiled.depth_pt,
            )
        }
    };

    let (display_w, display_h) = match max_width_px {
        Some(max_w) => fit_display_equation(width_pt, height_pt, max_w, zoom),
        None => (
            pt_to_px(width_pt.max(1.0), zoom),
            pt_to_px(height_pt.max(1.0), zoom),
        ),
    };
    let target_width_dev = quantize_device_px(display_w * scale_factor);
    let target_height_dev = ((display_h * scale_factor).round() as u32).max(1);
    let raster_key = RasterKey {
        source_hash: compile_key.source_hash,
        options_hash: compile_key.options_hash,
        target_width_dev,
        target_height_dev,
    };

    let device_size = GpuiSize {
        width: DevicePixels(target_width_dev.min(i32::MAX as u32) as i32),
        height: DevicePixels(target_height_dev.min(i32::MAX as u32) as i32),
    };
    match svg_renderer.render_parsed(&parsed_svg, SvgSize::ExactSize(device_size)) {
        Ok(image) => BackgroundRenderOutcome::Ready {
            metrics_key,
            compile_key,
            raster_key,
            width_pt,
            height_pt,
            depth_pt,
            parsed_svg,
            image,
        },
        Err(error) => BackgroundRenderOutcome::Error {
            compile_key,
            error: EquationRenderError::new(format!("Failed to rasterize equation SVG: {error}")),
        },
    }
}

fn build_equation_preview_element(
    visual: EquationVisual,
    numbering_tag: Option<&str>,
    zoom: f32,
    theme: &RichTheme,
    pad_x: f32,
    pad_y: f32,
) -> AnyElement {
    let center_content: AnyElement = match visual {
        EquationVisual::Image {
            image,
            display_width,
            display_height,
            inline_error,
        } => {
            let image_for_paint = Arc::clone(&image);
            let mut column = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(pt_to_px(4.0, zoom)))
                .child(
                    div().w(px(display_width)).h(px(display_height)).child(
                        canvas(
                            |_bounds, _window, _cx| {},
                            move |bounds, _state, window, _cx| {
                                if let Err(error) = window.paint_image(
                                    bounds,
                                    bounds,
                                    Corners::all(px(0.0)),
                                    image_for_paint,
                                    0,
                                    false,
                                ) {
                                    eprintln!("rich-gpui: failed to paint equation image: {error}");
                                }
                            },
                        )
                        .size_full(),
                    ),
                );
            if let Some(error) = inline_error {
                column = column.child(
                    div()
                        .px(px(pt_to_px(6.0, zoom)))
                        .py(px(pt_to_px(2.0, zoom)))
                        .rounded(px(pt_to_px(3.0, zoom)))
                        .bg(error_background(theme))
                        .text_size(px(pt_to_px(8.5, zoom)))
                        .text_color(error_foreground(theme))
                        .child(error.to_string()),
                );
            }
            column.into_any_element()
        }
        EquationVisual::Loading => div()
            .text_size(px(pt_to_px(9.5, zoom)))
            .text_color(theme.muted_text)
            .child("Rendering equation…")
            .into_any_element(),
        EquationVisual::Error(error) => div()
            .px(px(pt_to_px(8.0, zoom)))
            .py(px(pt_to_px(4.0, zoom)))
            .rounded(px(pt_to_px(4.0, zoom)))
            .bg(error_background(theme))
            .text_size(px(pt_to_px(9.0, zoom)))
            .text_color(error_foreground(theme))
            .child(format!("LaTeX error: {error}"))
            .into_any_element(),
    };

    let mut row = div()
        .relative()
        .size_full()
        .px(px(pad_x))
        .py(px(pad_y))
        .flex()
        .items_center()
        .justify_center()
        .child(center_content);

    if let Some(tag) = numbering_tag {
        row = row.child(
            div()
                .absolute()
                .right(px(pad_x))
                .text_size(px(pt_to_px(10.0, zoom)))
                .text_color(theme.muted_text)
                .child(tag.to_owned()),
        );
    }

    row.into_any_element()
}

fn style_header_pill(
    id: (&'static str, u64),
    zoom: f32,
    background: Hsla,
    border: Hsla,
    foreground: Hsla,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .h(px(pt_to_px(15.0, zoom)))
        .px(px(pt_to_px(6.5, zoom)))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(pt_to_px(3.5, zoom)))
        .border_1()
        .border_color(border)
        .bg(background)
        .text_size(px(pt_to_px(8.0, zoom)))
        .text_color(foreground)
        .cursor_pointer()
}

fn build_block_edit_header(
    block_id: BlockId,
    source: &str,
    copied: bool,
    zoom: f32,
    theme: &RichTheme,
    state: Rc<RefCell<EquationRuntimeState>>,
    parent_view: WeakEntity<RichDocumentView>,
) -> AnyElement {
    let header_h = pt_to_px(BLOCK_HEADER_HEIGHT_PT, zoom);
    let pad_x = pt_to_px(10.0, zoom);

    let copy_source = source.to_owned();
    let copy_state = Rc::clone(&state);
    let copy_parent = parent_view.clone();
    let (copy_bg, copy_border, copy_fg, copy_label) = if copied {
        (
            theme.accent.alpha(0.14),
            theme.accent,
            theme.accent,
            "Copied",
        )
    } else {
        (
            theme.surface_background,
            theme.code_border,
            theme.muted_text,
            "Copy LaTeX",
        )
    };
    let copy_button = style_header_pill(
        ("equation-copy", block_id.0),
        zoom,
        copy_bg,
        copy_border,
        copy_fg,
    )
    .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(copy_source.clone()));
        let state_weak = Rc::downgrade(&copy_state);
        let parent_weak = copy_parent.clone();
        let reset_task = cx.spawn(async move |async_app: &mut AsyncApp| {
            async_app
                .background_executor()
                .timer(COPY_FEEDBACK_DURATION)
                .await;
            async_app.update(|cx| {
                if let Some(state_rc) = state_weak.upgrade() {
                    let mut runtime = state_rc.borrow_mut();
                    if runtime.copied_block == Some(block_id) {
                        runtime.copied_block = None;
                        runtime.copy_reset_task = None;
                    }
                }
                if let Some(view) = parent_weak.upgrade() {
                    view.update(cx, |_view, cx| cx.notify());
                }
            });
        });
        {
            let mut runtime = copy_state.borrow_mut();
            runtime.copied_block = Some(block_id);
            runtime.copy_reset_task = Some(reset_task);
        }
        if let Some(view) = copy_parent.upgrade() {
            view.update(cx, |_view, cx| cx.notify());
        }
        cx.stop_propagation();
    })
    .child(copy_label);

    let done_parent = parent_view;
    let done_button = style_header_pill(
        ("equation-done", block_id.0),
        zoom,
        theme.accent,
        theme.accent,
        theme.accent_foreground,
    )
    .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
        if let Some(view) = done_parent.upgrade() {
            view.update(cx, |view, cx| {
                view.dispatch_extension_command(
                    SetEquationViewMode {
                        block_id,
                        mode: EquationViewMode::Rendered,
                    },
                    cx,
                );
                window.focus(&view.focus_handle, cx);
            });
        }
        cx.stop_propagation();
    })
    .child("Done");

    div()
        .w_full()
        .h(px(header_h))
        .px(px(pad_x))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(theme.code_border)
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .text_size(px(pt_to_px(8.5, zoom)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_text)
                .child("LaTeX Equation"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(pt_to_px(5.0, zoom)))
                .child(copy_button)
                .child(done_button),
        )
        .into_any_element()
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
