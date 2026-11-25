//! Built-in GPUI Mermaid diagram extension implementing [`ExtensionPlugin`] and
//! [`BlockRenderer`] for [`MermaidSpec`] with two-tier async SVG compilation and rasterization,
//! stale-while-revalidate zooming, host-supplied font rendering, live split editing, and direct
//! in-place diagram text editing.

mod editor;

use self::editor::{MermaidLabelEditor, MermaidSourceEditor};
use crate::gpui::completion::{
    CompletionAccept, CompletionBlockScope, CompletionDetail, CompletionItem, CompletionProvider,
    CompletionQueryContext, CompletionQueryResult, CompletionTrigger, fuzzy_match_label,
};
use crate::gpui::extension::{
    BlockRenderContext, BlockRenderer, CustomRenderOutput, ExtensionPlugin,
    ExtensionRegistrationError, ReadCommandOutcome,
};
use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::layout::{Size, pt_to_px};
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, DocumentSelection, HistoryMergeGroup,
    InsertMermaidDiagram, InteractionPolicy, MermaidBackend, MermaidBlockStatus,
    MermaidBlockStatusQuery, MermaidData, MermaidRenderError, MermaidRenderStatus, MermaidSpec,
    MermaidSvgLabel, MermaidThemePalette, MermaidViewMode, RgbaColor, SetMermaidCaption,
    SetMermaidViewMode, Step, SvgViewBox, ToggleMermaidViewMode, Transaction, UpdateMermaidSource,
    extract_svg_labels, find_editable_mermaid_label, hash_mermaid_source,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext, AssetSource, AsyncApp, ClipboardItem, Context, Corners, DevicePixels, Entity,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, ParentElement,
    ParsedSvg, RenderImage, Size as GpuiSize, Styled, SvgRenderer, SvgSize, Task, WeakEntity,
    Window, canvas, div, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const CARD_HEADER_HEIGHT_PT: f32 = 26.0;
const CARD_PAD_X_PT: f32 = 12.0;
const CARD_PAD_TOP_PT: f32 = 8.0;
const CARD_PAD_BOTTOM_PT: f32 = 10.0;
const CAPTION_HEIGHT_PT: f32 = 20.0;
const PLACEHOLDER_BODY_HEIGHT_PT: f32 = 160.0;
const ERROR_BODY_HEIGHT_PT: f32 = 112.0;
const EDITOR_LINE_HEIGHT_PT: f32 = 19.5;
const EDITOR_VERT_PADDING_PT: f32 = 18.0;
const SPLIT_MIN_SIDE_BY_SIDE_WIDTH_PT: f32 = 360.0;
const SPLIT_MIN_BODY_HEIGHT_PT: f32 = 180.0;
const SPLIT_DIVIDER_HIT_PT: f32 = 7.0;
const DEFAULT_SPLIT_RATIO: f32 = 0.5;
const MIN_SPLIT_RATIO: f32 = 0.25;
const MAX_SPLIT_RATIO: f32 = 0.75;
const DEFAULT_MAX_HEIGHT_PT: f32 = 480.0;
const DEFAULT_CACHE_CAPACITY: usize = 64;
const COPY_FEEDBACK_DURATION: Duration = Duration::from_millis(1400);
const LIVE_COMPILE_DEBOUNCE: Duration = Duration::from_millis(120);

/// Configuration options for [`MermaidExtension`].
#[derive(Clone)]
pub struct MermaidConfig {
    /// Optional host-supplied [`AssetSource`] used by GPUI's [`SvgRenderer`] for SVG font loading.
    ///
    /// When `None`, the extension uses `cx.svg_renderer()` backed by the application's configured
    /// [`AssetSource`].
    pub asset_source: Option<Arc<dyn AssetSource>>,
    /// Default maximum height (in typographic points) for rendered diagram bodies when a block
    /// does not specify `MermaidData::max_height_pt`.
    pub default_max_height_pt: f32,
    /// Maximum number of entries retained in each tier of the LRU compile and raster caches.
    pub cache_capacity: usize,
    /// Registers a built-in `/mermaid` slash-completion item when installing [`MermaidExtension`].
    pub register_slash_completion: bool,
}

impl Default for MermaidConfig {
    fn default() -> Self {
        Self {
            asset_source: None,
            default_max_height_pt: DEFAULT_MAX_HEIGHT_PT,
            cache_capacity: DEFAULT_CACHE_CAPACITY,
            register_slash_completion: true,
        }
    }
}

impl MermaidConfig {
    /// Creates a [`MermaidConfig`] with default sizing and cache capacity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the [`AssetSource`] used to construct GPUI's [`SvgRenderer`].
    pub fn with_asset_source(mut self, asset_source: Arc<dyn AssetSource>) -> Self {
        self.asset_source = Some(asset_source);
        self
    }

    /// Sets the default maximum diagram body height in typographic points.
    pub fn with_default_max_height_pt(mut self, max_height_pt: f32) -> Self {
        self.default_max_height_pt = max_height_pt.max(64.0);
        self
    }

    /// Sets the per-tier LRU cache capacity.
    pub fn with_cache_capacity(mut self, cache_capacity: usize) -> Self {
        self.cache_capacity = cache_capacity.max(4);
        self
    }

    /// Enables or disables automatic `/mermaid` slash completion registration.
    pub const fn with_slash_completion(mut self, enabled: bool) -> Self {
        self.register_slash_completion = enabled;
        self
    }
}

/// Built-in Mermaid diagram [`ExtensionPlugin`] registering [`MermaidSpec`] rendering, editing
/// commands, view-mode toggles, and status queries onto a [`RichDocumentView`].
pub struct MermaidExtension {
    backend: Arc<dyn MermaidBackend>,
    config: MermaidConfig,
}

impl MermaidExtension {
    /// Creates a built-in [`MermaidExtension`] using [`crate::model::BuiltInMermaidBackend`].
    #[cfg(feature = "mermaid")]
    pub fn builtin() -> Self {
        Self::with_backend(crate::model::BuiltInMermaidBackend::new())
    }

    /// Creates a [`MermaidExtension`] backed by a custom [`MermaidBackend`].
    pub fn with_backend(backend: impl MermaidBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            config: MermaidConfig::default(),
        }
    }

    /// Creates a [`MermaidExtension`] from a shared [`MermaidBackend`] trait object.
    pub fn with_shared_backend(backend: Arc<dyn MermaidBackend>) -> Self {
        Self {
            backend,
            config: MermaidConfig::default(),
        }
    }

    /// Applies custom [`MermaidConfig`] options.
    pub fn with_config(mut self, config: MermaidConfig) -> Self {
        self.config = config;
        self
    }
}

impl ExtensionPlugin for MermaidExtension {
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
        let state = Rc::new(RefCell::new(MermaidRuntimeState::new(
            self.backend,
            svg_renderer,
            self.config,
            cx.entity().downgrade(),
        )));

        view.register_block_renderer(
            MermaidBlockRenderer {
                state: Rc::clone(&state),
            },
            cx,
        )?;

        view.register_edit_command::<InsertMermaidDiagram>(|command, context| {
            let mut data = MermaidData::new(command.source.clone());
            data.caption = command.caption.clone();
            let block_id = context.reserve_block_id();
            let block = BlockNode::new(
                block_id,
                BlockKind::leaf::<MermaidSpec>(data),
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

        view.register_edit_command::<UpdateMermaidSource>(|command, context| {
            let block = context.document().find_block(command.block_id)?;
            let mut data = block.kind.read_data::<MermaidSpec>().ok()?.into_owned();
            if data.source == command.source {
                return None;
            }
            data.source = command.source.clone();
            context.set_history_group(HistoryMergeGroup::Typing {
                block_id: command.block_id,
            });
            Some(Transaction::new(vec![
                Step::update_block_data::<MermaidSpec>(command.block_id, data),
            ]))
        })?;

        view.register_edit_command::<SetMermaidCaption>(|command, context| {
            let block = context.document().find_block(command.block_id)?;
            let mut data = block.kind.read_data::<MermaidSpec>().ok()?.into_owned();
            if data.caption == command.caption {
                return None;
            }
            data.caption = command.caption.clone();
            Some(Transaction::new(vec![
                Step::update_block_data::<MermaidSpec>(command.block_id, data),
            ]))
        })?;

        let set_mode_state = Rc::clone(&state);
        view.register_read_command::<SetMermaidViewMode>(move |command, context| {
            let Some(block) = context.document().find_block(command.block_id) else {
                return ReadCommandOutcome::NoOp;
            };
            if block.kind.read_data::<MermaidSpec>().is_err() {
                return ReadCommandOutcome::NoOp;
            }
            let mut runtime = set_mode_state.borrow_mut();
            let current = runtime.view_mode(command.block_id);
            if current == command.mode {
                return ReadCommandOutcome::NoOp;
            }
            runtime.set_view_mode(command.block_id, command.mode);
            context.invalidate_block_layout(command.block_id);
            ReadCommandOutcome::Executed
        })?;

        let toggle_mode_state = Rc::clone(&state);
        view.register_read_command::<ToggleMermaidViewMode>(move |command, context| {
            let Some(block) = context.document().find_block(command.block_id) else {
                return ReadCommandOutcome::NoOp;
            };
            if block.kind.read_data::<MermaidSpec>().is_err() {
                return ReadCommandOutcome::NoOp;
            }
            let mut runtime = toggle_mode_state.borrow_mut();
            let next = match runtime.view_mode(command.block_id) {
                MermaidViewMode::Diagram => MermaidViewMode::Source,
                MermaidViewMode::Split | MermaidViewMode::Source => MermaidViewMode::Diagram,
            };
            runtime.set_view_mode(command.block_id, next);
            context.invalidate_block_layout(command.block_id);
            ReadCommandOutcome::Executed
        })?;

        let query_state = Rc::clone(&state);
        view.register_query::<MermaidBlockStatusQuery>(move |query, context| {
            let block = context.document().find_block(query.block_id)?;
            let data = block.kind.read_data::<MermaidSpec>().ok()?;
            let runtime = query_state.borrow();
            Some(runtime.block_status(query.block_id, &data.source))
        })?;

        if register_slash {
            view.register_completion_provider(MermaidSlashCompletionProvider::new())?;
        }

        Ok(())
    }
}

struct MermaidSlashCompletionProvider {
    triggers: [CompletionTrigger; 1],
}

impl MermaidSlashCompletionProvider {
    fn new() -> Self {
        Self {
            triggers: [CompletionTrigger::prefix("/")],
        }
    }
}

impl CompletionProvider for MermaidSlashCompletionProvider {
    fn id(&self) -> &'static str {
        "gpui_rich.mermaid.slash"
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
        const DEFAULT_DIAGRAM: &str = "flowchart LR\n    Start[Start] --> Done[Done]";
        let label = "Mermaid Diagram";
        let matched = fuzzy_match_label(context.query, label).or_else(|| {
            fuzzy_match_label(context.query, "flowchart").map(|(score, _)| (score - 10, Vec::new()))
        });
        let Some((score, indices)) = matched else {
            return CompletionQueryResult::Ready(Vec::new());
        };
        let item = CompletionItem::new(
            "mermaid-diagram",
            label,
            CompletionAccept::command(crate::model::RichCommand::extension(
                InsertMermaidDiagram::new(DEFAULT_DIAGRAM),
            )),
        )
        .with_subtitle("Insert an interactive Mermaid diagram")
        .with_badge("diagram")
        .with_category("Media")
        .with_match(score, indices)
        .with_detail(
            CompletionDetail::new(
                "Mermaid Diagram",
                "Inserts a live-editable diagram block supporting flowchart, sequence, and state syntax.",
            )
            .with_preview(DEFAULT_DIAGRAM),
        );
        CompletionQueryResult::Ready(vec![item])
    }
}

impl RichDocumentView {
    /// Installs the built-in Mermaid diagram extension with default configuration.
    #[cfg(feature = "mermaid")]
    pub fn install_mermaid(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(MermaidExtension::builtin(), cx)
    }

    /// Installs a configured [`MermaidExtension`] onto this view.
    pub fn install_mermaid_with(
        &mut self,
        extension: MermaidExtension,
        cx: &mut Context<Self>,
    ) -> Result<(), ExtensionRegistrationError> {
        self.install_extension(extension, cx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CompileKey {
    source_hash: u64,
    theme_hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RasterKey {
    source_hash: u64,
    theme_hash: u64,
    target_width_dev: u32,
    target_height_dev: u32,
}

enum CompiledDiagram {
    Ready {
        intrinsic_width: f32,
        intrinsic_height: f32,
        view_box: SvgViewBox,
        labels: Vec<MermaidSvgLabel>,
        parsed_svg: Arc<ParsedSvg>,
    },
    Error(MermaidRenderError),
}

struct CompileCacheEntry {
    last_used: u64,
    diagram: CompiledDiagram,
}

struct RasterCacheEntry {
    last_used: u64,
    image: Arc<RenderImage>,
}

#[derive(Clone)]
struct LastValidDiagram {
    image: Arc<RenderImage>,
    intrinsic_width: f32,
    intrinsic_height: f32,
    view_box: SvgViewBox,
    labels: Vec<MermaidSvgLabel>,
}

struct ActiveLabelEdit {
    block_id: BlockId,
    label_text: String,
    occurrence: usize,
    editor: Entity<MermaidLabelEditor>,
}

#[derive(Clone, Copy, Debug)]
struct SplitDragState {
    block_id: BlockId,
    start_x: f32,
    start_ratio: f32,
    container_width: f32,
}

enum BackgroundRenderOutcome {
    Ready {
        compile_key: CompileKey,
        raster_key: RasterKey,
        intrinsic_width: f32,
        intrinsic_height: f32,
        view_box: SvgViewBox,
        labels: Vec<MermaidSvgLabel>,
        parsed_svg: Arc<ParsedSvg>,
        image: Arc<RenderImage>,
    },
    Error {
        compile_key: CompileKey,
        error: MermaidRenderError,
    },
}

struct MermaidRuntimeState {
    backend: Arc<dyn MermaidBackend>,
    svg_renderer: SvgRenderer,
    config: MermaidConfig,
    parent_view: WeakEntity<RichDocumentView>,
    clock: u64,
    compile_cache: HashMap<CompileKey, CompileCacheEntry>,
    raster_cache: HashMap<RasterKey, RasterCacheEntry>,
    in_flight: HashMap<BlockId, (RasterKey, Task<()>)>,
    last_valid_diagram: HashMap<BlockId, LastValidDiagram>,
    block_modes: HashMap<BlockId, MermaidViewMode>,
    block_split_ratios: HashMap<BlockId, f32>,
    split_drag: Option<SplitDragState>,
    block_theme_hash: HashMap<BlockId, u64>,
    block_measured_height: HashMap<BlockId, f32>,
    draft_editors: HashMap<BlockId, Entity<MermaidSourceEditor>>,
    active_label_edit: Option<ActiveLabelEdit>,
    copied_block: Option<BlockId>,
    copy_reset_task: Option<Task<()>>,
}

impl MermaidRuntimeState {
    fn new(
        backend: Arc<dyn MermaidBackend>,
        svg_renderer: SvgRenderer,
        config: MermaidConfig,
        parent_view: WeakEntity<RichDocumentView>,
    ) -> Self {
        Self {
            backend,
            svg_renderer,
            config,
            parent_view,
            clock: 1,
            compile_cache: HashMap::new(),
            raster_cache: HashMap::new(),
            in_flight: HashMap::new(),
            last_valid_diagram: HashMap::new(),
            block_modes: HashMap::new(),
            block_split_ratios: HashMap::new(),
            split_drag: None,
            block_theme_hash: HashMap::new(),
            block_measured_height: HashMap::new(),
            draft_editors: HashMap::new(),
            active_label_edit: None,
            copied_block: None,
            copy_reset_task: None,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn view_mode(&self, block_id: BlockId) -> MermaidViewMode {
        self.block_modes
            .get(&block_id)
            .copied()
            .unwrap_or(MermaidViewMode::Diagram)
    }

    fn set_view_mode(&mut self, block_id: BlockId, mode: MermaidViewMode) {
        if mode == MermaidViewMode::Diagram {
            self.block_modes.remove(&block_id);
            self.draft_editors.remove(&block_id);
        } else {
            self.block_modes.insert(block_id, mode);
        }
        if mode == MermaidViewMode::Source
            && self
                .active_label_edit
                .as_ref()
                .is_some_and(|edit| edit.block_id == block_id)
        {
            self.active_label_edit = None;
        }
        if mode != MermaidViewMode::Split
            && self
                .split_drag
                .is_some_and(|drag| drag.block_id == block_id)
        {
            self.split_drag = None;
        }
    }

    fn split_ratio(&self, block_id: BlockId) -> f32 {
        self.block_split_ratios
            .get(&block_id)
            .copied()
            .unwrap_or(DEFAULT_SPLIT_RATIO)
            .clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
    }

    fn set_split_ratio(&mut self, block_id: BlockId, ratio: f32) {
        let clamped = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
        self.block_split_ratios.insert(block_id, clamped);
    }

    fn lookup_compiled(&mut self, key: CompileKey) -> Option<&CompiledDiagram> {
        let tick = self.next_tick();
        let entry = self.compile_cache.get_mut(&key)?;
        entry.last_used = tick;
        Some(&entry.diagram)
    }

    fn peek_compiled(&self, key: CompileKey) -> Option<&CompiledDiagram> {
        self.compile_cache.get(&key).map(|entry| &entry.diagram)
    }

    fn insert_compiled(&mut self, key: CompileKey, diagram: CompiledDiagram) {
        let tick = self.next_tick();
        self.compile_cache.insert(
            key,
            CompileCacheEntry {
                last_used: tick,
                diagram,
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

    fn stale_raster_for(&self, source_hash: u64, theme_hash: u64) -> Option<Arc<RenderImage>> {
        self.raster_cache
            .iter()
            .filter(|(key, _)| key.source_hash == source_hash && key.theme_hash == theme_hash)
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

    fn block_status(&self, block_id: BlockId, source: &str) -> MermaidBlockStatus {
        let mode = self.view_mode(block_id);
        let source_hash = hash_mermaid_source(source);
        let theme_hash = self
            .block_theme_hash
            .get(&block_id)
            .copied()
            .unwrap_or_else(|| MermaidThemePalette::light().fingerprint());
        let compile_key = CompileKey {
            source_hash,
            theme_hash,
        };
        let render = if let Some(compiled) = self.peek_compiled(compile_key) {
            match compiled {
                CompiledDiagram::Ready {
                    intrinsic_width,
                    intrinsic_height,
                    ..
                } => MermaidRenderStatus::Ready {
                    intrinsic_width: *intrinsic_width,
                    intrinsic_height: *intrinsic_height,
                },
                CompiledDiagram::Error(error) => MermaidRenderStatus::Error(error.clone()),
            }
        } else if self.in_flight.contains_key(&block_id) {
            MermaidRenderStatus::Rendering
        } else {
            MermaidRenderStatus::Idle
        };
        MermaidBlockStatus { mode, render }
    }
}

struct MermaidBlockRenderer {
    state: Rc<RefCell<MermaidRuntimeState>>,
}

fn measure_source_body_height(source: &str, pane_width: f32, zoom: f32) -> f32 {
    let pad_x = pt_to_px(CARD_PAD_X_PT, zoom);
    let inner_w = (pane_width - 2.0 * pad_x).max(40.0);
    let chars_per_line = (inner_w / pt_to_px(6.1, zoom)).floor().max(16.0) as usize;
    let visual_lines = source
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(chars_per_line))
        .sum::<usize>()
        .clamp(3, 18) as f32;
    pt_to_px(EDITOR_VERT_PADDING_PT, zoom) + visual_lines * pt_to_px(EDITOR_LINE_HEIGHT_PT, zoom)
}

fn measure_diagram_body_height(
    runtime: &MermaidRuntimeState,
    block_id: BlockId,
    data: &MermaidData,
    pane_width: f32,
    zoom: f32,
    preserve_last_valid_on_error: bool,
) -> f32 {
    let pad_x = pt_to_px(CARD_PAD_X_PT, zoom);
    let pad_top = pt_to_px(CARD_PAD_TOP_PT, zoom);
    let pad_bottom = pt_to_px(CARD_PAD_BOTTOM_PT, zoom);
    let caption_h = if data
        .caption
        .as_deref()
        .is_some_and(|caption| !caption.trim().is_empty())
    {
        pt_to_px(CAPTION_HEIGHT_PT, zoom)
    } else {
        0.0
    };

    let source_hash = hash_mermaid_source(&data.source);
    let theme_hash = runtime
        .block_theme_hash
        .get(&block_id)
        .copied()
        .unwrap_or_else(|| MermaidThemePalette::light().fingerprint());
    let max_body_w = (pane_width - 2.0 * pad_x).max(48.0);
    let max_body_h = pt_to_px(
        data.max_height_pt
            .unwrap_or(runtime.config.default_max_height_pt)
            .max(64.0),
        zoom,
    );

    let body_h = match runtime.peek_compiled(CompileKey {
        source_hash,
        theme_hash,
    }) {
        Some(CompiledDiagram::Ready {
            intrinsic_width,
            intrinsic_height,
            ..
        }) => {
            let (_, fitted_h) = fit_diagram_size(
                *intrinsic_width,
                *intrinsic_height,
                max_body_w,
                max_body_h,
                zoom,
            );
            fitted_h
        }
        Some(CompiledDiagram::Error(_)) => {
            if preserve_last_valid_on_error
                && let Some(last_valid) = runtime.last_valid_diagram.get(&block_id)
            {
                let (_, fitted_h) = fit_diagram_size(
                    last_valid.intrinsic_width,
                    last_valid.intrinsic_height,
                    max_body_w,
                    max_body_h,
                    zoom,
                );
                fitted_h + pt_to_px(22.0, zoom)
            } else {
                pt_to_px(ERROR_BODY_HEIGHT_PT, zoom)
            }
        }
        None => {
            if let Some(last_valid) = runtime.last_valid_diagram.get(&block_id) {
                let (_, fitted_h) = fit_diagram_size(
                    last_valid.intrinsic_width,
                    last_valid.intrinsic_height,
                    max_body_w,
                    max_body_h,
                    zoom,
                );
                fitted_h
            } else {
                pt_to_px(PLACEHOLDER_BODY_HEIGHT_PT, zoom)
            }
        }
    };

    pad_top + body_h + pad_bottom + caption_h
}

impl BlockRenderer for MermaidBlockRenderer {
    type Spec = MermaidSpec;

    fn measure_leaf(
        &self,
        block_id: BlockId,
        data: &MermaidData,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        let mut runtime = self.state.borrow_mut();
        let width = max_width.max(120.0);
        let header_h = pt_to_px(CARD_HEADER_HEIGHT_PT, zoom);

        let body_height = match runtime.view_mode(block_id) {
            MermaidViewMode::Source => measure_source_body_height(&data.source, width, zoom),
            MermaidViewMode::Diagram => {
                measure_diagram_body_height(&runtime, block_id, data, width, zoom, false)
            }
            MermaidViewMode::Split => {
                let side_by_side = width >= pt_to_px(SPLIT_MIN_SIDE_BY_SIDE_WIDTH_PT, zoom);
                if side_by_side {
                    let divider_w = pt_to_px(SPLIT_DIVIDER_HIT_PT, zoom);
                    let ratio = runtime.split_ratio(block_id);
                    let left_w = ((width - divider_w) * ratio).max(100.0);
                    let right_w = (width - divider_w - left_w).max(100.0);
                    let source_h = measure_source_body_height(&data.source, left_w, zoom);
                    let diagram_h =
                        measure_diagram_body_height(&runtime, block_id, data, right_w, zoom, true);
                    source_h
                        .max(diagram_h)
                        .max(pt_to_px(SPLIT_MIN_BODY_HEIGHT_PT, zoom))
                } else {
                    let source_h = measure_source_body_height(&data.source, width, zoom);
                    let diagram_h =
                        measure_diagram_body_height(&runtime, block_id, data, width, zoom, true);
                    source_h + diagram_h + 1.0
                }
            }
        };

        let height = header_h + body_height;
        runtime.block_measured_height.insert(block_id, height);
        Size::new(
            width,
            height.max(pt_to_px(CARD_HEADER_HEIGHT_PT + 32.0, zoom)),
        )
    }

    fn render(
        &self,
        context: BlockRenderContext<'_, MermaidData>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        let block_id = context.block_id;
        let zoom = context.zoom;
        let theme = context.theme;
        let palette = palette_from_rich_theme(theme);
        let theme_hash = palette.fingerprint();
        let source_hash = hash_mermaid_source(&context.data.source);
        let compile_key = CompileKey {
            source_hash,
            theme_hash,
        };

        let (policy, parent_view, mode, copied, split_ratio) = {
            let mut runtime = self.state.borrow_mut();
            runtime.block_theme_hash.insert(block_id, theme_hash);
            let parent = runtime.parent_view.clone();
            let policy = parent
                .upgrade()
                .map(|view| view.read(cx).policy())
                .unwrap_or_else(InteractionPolicy::viewer);
            let mode = runtime.view_mode(block_id);
            let copied = runtime.copied_block == Some(block_id);
            let split_ratio = runtime.split_ratio(block_id);
            (policy, parent, mode, copied, split_ratio)
        };

        let needs_source_editor = matches!(mode, MermaidViewMode::Source | MermaidViewMode::Split);
        let editor_entity = if needs_source_editor {
            let (entity, newly_created) = {
                let mut runtime = self.state.borrow_mut();
                if let Some(existing) = runtime.draft_editors.get(&block_id).cloned() {
                    (existing, false)
                } else {
                    let initial_source = context.data.source.clone();
                    let initial_theme = theme.clone();
                    let editable = policy.editing.enabled;
                    let parent_for_editor = parent_view.clone();
                    let entity = cx.new(|cx| {
                        MermaidSourceEditor::new(
                            block_id,
                            &initial_source,
                            &initial_theme,
                            editable,
                            zoom,
                            parent_for_editor,
                            cx,
                        )
                    });
                    runtime.draft_editors.insert(block_id, entity.clone());
                    (entity, true)
                }
            };
            entity.update(cx, |editor, cx| {
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
            Some(entity)
        } else {
            None
        };

        let active_label_edit = {
            let runtime = self.state.borrow();
            runtime
                .active_label_edit
                .as_ref()
                .filter(|edit| edit.block_id == block_id)
                .map(|edit| {
                    (
                        edit.label_text.clone(),
                        edit.occurrence,
                        edit.editor.clone(),
                    )
                })
        };
        if let Some((_, _, label_editor)) = &active_label_edit {
            label_editor.update(cx, |editor, cx| {
                editor.sync(theme, zoom, cx);
            });
        }

        let total_w: f32 = context.bounds.size.width.into();
        let scale_factor = window.scale_factor().max(1.0);
        let pad_x = pt_to_px(CARD_PAD_X_PT, zoom);
        let max_body_h = pt_to_px(
            context
                .data
                .max_height_pt
                .unwrap_or_else(|| self.state.borrow().config.default_max_height_pt)
                .max(64.0),
            zoom,
        );

        let body = match mode {
            MermaidViewMode::Source => {
                let editor_entity = editor_entity.clone()?;
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .child(editor_entity)
                    .into_any_element()
            }
            MermaidViewMode::Diagram => {
                let max_body_w = (total_w - 2.0 * pad_x).max(48.0);
                let render_visual = self.prepare_diagram_visual(
                    block_id,
                    &context.data.source,
                    palette,
                    compile_key,
                    max_body_w,
                    max_body_h,
                    zoom,
                    scale_factor,
                    false,
                    cx,
                );
                build_diagram_body(
                    block_id,
                    &context.data.source,
                    context.data.caption.as_deref(),
                    render_visual,
                    policy,
                    zoom,
                    theme,
                    Rc::clone(&self.state),
                    parent_view.clone(),
                    active_label_edit,
                )
            }
            MermaidViewMode::Split => {
                let editor_entity = editor_entity.clone()?;
                let side_by_side = total_w >= pt_to_px(SPLIT_MIN_SIDE_BY_SIDE_WIDTH_PT, zoom);
                let divider_w = pt_to_px(SPLIT_DIVIDER_HIT_PT, zoom);

                let (left_w, diagram_pane_w) = if side_by_side {
                    let usable_w = (total_w - divider_w).max(200.0);
                    let lw = (usable_w * split_ratio).max(100.0);
                    let rw = (usable_w - lw).max(100.0);
                    (lw, rw)
                } else {
                    (total_w, total_w)
                };

                let max_body_w = (diagram_pane_w - 2.0 * pad_x).max(48.0);
                let render_visual = self.prepare_diagram_visual(
                    block_id,
                    &context.data.source,
                    palette,
                    compile_key,
                    max_body_w,
                    max_body_h,
                    zoom,
                    scale_factor,
                    true,
                    cx,
                );
                let diagram_pane = build_diagram_body(
                    block_id,
                    &context.data.source,
                    context.data.caption.as_deref(),
                    render_visual,
                    policy,
                    zoom,
                    theme,
                    Rc::clone(&self.state),
                    parent_view.clone(),
                    active_label_edit,
                );

                if side_by_side {
                    let drag_start_state = Rc::clone(&self.state);
                    let drag_move_state = Rc::clone(&self.state);
                    let drag_move_parent = parent_view.clone();
                    let drag_end_state = Rc::clone(&self.state);
                    let drag_end_out_state = Rc::clone(&self.state);
                    let reset_state = Rc::clone(&self.state);
                    let reset_parent = parent_view.clone();
                    let container_usable_w = (total_w - divider_w).max(200.0);

                    let divider = div()
                        .id(("mermaid-split-divider", block_id.0))
                        .w(px(divider_w))
                        .h_full()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_col_resize()
                        .hover(|style| style.bg(theme.accent.alpha(0.10)))
                        .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                            if event.click_count >= 2 {
                                reset_state
                                    .borrow_mut()
                                    .set_split_ratio(block_id, DEFAULT_SPLIT_RATIO);
                                if let Some(view) = reset_parent.upgrade() {
                                    view.update(cx, |view, cx| {
                                        view.invalidate_block_layout(block_id, cx);
                                    });
                                }
                            } else {
                                let start_x: f32 = event.position.x.into();
                                let mut runtime = drag_start_state.borrow_mut();
                                let start_ratio = runtime.split_ratio(block_id);
                                runtime.split_drag = Some(SplitDragState {
                                    block_id,
                                    start_x,
                                    start_ratio,
                                    container_width: container_usable_w,
                                });
                            }
                            cx.stop_propagation();
                        })
                        .child(div().w(px(1.0)).h_full().bg(theme.code_border));

                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .flex()
                        .flex_row()
                        .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                            let drag = {
                                let runtime = drag_move_state.borrow();
                                runtime
                                    .split_drag
                                    .filter(|active| active.block_id == block_id)
                            };
                            let Some(drag) = drag else {
                                return;
                            };
                            if event.pressed_button != Some(MouseButton::Left) {
                                drag_move_state.borrow_mut().split_drag = None;
                                return;
                            }
                            let current_x: f32 = event.position.x.into();
                            let delta_ratio =
                                (current_x - drag.start_x) / drag.container_width.max(1.0);
                            let next_ratio = (drag.start_ratio + delta_ratio)
                                .clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                            let changed = {
                                let mut runtime = drag_move_state.borrow_mut();
                                let prev = runtime.split_ratio(block_id);
                                if (prev - next_ratio).abs() > 0.004 {
                                    runtime.set_split_ratio(block_id, next_ratio);
                                    true
                                } else {
                                    false
                                }
                            };
                            if changed && let Some(view) = drag_move_parent.upgrade() {
                                view.update(cx, |view, cx| {
                                    view.invalidate_block_layout(block_id, cx);
                                });
                            }
                        })
                        .on_mouse_up(MouseButton::Left, move |_event, _window, _cx| {
                            let mut runtime = drag_end_state.borrow_mut();
                            if runtime
                                .split_drag
                                .is_some_and(|active| active.block_id == block_id)
                            {
                                runtime.split_drag = None;
                            }
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_event, _window, _cx| {
                            let mut runtime = drag_end_out_state.borrow_mut();
                            if runtime
                                .split_drag
                                .is_some_and(|active| active.block_id == block_id)
                            {
                                runtime.split_drag = None;
                            }
                        })
                        .child(
                            div()
                                .w(px(left_w))
                                .h_full()
                                .flex_none()
                                .overflow_hidden()
                                .child(editor_entity),
                        )
                        .child(divider)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .h_full()
                                .overflow_hidden()
                                .bg(theme.surface_background.alpha(0.35))
                                .child(diagram_pane),
                        )
                        .into_any_element()
                } else {
                    let source_h = measure_source_body_height(&context.data.source, total_w, zoom);
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .w_full()
                                .h(px(source_h))
                                .flex_none()
                                .overflow_hidden()
                                .border_b_1()
                                .border_color(theme.code_border)
                                .child(editor_entity),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .w_full()
                                .overflow_hidden()
                                .bg(theme.surface_background.alpha(0.35))
                                .child(diagram_pane),
                        )
                        .into_any_element()
                }
            }
        };

        let header = build_header_bar(
            block_id,
            &context.data.source,
            mode,
            copied,
            policy,
            zoom,
            theme,
            Rc::clone(&self.state),
            parent_view,
            editor_entity,
        );

        let card = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .rounded(px(pt_to_px(5.0, zoom)))
            .border_1()
            .border_color(theme.code_border)
            .bg(theme.code_background)
            .child(header)
            .child(body)
            .into_any_element();

        Some(CustomRenderOutput::Element(card))
    }

    fn plain_text(&self, data: &MermaidData) -> Option<String> {
        Some(format!("```mermaid\n{}\n```", data.source))
    }

    fn html(&self, data: &MermaidData) -> Option<String> {
        let escaped_source = escape_html(&data.source);
        match data
            .caption
            .as_deref()
            .filter(|caption| !caption.trim().is_empty())
        {
            Some(caption) => Some(format!(
                "<figure data-mermaid=\"true\"><pre><code class=\"language-mermaid\">{escaped_source}</code></pre><figcaption>{}</figcaption></figure>",
                escape_html(caption)
            )),
            None => Some(format!(
                "<figure data-mermaid=\"true\"><pre><code class=\"language-mermaid\">{escaped_source}</code></pre></figure>"
            )),
        }
    }
}

enum DiagramVisual {
    Image {
        image: Arc<RenderImage>,
        display_width: f32,
        display_height: f32,
        view_box: SvgViewBox,
        labels: Vec<MermaidSvgLabel>,
        inline_error: Option<MermaidRenderError>,
    },
    Loading,
    Error(MermaidRenderError),
}

impl MermaidBlockRenderer {
    #[allow(clippy::too_many_arguments)]
    fn prepare_diagram_visual(
        &self,
        block_id: BlockId,
        source: &str,
        palette: MermaidThemePalette,
        compile_key: CompileKey,
        max_body_w: f32,
        max_body_h: f32,
        zoom: f32,
        scale_factor: f32,
        live_split_mode: bool,
        cx: &mut App,
    ) -> DiagramVisual {
        let mut runtime = self.state.borrow_mut();

        let (compiled_parsed, display_width, display_height, view_box, labels) =
            match runtime.lookup_compiled(compile_key) {
                Some(CompiledDiagram::Error(error)) => {
                    let error = error.clone();
                    if live_split_mode
                        && let Some(last_valid) = runtime.last_valid_diagram.get(&block_id).cloned()
                    {
                        let (w, h) = fit_diagram_size(
                            last_valid.intrinsic_width,
                            last_valid.intrinsic_height,
                            max_body_w,
                            max_body_h,
                            zoom,
                        );
                        return DiagramVisual::Image {
                            image: last_valid.image,
                            display_width: w,
                            display_height: h,
                            view_box: last_valid.view_box,
                            labels: last_valid.labels,
                            inline_error: Some(error),
                        };
                    }
                    return DiagramVisual::Error(error);
                }
                Some(CompiledDiagram::Ready {
                    intrinsic_width,
                    intrinsic_height,
                    view_box,
                    labels,
                    parsed_svg,
                }) => {
                    let (w, h) = fit_diagram_size(
                        *intrinsic_width,
                        *intrinsic_height,
                        max_body_w,
                        max_body_h,
                        zoom,
                    );
                    (
                        Some((
                            Arc::clone(parsed_svg),
                            *intrinsic_width,
                            *intrinsic_height,
                            *view_box,
                            labels.clone(),
                        )),
                        w,
                        h,
                        Some(*view_box),
                        Some(labels.clone()),
                    )
                }
                None => (
                    None,
                    max_body_w,
                    pt_to_px(PLACEHOLDER_BODY_HEIGHT_PT, zoom),
                    None,
                    None,
                ),
            };

        let target_width_dev = quantize_device_px(display_width * scale_factor);
        let target_height_dev = ((display_height * scale_factor).round() as u32).max(16);
        let raster_key = RasterKey {
            source_hash: compile_key.source_hash,
            theme_hash: compile_key.theme_hash,
            target_width_dev,
            target_height_dev,
        };

        if let Some(image) = runtime.lookup_raster(raster_key) {
            let resolved_view_box = view_box
                .unwrap_or_else(|| SvgViewBox::new(0.0, 0.0, display_width, display_height));
            let resolved_labels = labels.unwrap_or_default();
            if let Some((_, intrinsic_width, intrinsic_height, vb, lbls)) = compiled_parsed {
                runtime.last_valid_diagram.insert(
                    block_id,
                    LastValidDiagram {
                        image: Arc::clone(&image),
                        intrinsic_width,
                        intrinsic_height,
                        view_box: vb,
                        labels: lbls,
                    },
                );
            }
            return DiagramVisual::Image {
                image,
                display_width,
                display_height,
                view_box: resolved_view_box,
                labels: resolved_labels,
                inline_error: None,
            };
        }

        // Keep the most recent RenderImage visible during zoom, resize, or live typing while the
        // updated diagram compiles and rasterizes asynchronously.
        let stale_same_source =
            runtime.stale_raster_for(compile_key.source_hash, compile_key.theme_hash);
        let last_valid_fallback = runtime.last_valid_diagram.get(&block_id).cloned();

        let already_in_flight = runtime
            .in_flight
            .get(&block_id)
            .is_some_and(|(active_key, _)| *active_key == raster_key);
        if !already_in_flight {
            let backend = Arc::clone(&runtime.backend);
            let svg_renderer = runtime.svg_renderer.clone();
            let owned_source = source.to_owned();
            let state_weak = Rc::downgrade(&self.state);
            let parent_view = runtime.parent_view.clone();
            let debounce =
                if live_split_mode && compiled_parsed.is_none() && last_valid_fallback.is_some() {
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
                            palette,
                            compile_key,
                            raster_key,
                            max_body_w,
                            max_body_h,
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
                        runtime.in_flight.remove(&block_id);
                        match outcome {
                            BackgroundRenderOutcome::Ready {
                                compile_key,
                                raster_key,
                                intrinsic_width,
                                intrinsic_height,
                                view_box,
                                labels,
                                parsed_svg,
                                image,
                            } => {
                                let was_compiled = runtime.peek_compiled(compile_key).is_some();
                                runtime.last_valid_diagram.insert(
                                    block_id,
                                    LastValidDiagram {
                                        image: Arc::clone(&image),
                                        intrinsic_width,
                                        intrinsic_height,
                                        view_box,
                                        labels: labels.clone(),
                                    },
                                );
                                runtime.insert_compiled(
                                    compile_key,
                                    CompiledDiagram::Ready {
                                        intrinsic_width,
                                        intrinsic_height,
                                        view_box,
                                        labels,
                                        parsed_svg,
                                    },
                                );
                                runtime.insert_raster(raster_key, image);
                                !was_compiled
                            }
                            BackgroundRenderOutcome::Error { compile_key, error } => {
                                runtime.insert_compiled(compile_key, CompiledDiagram::Error(error));
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

            runtime.in_flight.insert(block_id, (raster_key, task));
        }

        if let Some(image) = stale_same_source {
            return DiagramVisual::Image {
                image,
                display_width,
                display_height,
                view_box: view_box
                    .unwrap_or_else(|| SvgViewBox::new(0.0, 0.0, display_width, display_height)),
                labels: labels.unwrap_or_default(),
                inline_error: None,
            };
        }

        if let Some(last_valid) = last_valid_fallback {
            let (w, h) = fit_diagram_size(
                last_valid.intrinsic_width,
                last_valid.intrinsic_height,
                max_body_w,
                max_body_h,
                zoom,
            );
            return DiagramVisual::Image {
                image: last_valid.image,
                display_width: w,
                display_height: h,
                view_box: last_valid.view_box,
                labels: last_valid.labels,
                inline_error: None,
            };
        }

        DiagramVisual::Loading
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run_background_compile_and_raster(
    existing_parsed: Option<(Arc<ParsedSvg>, f32, f32, SvgViewBox, Vec<MermaidSvgLabel>)>,
    source: String,
    palette: MermaidThemePalette,
    compile_key: CompileKey,
    initial_raster_key: RasterKey,
    max_body_w: f32,
    max_body_h: f32,
    zoom: f32,
    scale_factor: f32,
    backend: Arc<dyn MermaidBackend>,
    svg_renderer: SvgRenderer,
) -> BackgroundRenderOutcome {
    let (parsed_svg, intrinsic_width, intrinsic_height, view_box, labels) = match existing_parsed {
        Some(existing) => existing,
        None => {
            let svg = match backend.render_svg(&source, &palette) {
                Ok(svg) => svg,
                Err(error) => {
                    return BackgroundRenderOutcome::Error { compile_key, error };
                }
            };
            let (view_box, labels) =
                extract_svg_labels(&svg.svg, svg.intrinsic_width, svg.intrinsic_height);
            let parsed = match svg_renderer.parse_svg(svg.svg.as_bytes()) {
                Ok(parsed) => Arc::new(parsed),
                Err(error) => {
                    return BackgroundRenderOutcome::Error {
                        compile_key,
                        error: MermaidRenderError::new(format!(
                            "Failed to parse compiled Mermaid SVG: {error}"
                        )),
                    };
                }
            };
            (
                parsed,
                svg.intrinsic_width,
                svg.intrinsic_height,
                view_box,
                labels,
            )
        }
    };

    let (fitted_w, fitted_h) = fit_diagram_size(
        intrinsic_width,
        intrinsic_height,
        max_body_w,
        max_body_h,
        zoom,
    );
    let target_width_dev = quantize_device_px(fitted_w * scale_factor);
    let target_height_dev = ((fitted_h * scale_factor).round() as u32).max(16);
    let raster_key = RasterKey {
        source_hash: initial_raster_key.source_hash,
        theme_hash: initial_raster_key.theme_hash,
        target_width_dev,
        target_height_dev,
    };

    let device_size = GpuiSize {
        width: DevicePixels(target_width_dev.min(i32::MAX as u32) as i32),
        height: DevicePixels(target_height_dev.min(i32::MAX as u32) as i32),
    };
    match svg_renderer.render_parsed(&parsed_svg, SvgSize::ExactSize(device_size)) {
        Ok(image) => BackgroundRenderOutcome::Ready {
            compile_key,
            raster_key,
            intrinsic_width,
            intrinsic_height,
            view_box,
            labels,
            parsed_svg,
            image,
        },
        Err(error) => BackgroundRenderOutcome::Error {
            compile_key,
            error: MermaidRenderError::new(format!("Failed to rasterize Mermaid SVG: {error}")),
        },
    }
}

fn style_code_pill(
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

#[allow(clippy::too_many_arguments)]
fn build_header_bar(
    block_id: BlockId,
    source: &str,
    mode: MermaidViewMode,
    copied: bool,
    policy: InteractionPolicy,
    zoom: f32,
    theme: &RichTheme,
    state: Rc<RefCell<MermaidRuntimeState>>,
    parent_view: WeakEntity<RichDocumentView>,
    editor_entity: Option<Entity<MermaidSourceEditor>>,
) -> impl IntoElement {
    let show_copy = policy.can_copy_code();
    let show_modes = policy.editing.enabled || policy.can_copy_code();
    let owned_source = source.to_owned();
    let copy_state = Rc::clone(&state);
    let copy_parent = parent_view.clone();
    let copy_editor = editor_entity;

    let mode_pill = |id_prefix: &'static str,
                     label: &'static str,
                     target_mode: MermaidViewMode,
                     active: bool| {
        let (bg, border, fg) = if active {
            (theme.accent.alpha(0.14), theme.accent, theme.accent)
        } else {
            (
                theme.surface_background,
                theme.code_border,
                theme.muted_text,
            )
        };
        let parent_for_mode = parent_view.clone();
        style_code_pill((id_prefix, block_id.0), zoom, bg, border, fg)
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                if let Some(view) = parent_for_mode.upgrade() {
                    view.update(cx, |view, cx| {
                        view.dispatch_extension_command(
                            SetMermaidViewMode {
                                block_id,
                                mode: target_mode,
                            },
                            cx,
                        );
                    });
                }
                cx.stop_propagation();
            })
            .child(label)
    };

    let show_divider = mode != MermaidViewMode::Diagram;

    div()
        .w_full()
        .h(px(pt_to_px(CARD_HEADER_HEIGHT_PT, zoom)))
        .flex_none()
        .px(px(pt_to_px(10.0, zoom)))
        .flex()
        .items_center()
        .justify_between()
        .when(show_divider, |bar| {
            bar.border_b_1().border_color(theme.code_border)
        })
        .child(
            div()
                .text_size(px(pt_to_px(8.5, zoom)))
                .line_height(px(pt_to_px(8.5 * 1.25, zoom)))
                .text_color(theme.muted_text)
                .child("mermaid"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(pt_to_px(4.0, zoom)))
                .when(show_modes, |row| {
                    row.child(mode_pill(
                        "mermaid-mode-diagram",
                        "Diagram",
                        MermaidViewMode::Diagram,
                        mode == MermaidViewMode::Diagram,
                    ))
                    .child(mode_pill(
                        "mermaid-mode-split",
                        "Split",
                        MermaidViewMode::Split,
                        mode == MermaidViewMode::Split,
                    ))
                    .child(mode_pill(
                        "mermaid-mode-source",
                        "Source",
                        MermaidViewMode::Source,
                        mode == MermaidViewMode::Source,
                    ))
                })
                .when(show_copy, |row| {
                    let (bg, border, fg, label) = if copied {
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
                            "Copy",
                        )
                    };
                    row.child(
                        style_code_pill(("mermaid-copy", block_id.0), zoom, bg, border, fg)
                            .min_w(px(pt_to_px(42.0, zoom)))
                            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                                let text = copy_editor
                                    .as_ref()
                                    .map(|editor| editor.read(cx).current_source(cx))
                                    .unwrap_or_else(|| owned_source.clone());
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                let weak_state = Rc::downgrade(&copy_state);
                                let parent_for_timer = copy_parent.clone();
                                let reset_task = cx.spawn(async move |async_app: &mut AsyncApp| {
                                    async_app
                                        .background_executor()
                                        .timer(COPY_FEEDBACK_DURATION)
                                        .await;
                                    async_app.update(|cx| {
                                        if let Some(state_rc) = weak_state.upgrade() {
                                            let mut runtime = state_rc.borrow_mut();
                                            if runtime.copied_block == Some(block_id) {
                                                runtime.copied_block = None;
                                            }
                                            runtime.copy_reset_task = None;
                                        }
                                        if let Some(view) = parent_for_timer.upgrade() {
                                            view.update(cx, |_, cx| cx.notify());
                                        }
                                    });
                                });
                                {
                                    let mut runtime = copy_state.borrow_mut();
                                    runtime.copied_block = Some(block_id);
                                    runtime.copy_reset_task = Some(reset_task);
                                }
                                if let Some(view) = copy_parent.upgrade() {
                                    view.update(cx, |_, cx| cx.notify());
                                }
                                cx.stop_propagation();
                            })
                            .child(label),
                    )
                }),
        )
}

#[allow(clippy::too_many_arguments)]
fn build_diagram_body(
    block_id: BlockId,
    source: &str,
    caption: Option<&str>,
    visual: DiagramVisual,
    policy: InteractionPolicy,
    zoom: f32,
    theme: &RichTheme,
    state: Rc<RefCell<MermaidRuntimeState>>,
    parent_view: WeakEntity<RichDocumentView>,
    active_label_edit: Option<(String, usize, Entity<MermaidLabelEditor>)>,
) -> gpui_kit::AnyElement {
    let pad_x = pt_to_px(CARD_PAD_X_PT, zoom);
    let pad_top = pt_to_px(CARD_PAD_TOP_PT, zoom);
    let pad_bottom = pt_to_px(CARD_PAD_BOTTOM_PT, zoom);
    let clean_caption = caption
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);

    let mut footer_error: Option<MermaidRenderError> = None;

    let main_content = match visual {
        DiagramVisual::Image {
            image,
            display_width,
            display_height,
            view_box,
            labels,
            inline_error,
        } => {
            footer_error = inline_error;
            let can_direct_edit = policy.editing.enabled && footer_error.is_none();

            let paint_element = canvas(
                |_bounds, _window, _cx| (),
                move |bounds, (), window, _cx| {
                    if let Err(error) = window.paint_image(
                        bounds,
                        bounds,
                        Corners::all(px(0.0)),
                        Arc::clone(&image),
                        0,
                        false,
                    ) {
                        eprintln!(
                            "rich-gpui: failed to paint Mermaid diagram {block_id:?}: {error}"
                        );
                    }
                },
            )
            .w(px(display_width))
            .h(px(display_height));

            let mut label_overlays: Vec<gpui_kit::AnyElement> = Vec::new();
            if can_direct_edit {
                let scale_x = display_width / view_box.width.max(1.0);
                let scale_y = display_height / view_box.height.max(1.0);
                let hit_pad = pt_to_px(2.5, zoom);

                for (label_index, label) in labels.iter().enumerate() {
                    let Some(matched) =
                        find_editable_mermaid_label(source, &label.text, label.occurrence)
                    else {
                        continue;
                    };

                    let left = ((label.x - view_box.x) * scale_x - hit_pad)
                        .clamp(0.0, (display_width - 12.0).max(0.0));
                    let top = ((label.y - view_box.y) * scale_y - hit_pad)
                        .clamp(0.0, (display_height - 12.0).max(0.0));
                    let rect_w = (label.width * scale_x + hit_pad * 2.0)
                        .clamp(16.0, (display_width - left).max(16.0));
                    let rect_h = (label.height * scale_y + hit_pad * 2.0)
                        .clamp(16.0, (display_height - top).max(16.0));

                    let is_active =
                        active_label_edit
                            .as_ref()
                            .is_some_and(|(active_text, active_occ, _)| {
                                *active_text == label.text && *active_occ == label.occurrence
                            });

                    if is_active && let Some((_, _, active_editor)) = &active_label_edit {
                        let min_editor_w = pt_to_px(108.0, zoom);
                        let editor_w = (rect_w + pt_to_px(28.0, zoom))
                            .max(min_editor_w)
                            .min(display_width.max(min_editor_w));
                        let editor_h = rect_h.max(pt_to_px(22.0, zoom));
                        let center_x = left + rect_w * 0.5;
                        let center_y = top + rect_h * 0.5;
                        let editor_left = (center_x - editor_w * 0.5)
                            .clamp(0.0, (display_width - editor_w).max(0.0));
                        let editor_top = (center_y - editor_h * 0.5)
                            .clamp(0.0, (display_height - editor_h).max(0.0));

                        label_overlays.push(
                            div()
                                .absolute()
                                .left(px(editor_left))
                                .top(px(editor_top))
                                .w(px(editor_w))
                                .h(px(editor_h))
                                .child(active_editor.clone())
                                .into_any_element(),
                        );
                    } else {
                        let click_state = Rc::clone(&state);
                        let click_parent = parent_view.clone();
                        let click_theme = theme.clone();
                        let original_label = label.text.clone();
                        let initial_text = matched.display_text;
                        let occurrence = label.occurrence;
                        let element_id = block_id
                            .0
                            .wrapping_mul(1024)
                            .wrapping_add(label_index as u64);

                        label_overlays.push(
                            div()
                                .id(("mermaid-label", element_id))
                                .absolute()
                                .left(px(left))
                                .top(px(top))
                                .w(px(rect_w))
                                .h(px(rect_h))
                                .rounded(px(pt_to_px(3.0, zoom)))
                                .border_1()
                                .border_color(Hsla::transparent_black())
                                .cursor_text()
                                .hover(|style| {
                                    style
                                        .bg(theme.accent.alpha(0.12))
                                        .border_color(theme.accent.alpha(0.55))
                                })
                                .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                                    let previous_editor = {
                                        click_state
                                            .borrow_mut()
                                            .active_label_edit
                                            .take()
                                            .map(|edit| edit.editor)
                                    };
                                    if let Some(previous) = previous_editor {
                                        previous.update(cx, |editor, cx| {
                                            editor.commit(false, window, cx);
                                        });
                                    }

                                    let dismiss_state = Rc::downgrade(&click_state);
                                    let on_dismiss = Box::new(move |_cx: &mut App| {
                                        if let Some(state_rc) = dismiss_state.upgrade() {
                                            state_rc.borrow_mut().active_label_edit = None;
                                        }
                                    });
                                    let label_for_edit = original_label.clone();
                                    let text_for_edit = initial_text.clone();
                                    let theme_for_edit = click_theme.clone();
                                    let parent_for_edit = click_parent.clone();
                                    let editor = cx.new(|cx| {
                                        MermaidLabelEditor::new(
                                            block_id,
                                            label_for_edit,
                                            text_for_edit,
                                            occurrence,
                                            &theme_for_edit,
                                            zoom,
                                            parent_for_edit,
                                            on_dismiss,
                                            window,
                                            cx,
                                        )
                                    });
                                    click_state.borrow_mut().active_label_edit =
                                        Some(ActiveLabelEdit {
                                            block_id,
                                            label_text: original_label.clone(),
                                            occurrence,
                                            editor,
                                        });
                                    if let Some(view) = click_parent.upgrade() {
                                        view.update(cx, |_, cx| cx.notify());
                                    }
                                    cx.stop_propagation();
                                })
                                .into_any_element(),
                        );
                    }
                }
            }

            let diagram_stage = div()
                .relative()
                .w(px(display_width))
                .h(px(display_height))
                .child(paint_element)
                .children(label_overlays);

            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(diagram_stage)
                .into_any_element()
        }
        DiagramVisual::Loading => div()
            .flex_1()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(pt_to_px(9.5, zoom)))
            .text_color(theme.muted_text)
            .child("Rendering Mermaid diagram...")
            .into_any_element(),
        DiagramVisual::Error(error) => {
            let snippet = source.lines().take(4).collect::<Vec<_>>().join("\n");
            let split_parent = parent_view.clone();
            div()
                .flex_1()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(pt_to_px(6.0, zoom)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(pt_to_px(8.0, zoom)))
                        .child(
                            div()
                                .text_size(px(pt_to_px(9.0, zoom)))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(format!("Mermaid error: {error}")),
                        )
                        .when(policy.editing.enabled, |row| {
                            row.child(
                                style_code_pill(
                                    ("mermaid-error-split", block_id.0),
                                    zoom,
                                    theme.surface_background,
                                    theme.code_border,
                                    theme.muted_text,
                                )
                                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                                    if let Some(view) = split_parent.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.dispatch_extension_command(
                                                SetMermaidViewMode {
                                                    block_id,
                                                    mode: MermaidViewMode::Split,
                                                },
                                                cx,
                                            );
                                        });
                                    }
                                    cx.stop_propagation();
                                })
                                .child("Open Split Editor"),
                            )
                        }),
                )
                .child(
                    div()
                        .w_full()
                        .p(px(pt_to_px(6.0, zoom)))
                        .rounded(px(pt_to_px(3.5, zoom)))
                        .border_1()
                        .border_color(theme.code_border)
                        .bg(theme.surface_background)
                        .text_size(px(pt_to_px(8.5, zoom)))
                        .text_color(theme.muted_text)
                        .child(snippet),
                )
                .into_any_element()
        }
    };

    div()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .px(px(pad_x))
        .pt(px(pad_top))
        .pb(px(pad_bottom))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(pt_to_px(4.0, zoom)))
        .child(main_content)
        .when_some(footer_error, |container, error| {
            container.child(
                div()
                    .w_full()
                    .px(px(pt_to_px(6.0, zoom)))
                    .py(px(pt_to_px(3.0, zoom)))
                    .rounded(px(pt_to_px(3.0, zoom)))
                    .border_1()
                    .border_color(theme.code_border)
                    .bg(theme.surface_background)
                    .text_size(px(pt_to_px(8.0, zoom)))
                    .text_color(theme.muted_text)
                    .child(format!("Mermaid error: {error}")),
            )
        })
        .when_some(clean_caption, |container, text| {
            container.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_size(px(pt_to_px(8.5, zoom)))
                    .text_color(theme.muted_text)
                    .child(text),
            )
        })
        .into_any_element()
}

fn fit_diagram_size(
    intrinsic_width: f32,
    intrinsic_height: f32,
    max_width: f32,
    max_height: f32,
    zoom: f32,
) -> (f32, f32) {
    let safe_w = intrinsic_width.max(1.0);
    let safe_h = intrinsic_height.max(1.0);
    let desired_w = safe_w * zoom;
    let desired_h = safe_h * zoom;
    let scale = (max_width / desired_w).min(max_height / desired_h).min(1.0);
    ((desired_w * scale).max(16.0), (desired_h * scale).max(16.0))
}

fn quantize_device_px(raw: f32) -> u32 {
    let clamped = raw.clamp(16.0, 8192.0).round() as u32;
    clamped.div_ceil(16) * 16
}

fn palette_from_rich_theme(theme: &RichTheme) -> MermaidThemePalette {
    MermaidThemePalette {
        dark: theme.is_dark(),
        background: hsla_to_rgba(theme.code_background),
        surface: hsla_to_rgba(theme.surface_background),
        text: hsla_to_rgba(theme.text),
        muted_text: hsla_to_rgba(theme.muted_text),
        border: hsla_to_rgba(theme.code_border),
        accent: hsla_to_rgba(theme.accent),
    }
}

fn hsla_to_rgba(color: Hsla) -> RgbaColor {
    let rgba = color.to_rgb();
    let to_u8 = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    RgbaColor::rgba(to_u8(rgba.r), to_u8(rgba.g), to_u8(rgba.b), to_u8(rgba.a))
}

fn escape_html(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for character in raw.chars() {
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
