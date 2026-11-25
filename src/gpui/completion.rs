//! Layer B: IDE-style caret completion and action menu controller built on top of Layer A
//! anchored overlays ([`crate::gpui::overlay`]).
//!
//! Supports prefix triggers (`/` slash commands, `@` mentions, `:` emoji), word-prefix
//! auto-triggers (such as symbol completion in code blocks), and manual invocation (`Ctrl+Space`),
//! with multi-provider merging, sync-first results with race-free async updates, opt-in side
//! detail cards, opt-in inline ghost-text previews, and atomic Undo acceptance.

use crate::gpui::extension::{ExtensionNamespace, ExtensionRegistrationError};
use crate::gpui::overlay::{
    COMPLETION_MENU_OVERLAY_ID, OverlayAnchor, OverlayDismissPolicy, OverlayId, OverlayKeyAction,
    OverlayKeyResult, OverlayPlacement, OverlayRenderContext, OverlaySpec, OverlayZLayer,
};
use crate::gpui::theme::RichTheme;
use crate::gpui::view::RichDocumentView;
use crate::model::{
    Affinity, BlockId, BlockKind, CommandEffectKind, CommandOutcome, CommandResolution,
    CommandSource, Document, DocumentSelection, InlineObject, MoveDirection, MoveUnit, RichCommand,
    SharedStr,
};
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, StatefulInteractiveElement, Styled, WeakEntity, Window, div, point,
    px,
};
use std::ops::Range;
use std::rc::Rc;

const MAX_VISIBLE_ROWS: usize = 7;

/// Identifier returned when registering a [`CompletionProvider`] on [`RichDocumentView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompletionProviderId(pub(crate) u64);

/// Monotonic identifier for an active completion session, used to reject stale async updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompletionSessionId(pub u64);

/// Boundary requirement immediately preceding a prefix trigger (such as `/` or `@`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixBoundary {
    /// Only triggers at the start of a block, after a newline, or after whitespace.
    LineStartOrWhitespace,
    /// Only triggers at byte offset 0 of a text-bearing leaf block.
    BlockStartOnly,
    /// Triggers at any character boundary.
    Any,
}

/// Trigger rule activating a [`CompletionProvider`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionTrigger {
    /// Fires when the user types `prefix` at `boundary` and tracks the following query text.
    Prefix {
        prefix: SharedStr,
        boundary: PrefixBoundary,
        allow_spaces: bool,
    },
    /// Fires automatically after typing at least `min_chars` contiguous identifier characters.
    WordPrefix { min_chars: usize },
    /// Fires only when explicitly invoked (for example via `Ctrl+Space` or `trigger_completion`).
    Manual,
}

impl CompletionTrigger {
    /// Creates a prefix trigger matching at block start or after whitespace, disallowing spaces.
    pub fn prefix(prefix: impl Into<SharedStr>) -> Self {
        Self::Prefix {
            prefix: prefix.into(),
            boundary: PrefixBoundary::LineStartOrWhitespace,
            allow_spaces: false,
        }
    }

    /// Creates a prefix trigger that allows spaces in the query (for example `@Alice Ch`).
    pub fn prefix_with_spaces(prefix: impl Into<SharedStr>) -> Self {
        Self::Prefix {
            prefix: prefix.into(),
            boundary: PrefixBoundary::LineStartOrWhitespace,
            allow_spaces: true,
        }
    }

    /// Creates a word-prefix auto-trigger activating after `min_chars` word characters.
    pub const fn word_prefix(min_chars: usize) -> Self {
        Self::WordPrefix { min_chars }
    }
}

/// Resolved trigger kind active for the current [`CompletionQueryContext`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionTriggerKind {
    Prefix(SharedStr),
    WordPrefix,
    Manual,
}

/// Block kinds where a [`CompletionProvider`] is eligible to activate.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum CompletionBlockScope {
    /// Eligible in every text-bearing leaf block.
    #[default]
    All,
    /// Eligible only outside `BlockKind::CodeBlock`.
    ProseOnly,
    /// Eligible only inside `BlockKind::CodeBlock` (optionally filtered case-insensitively by language).
    CodeBlocks { language: Option<SharedStr> },
}

impl CompletionBlockScope {
    /// Returns true when `block_kind` satisfies this scope.
    pub fn matches(&self, block_kind: &BlockKind) -> bool {
        match self {
            Self::All => block_kind.inline_content().is_some(),
            Self::ProseOnly => {
                block_kind.inline_content().is_some() && block_kind.code_block_data().is_none()
            }
            Self::CodeBlocks { language } => {
                let Some(code) = block_kind.code_block_data() else {
                    return false;
                };
                match language {
                    None => true,
                    Some(expected) => code
                        .language
                        .as_deref()
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected.as_ref())),
                }
            }
        }
    }
}

/// Policy controlling whether `Tab` and/or `Enter` accept the highlighted completion item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionAcceptKeys {
    /// Only `Tab` accepts; `Enter` dismisses and passes through to the document.
    TabOnly,
    /// Both `Tab` and `Enter` accept immediately.
    TabOrEnter,
    /// `Tab` always accepts; `Enter` accepts only after explicit `Up`/`Down` navigation,
    /// otherwise `Enter` dismisses and inserts a normal block split.
    TabOrSelectedEnter,
}

/// Opt-in side detail preview card displayed alongside the completion menu for rich items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionDetail {
    pub title: String,
    pub description: String,
    pub preview: Option<String>,
}

impl CompletionDetail {
    /// Creates a side detail card with a title and description.
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            preview: None,
        }
    }

    /// Attaches an optional preview snippet box to the side detail card.
    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }
}

/// Action executed when a [`CompletionItem`] is accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionAccept {
    /// Replaces the active trigger+query range with plain text.
    ReplaceText(String),
    /// Replaces the active trigger+query range with an atomic inline object (such as a `SmartChip`).
    ReplaceWithInlineObject(InlineObject),
    /// Runs a built-in or typed [`crate::model::ExtensionCommand`], optionally deleting the
    /// trigger+query range first.
    ///
    /// When `delete_trigger` is `true` and `command` is an edit command, the range deletion and
    /// command mutation merge into a single atomic Undo step.
    RunCommand {
        command: RichCommand,
        delete_trigger: bool,
    },
}

impl CompletionAccept {
    /// Creates a plain-text replacement accept action.
    pub fn replace_text(text: impl Into<String>) -> Self {
        Self::ReplaceText(text.into())
    }

    /// Creates an inline-object replacement accept action.
    pub fn replace_inline_object(object: InlineObject) -> Self {
        Self::ReplaceWithInlineObject(object)
    }

    /// Runs `command` after deleting the trigger+query range (default for `/slash` commands).
    pub fn command(command: RichCommand) -> Self {
        Self::RunCommand {
            command,
            delete_trigger: true,
        }
    }

    /// Runs `command` without modifying document text (safe for viewer mode and cancellable dialogs).
    pub fn read_only_command(command: RichCommand) -> Self {
        Self::RunCommand {
            command,
            delete_trigger: false,
        }
    }

    /// Returns true when accepting this item mutates the document.
    pub fn requires_editing(&self) -> bool {
        match self {
            Self::ReplaceText(_) | Self::ReplaceWithInlineObject(_) => true,
            Self::RunCommand {
                command,
                delete_trigger,
            } => *delete_trigger || command.effect() == CommandEffectKind::Edit,
        }
    }
}

/// Single candidate row displayed inside the caret completion menu.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub id: SharedStr,
    pub label: String,
    pub subtitle: Option<String>,
    pub badge: Option<String>,
    pub category: Option<String>,
    /// Optional leading emoji or glyph icon rendered beside the item label.
    pub icon: Option<SharedStr>,
    /// Character indices within [`Self::label`] highlighted as matched query text.
    pub match_indices: Vec<usize>,
    /// Ranking score (higher sorts first among merged provider candidates).
    pub score: i32,
    /// Opt-in side detail card rendered only while this item is highlighted.
    pub detail: Option<CompletionDetail>,
    /// Opt-in inline ghost-text suffix rendered at the caret while this item is highlighted.
    pub inline_preview: Option<String>,
    pub accept: CompletionAccept,
}

impl CompletionItem {
    /// Creates a completion item with `id`, `label`, and `accept` effect.
    pub fn new(
        id: impl Into<SharedStr>,
        label: impl Into<String>,
        accept: CompletionAccept,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            subtitle: None,
            badge: None,
            category: None,
            icon: None,
            match_indices: Vec::new(),
            score: 0,
            detail: None,
            inline_preview: None,
            accept,
        }
    }

    /// Sets the secondary inline subtitle shown below the item label.
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Sets the right-aligned badge or shortcut pill.
    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// Sets an optional category label.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Sets an optional leading emoji/glyph icon displayed beside the item label.
    pub fn with_icon(mut self, icon: impl Into<SharedStr>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Sets matched character indices and ranking score.
    pub fn with_match(mut self, score: i32, match_indices: Vec<usize>) -> Self {
        self.score = score;
        self.match_indices = match_indices;
        self
    }

    /// Sets the opt-in side detail preview card shown when highlighted.
    pub fn with_detail(mut self, detail: CompletionDetail) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Sets the opt-in inline ghost-text suffix painted at the caret when highlighted.
    pub fn with_inline_preview(mut self, suffix: impl Into<String>) -> Self {
        self.inline_preview = Some(suffix.into());
        self
    }
}

/// Context passed synchronously to [`CompletionProvider::query`].
pub struct CompletionQueryContext<'doc> {
    pub session_id: CompletionSessionId,
    pub query_revision: u64,
    pub block_id: BlockId,
    pub block_kind: &'doc BlockKind,
    pub trigger: CompletionTriggerKind,
    pub query: &'doc str,
    pub trigger_start: usize,
    pub caret_offset: usize,
    pub document: &'doc Document,
    pub selection: Option<&'doc DocumentSelection>,
}

/// Synchronous result returned by [`CompletionProvider::query`].
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionQueryResult {
    /// Complete synchronous item list.
    Ready(Vec<CompletionItem>),
    /// Immediate items plus loading state while an asynchronous task calls
    /// [`RichDocumentView::update_completion_items`].
    Pending { initial: Vec<CompletionItem> },
}

/// Read-only notification context passed to [`CompletionProvider::on_accept`] after the accepted
/// command has executed through [`RichDocumentView::dispatch_command`].
pub struct CompletionAcceptContext<'accept> {
    pub session_id: CompletionSessionId,
    pub block_id: BlockId,
    pub trigger: &'accept CompletionTriggerKind,
    pub item: &'accept CompletionItem,
}

/// Provider supplying caret completion or action menu candidates.
pub trait CompletionProvider: 'static {
    /// Stable dot-namespaced identifier (for example `"gpui_rich.mermaid.slash"`).
    fn id(&self) -> &'static str;

    /// Trigger rules that activate this provider.
    fn triggers(&self) -> &[CompletionTrigger];

    /// Block scope filter where this provider is eligible.
    fn block_scope(&self) -> CompletionBlockScope {
        CompletionBlockScope::All
    }

    /// Priority tie-breaker when merging multiple providers on the same trigger (higher first).
    fn priority(&self) -> i32 {
        0
    }

    /// Acceptance key policy for sessions driven by this provider.
    fn accept_keys(&self) -> CompletionAcceptKeys {
        CompletionAcceptKeys::TabOrEnter
    }

    /// Optional header title displayed at the top of the completion menu.
    fn header_title(&self) -> Option<&str> {
        None
    }

    /// Evaluates completion items synchronously, optionally spawning an async task targeting
    /// `(context.session_id, context.query_revision)` via `view`.
    fn query(
        &self,
        context: &CompletionQueryContext<'_>,
        view: WeakEntity<RichDocumentView>,
        cx: &mut App,
    ) -> CompletionQueryResult;

    /// Read-only post-accept hook invoked after an item from this provider executed cleanly
    /// through the command dispatcher (useful for updating provider-internal recency state).
    fn on_accept(&self, _context: &CompletionAcceptContext<'_>) {}
}

/// Merged completion entry tagged with its originating provider identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCompletionItem {
    pub provider_id: &'static str,
    pub provider_priority: i32,
    pub item: CompletionItem,
}

/// Read-only snapshot passed to a custom completion menu renderer.
pub struct CompletionMenuSnapshot<'menu> {
    pub session_id: CompletionSessionId,
    pub query_revision: u64,
    pub header_title: &'menu str,
    pub loading: bool,
    pub accept_keys: CompletionAcceptKeys,
    pub user_navigated: bool,
    pub items: &'menu [ResolvedCompletionItem],
    pub selected_index: usize,
}

/// Optional host callback overriding the default `RichTheme` completion menu UI.
pub type CompletionMenuRenderer = Rc<
    dyn Fn(
        &CompletionMenuSnapshot<'_>,
        &OverlayRenderContext<'_>,
        &mut Window,
        &mut App,
    ) -> AnyElement,
>;

/// Case-insensitive subsequence/prefix matcher returning `(score, matched_char_indices)`.
///
/// Empty `query` matches with score `0` and empty indices. Exact prefix matches receive the
/// highest boost, followed by word-boundary hits and contiguous runs.
pub fn fuzzy_match_label(query: &str, label: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query_chars.is_empty() {
        return Some((0, Vec::new()));
    }
    let label_chars: Vec<char> = label.chars().collect();
    let label_lower: Vec<char> = label_chars
        .iter()
        .map(|character| character.to_lowercase().next().unwrap_or(*character))
        .collect();

    let mut matched = Vec::with_capacity(query_chars.len());
    let mut query_pos = 0usize;
    let mut score = 0i32;
    let mut previous_match: Option<usize> = None;

    for (label_index, &lower_char) in label_lower.iter().enumerate() {
        if query_pos < query_chars.len() && lower_char == query_chars[query_pos] {
            matched.push(label_index);
            score += 10;
            if label_index == query_pos {
                score += 24;
            }
            if label_index == 0
                || label_chars
                    .get(label_index.saturating_sub(1))
                    .is_some_and(|prev| !prev.is_alphanumeric())
            {
                score += 16;
            }
            if previous_match.is_some_and(|prev| prev + 1 == label_index) {
                score += 12;
            }
            previous_match = Some(label_index);
            query_pos += 1;
        }
    }

    if query_pos != query_chars.len() {
        return None;
    }

    let length_penalty = (label_chars.len().saturating_sub(query_chars.len())).min(40) as i32;
    Some((score - length_penalty, matched))
}

/// Derives an inline ghost-text completion suffix when `completion` starts with `query`
/// (case-insensitively) and has remaining trailing characters.
pub fn inline_preview_suffix(query: &str, completion: &str) -> Option<String> {
    if query.is_empty() || completion.len() <= query.len() {
        return None;
    }
    let prefix = completion.get(..query.len())?;
    if !prefix.eq_ignore_ascii_case(query) {
        return None;
    }
    completion
        .get(query.len()..)
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetectedTrigger {
    pub trigger: CompletionTriggerKind,
    pub trigger_start: usize,
    pub query_start: usize,
    pub caret_offset: usize,
    pub allow_spaces: bool,
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn boundary_satisfied(text: &str, byte_offset: usize, boundary: PrefixBoundary) -> bool {
    match boundary {
        PrefixBoundary::Any => true,
        PrefixBoundary::BlockStartOnly => byte_offset == 0,
        PrefixBoundary::LineStartOrWhitespace => {
            if byte_offset == 0 {
                return true;
            }
            text.get(..byte_offset)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|character| character.is_whitespace() || character == '\n')
        }
    }
}

/// Scans `text` before `caret_offset` for a matching prefix trigger.
pub(crate) fn match_prefix_trigger(
    text: &str,
    caret_offset: usize,
    prefix: &str,
    boundary: PrefixBoundary,
    allow_spaces: bool,
) -> Option<DetectedTrigger> {
    if prefix.is_empty() || caret_offset > text.len() || !text.is_char_boundary(caret_offset) {
        return None;
    }
    let before_caret = text.get(..caret_offset)?;
    let search_start = before_caret.rfind('\n').map_or(0, |index| index + 1);
    let line_slice = before_caret.get(search_start..)?;

    let mut candidate_pos = line_slice.len();
    while let Some(found) = line_slice.get(..candidate_pos)?.rfind(prefix) {
        let absolute_trigger_start = search_start + found;
        let query_start = absolute_trigger_start + prefix.len();
        let query = text.get(query_start..caret_offset)?;
        let spaces_valid = if allow_spaces {
            !query.contains('\n') && !query.contains("  ") && query.chars().count() <= 48
        } else {
            !query.chars().any(char::is_whitespace)
        };
        if spaces_valid
            && !query.contains('\u{FFFC}')
            && boundary_satisfied(text, absolute_trigger_start, boundary)
        {
            return Some(DetectedTrigger {
                trigger: CompletionTriggerKind::Prefix(SharedStr::from(prefix)),
                trigger_start: absolute_trigger_start,
                query_start,
                caret_offset,
                allow_spaces,
            });
        }
        if found == 0 {
            break;
        }
        candidate_pos = found;
    }
    None
}

/// Scans `text` before `caret_offset` for a contiguous word prefix of at least `min_chars`.
pub(crate) fn match_word_prefix_trigger(
    text: &str,
    caret_offset: usize,
    min_chars: usize,
) -> Option<DetectedTrigger> {
    if min_chars == 0 || caret_offset > text.len() || !text.is_char_boundary(caret_offset) {
        return None;
    }
    let before_caret = text.get(..caret_offset)?;
    let mut start = caret_offset;
    let mut count = 0usize;
    for (byte_index, character) in before_caret.char_indices().rev() {
        if is_word_char(character) {
            start = byte_index;
            count += 1;
        } else {
            break;
        }
    }
    if count < min_chars {
        return None;
    }
    Some(DetectedTrigger {
        trigger: CompletionTriggerKind::WordPrefix,
        trigger_start: start,
        query_start: start,
        caret_offset,
        allow_spaces: false,
    })
}

/// Scans `text` before `caret_offset` for a manual completion anchor (current word prefix or caret).
pub(crate) fn match_manual_trigger(text: &str, caret_offset: usize) -> Option<DetectedTrigger> {
    if caret_offset > text.len() || !text.is_char_boundary(caret_offset) {
        return None;
    }
    let before_caret = text.get(..caret_offset)?;
    let mut start = caret_offset;
    for (byte_index, character) in before_caret.char_indices().rev() {
        if is_word_char(character) {
            start = byte_index;
        } else {
            break;
        }
    }
    Some(DetectedTrigger {
        trigger: CompletionTriggerKind::Manual,
        trigger_start: start,
        query_start: start,
        caret_offset,
        allow_spaces: false,
    })
}

pub(crate) struct RegisteredCompletionProvider {
    pub handle: CompletionProviderId,
    pub provider: Rc<dyn CompletionProvider>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderSlice {
    pub provider_id: &'static str,
    pub priority: i32,
    pub pending: bool,
    pub items: Vec<CompletionItem>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveCompletionSession {
    pub session_id: CompletionSessionId,
    pub query_revision: u64,
    pub block_id: BlockId,
    pub trigger: CompletionTriggerKind,
    pub trigger_start: usize,
    pub query_start: usize,
    pub caret_offset: usize,
    pub header_title: String,
    pub accept_keys: CompletionAcceptKeys,
    pub user_navigated: bool,
    pub slices: Vec<ProviderSlice>,
    pub merged_items: Vec<ResolvedCompletionItem>,
    pub selected_index: usize,
    pub scroll_offset: usize,
}

impl ActiveCompletionSession {
    pub(crate) fn is_loading(&self) -> bool {
        self.slices.iter().any(|slice| slice.pending)
    }

    pub(crate) fn replace_range(&self) -> Range<usize> {
        self.trigger_start..self.caret_offset
    }

    pub(crate) fn selected_item(&self) -> Option<&ResolvedCompletionItem> {
        self.merged_items.get(self.selected_index)
    }

    pub(crate) fn rebuild_merged_preserving_selection(
        &mut self,
        preserve_selection_key: Option<(&'static str, SharedStr)>,
    ) {
        let mut combined = Vec::new();
        for slice in &self.slices {
            for item in &slice.items {
                combined.push(ResolvedCompletionItem {
                    provider_id: slice.provider_id,
                    provider_priority: slice.priority,
                    item: item.clone(),
                });
            }
        }
        combined.sort_by(|left, right| {
            right
                .item
                .score
                .cmp(&left.item.score)
                .then_with(|| right.provider_priority.cmp(&left.provider_priority))
                .then_with(|| left.item.label.cmp(&right.item.label))
        });
        self.merged_items = combined;

        if self.merged_items.is_empty() {
            self.selected_index = 0;
            self.scroll_offset = 0;
            return;
        }

        if let Some((provider_id, item_id)) = preserve_selection_key
            && let Some(found_index) = self
                .merged_items
                .iter()
                .position(|entry| entry.provider_id == provider_id && entry.item.id == item_id)
        {
            self.selected_index = found_index;
            self.ensure_selected_in_window();
            return;
        }

        self.selected_index = self.selected_index.min(self.merged_items.len() - 1);
        self.ensure_selected_in_window();
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let len = self.merged_items.len();
        if len == 0 {
            return;
        }
        let next = if delta < 0 {
            let step = (-delta) as usize % len;
            (self.selected_index + len - step) % len
        } else {
            (self.selected_index + delta as usize) % len
        };
        self.selected_index = next;
        self.user_navigated = true;
        self.ensure_selected_in_window();
    }

    pub(crate) fn ensure_selected_in_window(&mut self) {
        let len = self.merged_items.len();
        if len <= MAX_VISIBLE_ROWS {
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + MAX_VISIBLE_ROWS {
            self.scroll_offset = self.selected_index + 1 - MAX_VISIBLE_ROWS;
        }
        let max_scroll = len.saturating_sub(MAX_VISIBLE_ROWS);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    /// Applies an async item update for `provider_id` when `(session_id, query_revision)` matches.
    pub(crate) fn apply_async_update(
        &mut self,
        session_id: CompletionSessionId,
        query_revision: u64,
        provider_id: &str,
        items: Vec<CompletionItem>,
        is_complete: bool,
    ) -> bool {
        if self.session_id != session_id || self.query_revision != query_revision {
            return false;
        }
        let previous_key = self
            .selected_item()
            .map(|entry| (entry.provider_id, entry.item.id.clone()));

        let Some(slice) = self
            .slices
            .iter_mut()
            .find(|slice| slice.provider_id == provider_id)
        else {
            return false;
        };
        slice.items = items;
        if is_complete {
            slice.pending = false;
        }
        self.rebuild_merged_preserving_selection(previous_key);
        true
    }
}

/// Coordinator holding registered completion providers, active session state, and optional menu renderer.
#[derive(Default)]
pub(crate) struct CompletionManager {
    pub providers: Vec<RegisteredCompletionProvider>,
    pub active_session: Option<ActiveCompletionSession>,
    pub custom_renderer: Option<CompletionMenuRenderer>,
    pub suppress_retrigger: bool,
    next_provider_id: u64,
    next_session_id: u64,
}

impl CompletionManager {
    pub(crate) fn register_provider(
        &mut self,
        provider: Rc<dyn CompletionProvider>,
    ) -> Result<CompletionProviderId, ExtensionRegistrationError> {
        let key = provider.id();
        if key.trim().is_empty() {
            return Err(ExtensionRegistrationError::EmptyKey {
                namespace: ExtensionNamespace::Completion,
            });
        }
        if self
            .providers
            .iter()
            .any(|registered| registered.provider.id() == key)
        {
            return Err(ExtensionRegistrationError::DuplicateKey {
                namespace: ExtensionNamespace::Completion,
                key,
            });
        }
        self.next_provider_id = self.next_provider_id.saturating_add(1);
        let handle = CompletionProviderId(self.next_provider_id);
        self.providers
            .push(RegisteredCompletionProvider { handle, provider });
        Ok(handle)
    }

    pub(crate) fn unregister_provider(&mut self, handle: CompletionProviderId) -> bool {
        let before = self.providers.len();
        self.providers
            .retain(|registered| registered.handle != handle);
        self.providers.len() != before
    }

    pub(crate) fn unregister_provider_by_key(&mut self, key: &str) -> bool {
        let before = self.providers.len();
        self.providers
            .retain(|registered| registered.provider.id() != key);
        self.providers.len() != before
    }

    pub(crate) fn allocate_session_id(&mut self) -> CompletionSessionId {
        self.next_session_id = self.next_session_id.saturating_add(1);
        CompletionSessionId(self.next_session_id)
    }
}

/// Evaluates whether a key action should be consumed by an active completion session.
pub(crate) fn should_accept_on_key(
    action: OverlayKeyAction,
    accept_keys: CompletionAcceptKeys,
    user_navigated: bool,
) -> bool {
    match action {
        OverlayKeyAction::Tab => true,
        OverlayKeyAction::Enter => match accept_keys {
            CompletionAcceptKeys::TabOnly => false,
            CompletionAcceptKeys::TabOrEnter => true,
            CompletionAcceptKeys::TabOrSelectedEnter => user_navigated,
        },
        _ => false,
    }
}

/// Builds the Layer A [`OverlaySpec`] mounting the active completion menu.
pub(crate) fn build_completion_overlay_spec(
    anchor_pos: crate::model::DocPos,
    custom_renderer: Option<CompletionMenuRenderer>,
) -> OverlaySpec {
    OverlaySpec::new(
        OverlayId::new(COMPLETION_MENU_OVERLAY_ID),
        OverlayAnchor::DocPos {
            pos: anchor_pos,
            affinity: Affinity::Downstream,
        },
        move |context, window, cx| {
            let session_opt = context.view.read(cx).completion.active_session.clone();
            let Some(session) = session_opt else {
                return div().into_any_element();
            };
            if let Some(renderer) = &custom_renderer {
                let snapshot = CompletionMenuSnapshot {
                    session_id: session.session_id,
                    query_revision: session.query_revision,
                    header_title: &session.header_title,
                    loading: session.is_loading(),
                    accept_keys: session.accept_keys,
                    user_navigated: session.user_navigated,
                    items: &session.merged_items,
                    selected_index: session.selected_index,
                };
                return renderer(&snapshot, context, window, cx);
            }
            build_default_completion_menu(&session, context.theme, context.view.clone(), window)
        },
    )
    .with_placement(OverlayPlacement::below_start())
    .with_z_layer(OverlayZLayer::Menu)
    .with_dismiss_policy(OverlayDismissPolicy::menu())
    .with_key_handler(|action, view, _window, cx| view.handle_completion_overlay_key(action, cx))
}

fn build_highlighted_label(label: &str, match_indices: &[usize], theme: &RichTheme) -> AnyElement {
    if match_indices.is_empty() {
        return div()
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .child(label.to_owned())
            .into_any_element();
    }

    let mut row = div().flex().items_center().text_size(px(12.5));
    let mut current_text = String::new();
    let mut current_matched = false;

    for (char_index, character) in label.chars().enumerate() {
        let is_matched =
            match_indices.binary_search(&char_index).is_ok() || match_indices.contains(&char_index);
        if current_text.is_empty() {
            current_matched = is_matched;
            current_text.push(character);
        } else if is_matched == current_matched {
            current_text.push(character);
        } else {
            let segment = std::mem::take(&mut current_text);
            row = row.child(
                div()
                    .font_weight(if current_matched {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if current_matched {
                        theme.accent
                    } else {
                        theme.text
                    })
                    .child(segment),
            );
            current_matched = is_matched;
            current_text.push(character);
        }
    }
    if !current_text.is_empty() {
        row = row.child(
            div()
                .font_weight(if current_matched {
                    FontWeight::BOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(if current_matched {
                    theme.accent
                } else {
                    theme.text
                })
                .child(current_text),
        );
    }
    row.into_any_element()
}

fn build_default_completion_menu(
    session: &ActiveCompletionSession,
    theme: &RichTheme,
    view: Entity<RichDocumentView>,
    window: &Window,
) -> AnyElement {
    let menu_width = px(296.0);
    let shadow = vec![BoxShadow {
        color: theme.paper_shadow,
        offset: point(px(0.0), px(6.0)),
        blur_radius: px(16.0),
        spread_radius: px(0.0),
        inset: false,
    }];

    let header_status = if session.is_loading() {
        "Syncing…".to_owned()
    } else if session.merged_items.len() == 1 {
        "1 match".to_owned()
    } else {
        format!("{} matches", session.merged_items.len())
    };

    let mut list = div()
        .w(menu_width)
        .p(px(5.0))
        .rounded(px(10.0))
        .bg(theme.surface_background)
        .border_1()
        .border_color(theme.paper_border)
        .shadow(shadow.clone())
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .px(px(8.0))
                .py(px(3.0))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(10.5))
                .text_color(theme.muted_text)
                .child(session.header_title.clone())
                .child(header_status),
        );

    if session.merged_items.is_empty() {
        let empty_text = if session.is_loading() {
            "Searching…"
        } else {
            "No matching items"
        };
        list = list.child(
            div()
                .px(px(10.0))
                .py(px(12.0))
                .text_size(px(12.0))
                .text_color(theme.muted_text)
                .child(empty_text),
        );
    } else {
        let start = session.scroll_offset.min(session.merged_items.len());
        let end = (start + MAX_VISIBLE_ROWS).min(session.merged_items.len());
        for (absolute_index, entry) in session
            .merged_items
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
        {
            let is_selected = absolute_index == session.selected_index;
            let row_bg = if is_selected {
                theme.selection_background
            } else {
                gpui_kit::transparent_black()
            };
            let view_for_click = view.clone();
            let view_for_hover = view.clone();

            let mut main_col = div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(build_highlighted_label(
                    &entry.item.label,
                    &entry.item.match_indices,
                    theme,
                ));
            if let Some(subtitle) = &entry.item.subtitle {
                main_col = main_col.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_text)
                        .child(subtitle.clone()),
                );
            }

            let left_content: AnyElement = if let Some(icon) = &entry.item.icon {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(crate::gpui::emoji::render_emoji_element(
                        icon.as_ref(),
                        16.0,
                        window,
                    ))
                    .child(main_col)
                    .into_any_element()
            } else {
                main_col.into_any_element()
            };

            let mut row = div()
                .id(("rich-completion-row", absolute_index))
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(row_bg)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .on_hover(move |hovered, _window, cx| {
                    if *hovered {
                        view_for_hover.update(cx, |this, cx| {
                            this.select_completion_index(absolute_index, cx);
                        });
                    }
                })
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    cx.stop_propagation();
                    view_for_click.update(cx, |this, cx| {
                        this.accept_completion_at(absolute_index, cx);
                    });
                })
                .child(left_content);

            if let Some(badge) = &entry.item.badge {
                row = row.child(
                    div()
                        .px(px(6.0))
                        .py(px(1.5))
                        .rounded(px(4.0))
                        .bg(theme.code_background)
                        .border_1()
                        .border_color(theme.code_border)
                        .text_size(px(10.0))
                        .text_color(theme.muted_text)
                        .child(badge.clone()),
                );
            }

            list = list.child(row);
        }
    }

    let accept_hint = match session.accept_keys {
        CompletionAcceptKeys::TabOnly => "↑↓ move · Tab accept",
        CompletionAcceptKeys::TabOrEnter => "↑↓ move · Tab / Enter accept",
        CompletionAcceptKeys::TabOrSelectedEnter => {
            if session.user_navigated {
                "↑↓ active · Tab / Enter accept"
            } else {
                "↑↓ move · Tab accept"
            }
        }
    };

    list = list.child(
        div()
            .mt(px(2.0))
            .pt(px(4.0))
            .px(px(8.0))
            .border_t_1()
            .border_color(theme.code_border)
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(10.0))
            .text_color(theme.muted_text)
            .child(accept_hint)
            .child("Esc dismiss"),
    );

    let side_detail = session
        .selected_item()
        .and_then(|entry| entry.item.detail.as_ref());

    if let Some(detail) = side_detail {
        let mut detail_card = div()
            .w(px(220.0))
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(theme.surface_background)
            .border_1()
            .border_color(theme.paper_border)
            .shadow(shadow)
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(detail.title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_text)
                    .child(detail.description.clone()),
            );

        if let Some(preview) = &detail.preview {
            detail_card = detail_card.child(
                div()
                    .p(px(7.0))
                    .rounded(px(6.0))
                    .bg(theme.code_background)
                    .border_1()
                    .border_color(theme.code_border)
                    .text_size(px(11.0))
                    .text_color(theme.text)
                    .child(preview.clone()),
            );
        }

        return div()
            .flex()
            .items_start()
            .gap(px(8.0))
            .child(list)
            .child(detail_card)
            .into_any_element();
    }

    list.into_any_element()
}

impl RichDocumentView {
    /// Registers a caret [`CompletionProvider`] under [`ExtensionNamespace::Completion`].
    ///
    /// Returns [`ExtensionRegistrationError`] when the provider's [`CompletionProvider::id`] is
    /// empty or duplicates an already registered provider.
    pub fn register_completion_provider(
        &mut self,
        provider: impl CompletionProvider,
    ) -> Result<CompletionProviderId, ExtensionRegistrationError> {
        self.completion.register_provider(Rc::new(provider))
    }

    /// Unregisters a previously registered [`CompletionProvider`] by handle.
    pub fn unregister_completion_provider(&mut self, id: CompletionProviderId) -> bool {
        self.completion.unregister_provider(id)
    }

    /// Unregisters a previously registered [`CompletionProvider`] by its string key.
    pub fn unregister_completion_provider_by_key(&mut self, key: &str) -> bool {
        self.completion.unregister_provider_by_key(key)
    }

    /// Installs or clears a custom renderer overriding the default `RichTheme` completion menu UI.
    pub fn set_completion_menu_renderer(
        &mut self,
        renderer: Option<CompletionMenuRenderer>,
        cx: &mut Context<Self>,
    ) {
        self.completion.custom_renderer = renderer;
        if let Some(session) = &self.completion.active_session
            && let Some(anchor_pos) = self
                .document
                .doc_pos_for_block(session.block_id, session.trigger_start)
        {
            let spec =
                build_completion_overlay_spec(anchor_pos, self.completion.custom_renderer.clone());
            self.set_overlay(spec, cx);
        }
    }

    /// Returns true when a caret completion menu session is currently open.
    pub fn has_active_completion(&self) -> bool {
        self.completion.active_session.is_some()
    }

    /// Returns the active completion session identifier, if any.
    pub fn active_completion_session_id(&self) -> Option<CompletionSessionId> {
        self.completion
            .active_session
            .as_ref()
            .map(|session| session.session_id)
    }

    /// Dismisses any active caret completion menu session.
    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        let had_session = self.completion.active_session.take().is_some();
        let removed_overlay = self
            .overlays
            .remove(&OverlayId::new(COMPLETION_MENU_OVERLAY_ID));
        if had_session || removed_overlay {
            cx.notify();
        }
    }

    /// Manually invokes caret completion at the current collapsed caret (for example via `Ctrl+Space`).
    pub fn trigger_completion(&mut self, cx: &mut Context<Self>) -> bool {
        self.evaluate_completions(true, cx)
    }

    /// Pushes asynchronous completion items for `provider_id` into an active session.
    ///
    /// Returns `false` when `(session_id, query_revision)` no longer matches the active session,
    /// safely discarding stale responses. When accepted, the currently highlighted item is
    /// preserved by `(provider_id, item.id)`.
    pub fn update_completion_items(
        &mut self,
        session_id: CompletionSessionId,
        query_revision: u64,
        provider_id: &str,
        items: Vec<CompletionItem>,
        is_complete: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let filtered_items = self.filter_items_for_policy(items);
        let applied = match self.completion.active_session.as_mut() {
            Some(session) => session.apply_async_update(
                session_id,
                query_revision,
                provider_id,
                filtered_items,
                is_complete,
            ),
            None => false,
        };
        if !applied {
            return false;
        }
        let should_dismiss = self
            .completion
            .active_session
            .as_ref()
            .is_some_and(|session| session.merged_items.is_empty() && !session.is_loading());
        if should_dismiss {
            self.dismiss_completion(cx);
        } else {
            cx.notify();
        }
        true
    }

    pub(crate) fn refresh_completion_after_edit(&mut self, cx: &mut Context<Self>) {
        if self.completion.suppress_retrigger || self.completion.providers.is_empty() {
            return;
        }
        self.evaluate_completions(false, cx);
    }

    pub(crate) fn reconcile_completion_with_selection(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.completion.active_session else {
            return;
        };
        let still_inside = match &self.selection {
            Some(DocumentSelection::Text { anchor, focus, .. }) if anchor == focus => {
                focus.leaf_block_id() == Some(session.block_id)
                    && focus.offset >= session.query_start
                    && focus.offset <= session.caret_offset
            }
            _ => false,
        };
        if !still_inside {
            self.dismiss_completion(cx);
        }
    }

    pub(crate) fn select_completion_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = self.completion.active_session.as_mut() else {
            return;
        };
        if index < session.merged_items.len() && session.selected_index != index {
            session.selected_index = index;
            session.user_navigated = true;
            session.ensure_selected_in_window();
            cx.notify();
        }
    }

    pub(crate) fn handle_completion_overlay_key(
        &mut self,
        action: OverlayKeyAction,
        cx: &mut Context<Self>,
    ) -> OverlayKeyResult {
        let Some(session) = self.completion.active_session.as_mut() else {
            return OverlayKeyResult::PassThrough;
        };
        match action {
            OverlayKeyAction::Up | OverlayKeyAction::ShiftTab => {
                session.move_selection(-1);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Down => {
                session.move_selection(1);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::PageUp => {
                session.move_selection(-(MAX_VISIBLE_ROWS as isize));
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::PageDown => {
                session.move_selection(MAX_VISIBLE_ROWS as isize);
                cx.notify();
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Left | OverlayKeyAction::Right => OverlayKeyResult::PassThrough,
            OverlayKeyAction::Escape => {
                self.dismiss_completion(cx);
                OverlayKeyResult::Handled
            }
            OverlayKeyAction::Tab | OverlayKeyAction::Enter => {
                let should_accept =
                    should_accept_on_key(action, session.accept_keys, session.user_navigated);
                let has_items = !session.merged_items.is_empty();
                let selected_index = session.selected_index;
                if should_accept && has_items {
                    self.accept_completion_at(selected_index, cx);
                    OverlayKeyResult::Handled
                } else if action == OverlayKeyAction::Enter {
                    self.dismiss_completion(cx);
                    OverlayKeyResult::PassThrough
                } else {
                    OverlayKeyResult::PassThrough
                }
            }
        }
    }

    pub(crate) fn accept_completion_at(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.completion.active_session.take() else {
            return false;
        };
        self.overlays
            .remove(&OverlayId::new(COMPLETION_MENU_OVERLAY_ID));
        let Some(resolved) = session.merged_items.get(index).cloned() else {
            cx.notify();
            return false;
        };

        let block_id = session.block_id;
        let replace_range = session.replace_range();
        let current_text_len = self
            .document
            .find_block(block_id)
            .and_then(|block| block.kind.inline_content())
            .map(|inline| inline.text.len())
            .unwrap_or(0);
        let safe_start = replace_range.start.min(current_text_len);
        let safe_end = replace_range.end.clamp(safe_start, current_text_len);
        let safe_range = safe_start..safe_end;

        let provider_ref = self
            .completion
            .providers
            .iter()
            .find(|registered| registered.provider.id() == resolved.provider_id)
            .map(|registered| Rc::clone(&registered.provider));

        let original_selection = self.selection.clone();
        let range_selection = if !safe_range.is_empty() {
            match (
                self.document.doc_pos_for_block(block_id, safe_range.start),
                self.document.doc_pos_for_block(block_id, safe_range.end),
            ) {
                (Some(anchor), Some(focus)) => Some(DocumentSelection::Text {
                    anchor,
                    focus,
                    affinity: Affinity::Downstream,
                    goal_x: None,
                    stored_marks: None,
                }),
                _ => None,
            }
        } else {
            None
        };

        // Route every acceptance edit strictly through `dispatch_command` so middleware,
        // `InteractionPolicy`, undo history, and `RichDocumentEvent::CommandExecuted` run uniformly.
        self.history.break_coalescing();
        self.completion.suppress_retrigger = true;

        let accepted_outcome = match resolved.item.accept.clone() {
            CompletionAccept::ReplaceText(text) => {
                if let Some(selection) = range_selection {
                    self.selection = Some(selection);
                }
                let outcome = self.dispatch_command_with_source(
                    RichCommand::InsertText(text),
                    CommandSource::User,
                    cx,
                );
                if outcome != CommandOutcome::Executed {
                    self.selection = original_selection;
                    cx.notify();
                }
                outcome
            }
            CompletionAccept::ReplaceWithInlineObject(object) => {
                if let Some(selection) = range_selection {
                    self.selection = Some(selection);
                }
                let outcome = self.dispatch_command_with_source(
                    RichCommand::InsertInlineObject(object),
                    CommandSource::User,
                    cx,
                );
                if outcome != CommandOutcome::Executed {
                    self.selection = original_selection;
                    cx.notify();
                }
                outcome
            }
            CompletionAccept::RunCommand {
                command,
                delete_trigger,
            } => {
                if !delete_trigger || safe_range.is_empty() {
                    self.dispatch_command_with_source(command, CommandSource::User, cx)
                } else {
                    let command_resolvable = matches!(
                        self.extensions.resolve_command(&command),
                        CommandResolution::BuiltIn | CommandResolution::RegisteredExtension
                    );
                    let command_allowed = self
                        .policy()
                        .check_command(&command, CommandSource::User)
                        .is_none();
                    if !command_resolvable || !command_allowed {
                        cx.notify();
                        CommandOutcome::NoOp
                    } else {
                        if let Some(selection) = range_selection {
                            self.selection = Some(selection);
                        }
                        let delete_outcome = self.dispatch_command_with_source(
                            RichCommand::Delete {
                                direction: MoveDirection::Backward,
                                unit: MoveUnit::Grapheme,
                            },
                            CommandSource::User,
                            cx,
                        );
                        if delete_outcome != CommandOutcome::Executed {
                            self.selection = original_selection;
                            cx.notify();
                            delete_outcome
                        } else {
                            let undo_len_after_delete = self.history.undo_len();
                            let outcome =
                                self.dispatch_command_with_source(command, CommandSource::User, cx);
                            if self.history.undo_len() > undo_len_after_delete {
                                self.history.merge_last_two_into_atomic();
                            }
                            outcome
                        }
                    }
                }
            }
        };

        self.completion.suppress_retrigger = false;

        if accepted_outcome == CommandOutcome::Executed {
            if let Some(provider) = provider_ref {
                provider.on_accept(&CompletionAcceptContext {
                    session_id: session.session_id,
                    block_id,
                    trigger: &session.trigger,
                    item: &resolved.item,
                });
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn active_completion_inline_preview(
        &self,
    ) -> Option<(BlockId, usize, Affinity, String)> {
        let session = self.completion.active_session.as_ref()?;
        let preview = session
            .selected_item()?
            .item
            .inline_preview
            .as_ref()
            .filter(|text| !text.is_empty())?;
        let (focus, affinity) = match self.selection.as_ref()? {
            DocumentSelection::Text {
                anchor,
                focus,
                affinity,
                ..
            } if anchor == focus => (focus, *affinity),
            _ => return None,
        };
        if focus.leaf_block_id() != Some(session.block_id) {
            return None;
        }
        Some((session.block_id, focus.offset, affinity, preview.clone()))
    }

    fn filter_items_for_policy(&self, items: Vec<CompletionItem>) -> Vec<CompletionItem> {
        if self.policy().editing.enabled {
            items
        } else {
            items
                .into_iter()
                .filter(|item| !item.accept.requires_editing())
                .collect()
        }
    }

    fn evaluate_completions(&mut self, manual_invoke: bool, cx: &mut Context<Self>) -> bool {
        if self.ime_preedit.is_some() || self.completion.providers.is_empty() {
            self.dismiss_completion(cx);
            return false;
        }
        let Some((block_id, caret_offset)) = (match &self.selection {
            Some(DocumentSelection::Text { anchor, focus, .. }) if anchor == focus => {
                focus.leaf_block_id().map(|id| (id, focus.offset))
            }
            _ => None,
        }) else {
            self.dismiss_completion(cx);
            return false;
        };

        let Some(block_text) = self
            .document
            .find_block(block_id)
            .and_then(|block| block.kind.inline_content())
            .map(|inline| inline.text.clone())
        else {
            self.dismiss_completion(cx);
            return false;
        };
        let Some(block_kind) = self
            .document
            .find_block(block_id)
            .map(|block| block.kind.clone())
        else {
            self.dismiss_completion(cx);
            return false;
        };

        // 1. Find best matching prefix trigger (closest trigger_start to the caret), merging all
        // providers that match the exact same prefix and start offset.
        let mut best_prefix: Option<(DetectedTrigger, Vec<usize>)> = None;
        for (provider_index, registered) in self.completion.providers.iter().enumerate() {
            if !registered.provider.block_scope().matches(&block_kind) {
                continue;
            }
            for trigger in registered.provider.triggers() {
                if let CompletionTrigger::Prefix {
                    prefix,
                    boundary,
                    allow_spaces,
                } = trigger
                    && let Some(detected) = match_prefix_trigger(
                        &block_text,
                        caret_offset,
                        prefix.as_ref(),
                        *boundary,
                        *allow_spaces,
                    )
                {
                    match &mut best_prefix {
                        None => {
                            best_prefix = Some((detected, vec![provider_index]));
                        }
                        Some((current, indices)) => {
                            if detected.trigger_start > current.trigger_start {
                                *current = detected;
                                indices.clear();
                                indices.push(provider_index);
                            } else if detected.trigger_start == current.trigger_start
                                && detected.trigger == current.trigger
                                && !indices.contains(&provider_index)
                            {
                                indices.push(provider_index);
                            }
                        }
                    }
                }
            }
        }

        // 2. Fallback to manual triggers (explicit invoke or continuing an active manual session).
        let continuing_manual = self.completion.active_session.as_ref().and_then(|session| {
            if session.trigger == CompletionTriggerKind::Manual
                && session.block_id == block_id
                && caret_offset >= session.trigger_start
            {
                let query = block_text.get(session.trigger_start..caret_offset)?;
                if !query.chars().any(char::is_whitespace) && !query.contains('\u{FFFC}') {
                    return Some(DetectedTrigger {
                        trigger: CompletionTriggerKind::Manual,
                        trigger_start: session.trigger_start,
                        query_start: session.trigger_start,
                        caret_offset,
                        allow_spaces: false,
                    });
                }
            }
            None
        });

        let selected_match: Option<(DetectedTrigger, Vec<usize>)> = if let Some(prefix_match) =
            best_prefix
        {
            Some(prefix_match)
        } else if manual_invoke || continuing_manual.is_some() {
            let detected =
                continuing_manual.or_else(|| match_manual_trigger(&block_text, caret_offset));
            detected.and_then(|detected| {
                let mut indices = Vec::new();
                for (provider_index, registered) in self.completion.providers.iter().enumerate() {
                    if !registered.provider.block_scope().matches(&block_kind) {
                        continue;
                    }
                    if registered.provider.triggers().iter().any(|trigger| {
                        matches!(
                            trigger,
                            CompletionTrigger::Manual | CompletionTrigger::WordPrefix { .. }
                        )
                    }) {
                        indices.push(provider_index);
                    }
                }
                (!indices.is_empty()).then_some((detected, indices))
            })
        } else {
            // 3. Automatic word-prefix triggers (for example code block symbol completions).
            let mut word_match: Option<(DetectedTrigger, Vec<usize>)> = None;
            for (provider_index, registered) in self.completion.providers.iter().enumerate() {
                if !registered.provider.block_scope().matches(&block_kind) {
                    continue;
                }
                for trigger in registered.provider.triggers() {
                    if let CompletionTrigger::WordPrefix { min_chars } = trigger
                        && let Some(detected) =
                            match_word_prefix_trigger(&block_text, caret_offset, *min_chars)
                    {
                        match &mut word_match {
                            None => word_match = Some((detected, vec![provider_index])),
                            Some((current, indices))
                                if current.trigger_start == detected.trigger_start =>
                            {
                                if !indices.contains(&provider_index) {
                                    indices.push(provider_index);
                                }
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
            word_match
        };

        let Some((detected, provider_indices)) = selected_match else {
            self.dismiss_completion(cx);
            return false;
        };

        let query = block_text
            .get(detected.query_start..detected.caret_offset)
            .unwrap_or("");

        let same_session = self
            .completion
            .active_session
            .as_ref()
            .is_some_and(|session| {
                session.block_id == block_id
                    && session.trigger == detected.trigger
                    && session.trigger_start == detected.trigger_start
            });

        let (session_id, query_revision, preserve_key, user_navigated) = if same_session {
            if let Some(existing) = self.completion.active_session.as_ref() {
                (
                    existing.session_id,
                    existing.query_revision.saturating_add(1),
                    existing
                        .selected_item()
                        .map(|entry| (entry.provider_id, entry.item.id.clone())),
                    existing.user_navigated,
                )
            } else {
                (self.completion.allocate_session_id(), 1, None, false)
            }
        } else {
            (self.completion.allocate_session_id(), 1, None, false)
        };

        let weak_view = cx.entity().downgrade();
        let mut slices = Vec::with_capacity(provider_indices.len());
        let mut header_title: Option<String> = None;
        let mut header_priority = i32::MIN;
        let mut accept_keys = match detected.trigger {
            CompletionTriggerKind::WordPrefix => CompletionAcceptKeys::TabOrSelectedEnter,
            CompletionTriggerKind::Prefix(_) | CompletionTriggerKind::Manual => {
                CompletionAcceptKeys::TabOrEnter
            }
        };
        let mut accept_priority = i32::MIN;

        let context = CompletionQueryContext {
            session_id,
            query_revision,
            block_id,
            block_kind: &block_kind,
            trigger: detected.trigger.clone(),
            query,
            trigger_start: detected.trigger_start,
            caret_offset: detected.caret_offset,
            document: &self.document,
            selection: self.selection.as_ref(),
        };

        for provider_index in provider_indices {
            let Some(registered) = self.completion.providers.get(provider_index) else {
                continue;
            };
            let priority = registered.provider.priority();
            if priority >= header_priority
                && let Some(title) = registered.provider.header_title()
            {
                header_priority = priority;
                header_title = Some(title.to_owned());
            }
            if priority >= accept_priority {
                accept_priority = priority;
                accept_keys = registered.provider.accept_keys();
            }
            let result = registered.provider.query(&context, weak_view.clone(), cx);
            let (items, pending) = match result {
                CompletionQueryResult::Ready(items) => (items, false),
                CompletionQueryResult::Pending { initial } => (initial, true),
            };
            let filtered = self.filter_items_for_policy(items);
            slices.push(ProviderSlice {
                provider_id: registered.provider.id(),
                priority,
                pending,
                items: filtered,
            });
        }

        let mut session = ActiveCompletionSession {
            session_id,
            query_revision,
            block_id,
            trigger: detected.trigger,
            trigger_start: detected.trigger_start,
            query_start: detected.query_start,
            caret_offset: detected.caret_offset,
            header_title: header_title.unwrap_or_else(|| "Completions".to_owned()),
            accept_keys,
            user_navigated,
            slices,
            merged_items: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
        };
        session.rebuild_merged_preserving_selection(preserve_key);

        if session.merged_items.is_empty() && !session.is_loading() {
            self.dismiss_completion(cx);
            return false;
        }

        let Some(anchor_pos) = self
            .document
            .doc_pos_for_block(block_id, detected.trigger_start)
        else {
            self.dismiss_completion(cx);
            return false;
        };

        self.completion.active_session = Some(session);
        let spec =
            build_completion_overlay_spec(anchor_pos, self.completion.custom_renderer.clone());
        self.set_overlay(spec, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_trigger_respects_whitespace_boundary_and_rejects_urls() {
        let found = match_prefix_trigger(
            "hello /head",
            11,
            "/",
            PrefixBoundary::LineStartOrWhitespace,
            false,
        );
        assert_eq!(
            found,
            Some(DetectedTrigger {
                trigger: CompletionTriggerKind::Prefix(SharedStr::from("/")),
                trigger_start: 6,
                query_start: 7,
                caret_offset: 11,
                allow_spaces: false,
            })
        );

        let url = match_prefix_trigger(
            "https://example.com/path",
            24,
            "/",
            PrefixBoundary::LineStartOrWhitespace,
            false,
        );
        assert!(url.is_none());

        let email = match_prefix_trigger(
            "alice@acme.dev",
            14,
            "@",
            PrefixBoundary::LineStartOrWhitespace,
            false,
        );
        assert!(email.is_none());
    }

    #[test]
    fn word_prefix_trigger_requires_min_chars() {
        assert!(match_word_prefix_trigger("items.re", 8, 3).is_none());
        let matched = match_word_prefix_trigger("items.ret", 9, 3);
        assert_eq!(
            matched,
            Some(DetectedTrigger {
                trigger: CompletionTriggerKind::WordPrefix,
                trigger_start: 6,
                query_start: 6,
                caret_offset: 9,
                allow_spaces: false,
            })
        );
    }

    #[test]
    fn async_update_drops_stale_revisions_and_preserves_selected_item_by_id() {
        let mut session = ActiveCompletionSession {
            session_id: CompletionSessionId(4),
            query_revision: 3,
            block_id: BlockId(1),
            trigger: CompletionTriggerKind::Prefix(SharedStr::from("@")),
            trigger_start: 0,
            query_start: 1,
            caret_offset: 3,
            header_title: "People".to_owned(),
            accept_keys: CompletionAcceptKeys::TabOrEnter,
            user_navigated: false,
            slices: vec![ProviderSlice {
                provider_id: "acme.mentions",
                priority: 0,
                pending: true,
                items: vec![
                    CompletionItem::new(
                        "alex",
                        "Alex Rivera",
                        CompletionAccept::replace_text("Alex"),
                    )
                    .with_match(40, vec![0, 1]),
                ],
            }],
            merged_items: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
        };
        session.rebuild_merged_preserving_selection(None);
        assert_eq!(
            session.selected_item().map(|e| e.item.id.as_ref()),
            Some("alex")
        );

        // Stale revision is ignored.
        assert!(!session.apply_async_update(
            CompletionSessionId(4),
            2,
            "acme.mentions",
            Vec::new(),
            true,
        ));
        assert!(session.is_loading());

        // Current revision merges higher-scoring "alice" above "alex" while keeping "alex" selected.
        assert!(session.apply_async_update(
            CompletionSessionId(4),
            3,
            "acme.mentions",
            vec![
                CompletionItem::new("alice", "Alice Chen", CompletionAccept::replace_text("Alice"))
                    .with_match(80, vec![0, 1]),
                CompletionItem::new("alex", "Alex Rivera", CompletionAccept::replace_text("Alex"))
                    .with_match(40, vec![0, 1]),
            ],
            true,
        ));
        assert!(!session.is_loading());
        assert_eq!(session.merged_items[0].item.id.as_ref(), "alice");
        assert_eq!(session.selected_index, 1);
        assert_eq!(
            session.selected_item().map(|e| e.item.id.as_ref()),
            Some("alex")
        );
    }

    #[test]
    fn accept_keys_policy_respects_user_navigation() {
        assert!(should_accept_on_key(
            OverlayKeyAction::Tab,
            CompletionAcceptKeys::TabOnly,
            false
        ));
        assert!(!should_accept_on_key(
            OverlayKeyAction::Enter,
            CompletionAcceptKeys::TabOnly,
            true
        ));
        assert!(should_accept_on_key(
            OverlayKeyAction::Enter,
            CompletionAcceptKeys::TabOrEnter,
            false
        ));
        assert!(!should_accept_on_key(
            OverlayKeyAction::Enter,
            CompletionAcceptKeys::TabOrSelectedEnter,
            false
        ));
        assert!(should_accept_on_key(
            OverlayKeyAction::Enter,
            CompletionAcceptKeys::TabOrSelectedEnter,
            true
        ));
    }

    #[test]
    fn fuzzy_matcher_scores_prefix_above_subsequence() {
        let prefix = fuzzy_match_label("he", "Heading 1");
        let subseq = fuzzy_match_label("he", "The Table");
        assert!(prefix.is_some());
        assert!(subseq.is_some());
        assert!(prefix.map(|(score, _)| score) > subseq.map(|(score, _)| score));
    }
}
