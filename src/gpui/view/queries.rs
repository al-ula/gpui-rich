//! Unified read-only CQRS query execution on [`RichDocumentView`], bridging pure-model
//! [`ModelQuery`] types and view/layout-level [`ViewQuery`] snapshots.

use super::*;
use crate::model::{
    ActiveTableContext, ExtensionQuery, FormattingSnapshot, FormattingState, ModelQuery,
    ModelQueryContext, QueryError, SearchState,
};
use serde::{Deserialize, Serialize};

/// Read-only query evaluated against [`RichDocumentView`] via [`RichDocumentView::query`].
///
/// Every pure-model [`ModelQuery`] automatically implements [`ViewQuery`] and receives the view's
/// component/extension clipboard formatter.
pub trait ViewQuery {
    /// Stable dot-namespaced identifier for diagnostics and tooling.
    const ID: &'static str;
    /// Typed value produced by this query.
    type Output;

    /// Evaluates this query against `view`.
    fn execute(self, view: &RichDocumentView) -> Self::Output;
}

impl<Q: ModelQuery> ViewQuery for Q {
    const ID: &'static str = Q::ID;
    type Output = Q::Output;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        let formatter = clipboard::ComponentClipboardFormatter::new(view);
        let context = ModelQueryContext::new(&view.document, view.selection.as_ref())
            .with_clipboard_formatter(&formatter);
        self.execute(&context)
    }
}

/// Combined formatting, table, history, and policy snapshot for host toolbars.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolbarSnapshot {
    pub formatting: FormattingSnapshot,
    pub table: Option<ActiveTableContext>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_len: usize,
    pub redo_len: usize,
    pub policy: InteractionPolicy,
}

/// Query returning [`ToolbarSnapshot`] for the current selection and view state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolbarState;

impl ViewQuery for ToolbarState {
    const ID: &'static str = "rich.query.toolbar_state";
    type Output = ToolbarSnapshot;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        ToolbarSnapshot {
            formatting: view.query(FormattingState),
            table: view.active_table_context(),
            can_undo: view.history.can_undo(),
            can_redo: view.history.can_redo(),
            undo_len: view.history.undo_len(),
            redo_len: view.history.redo_len(),
            policy: view.policy(),
        }
    }
}

/// Snapshot of view-level viewport, scroll, zoom, layout, and policy state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSnapshot {
    pub zoom: f32,
    pub horizontal_layout: HorizontalLayout,
    pub margins_pt: Edges,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub total_height_px: f32,
    pub debug_overlay: bool,
    pub policy: InteractionPolicy,
    pub document_revision: u64,
    pub has_ime_preedit: bool,
}

/// Query returning [`ViewSnapshot`] for the current [`RichDocumentView`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewState;

impl ViewQuery for ViewState {
    const ID: &'static str = "rich.query.view_state";
    type Output = ViewSnapshot;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        ViewSnapshot {
            zoom: view.zoom,
            horizontal_layout: view.document.config.horizontal_layout,
            margins_pt: view.document.config.active_margins_pt(),
            scroll_x: view.scroll_x,
            scroll_y: view.scroll_y,
            total_height_px: view.flow.total_height(),
            debug_overlay: view.debug_overlay,
            policy: view.policy(),
            document_revision: view.document_revision,
            has_ime_preedit: view.ime_preedit.is_some(),
        }
    }
}

/// Query returning a snapshot of the view's active [`SearchState`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSearchState;

impl ViewQuery for ActiveSearchState {
    const ID: &'static str = "rich.query.active_search_state";
    type Output = SearchState;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.search_state().clone()
    }
}

/// Query returning the last painted window-space viewport bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewportBounds;

impl ViewQuery for ViewportBounds {
    const ID: &'static str = "rich.query.viewport_bounds";
    type Output = Option<Bounds<Pixels>>;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.viewport_bounds()
    }
}

/// Query returning the window-space bounds of the active caret (or selection focus edge).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretBounds;

impl ViewQuery for CaretBounds {
    const ID: &'static str = "rich.query.caret_bounds";
    type Output = Option<Bounds<Pixels>>;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.caret_bounds()
    }
}

/// Query returning window-space rectangles covering the active selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionRects;

impl ViewQuery for SelectionRects {
    const ID: &'static str = "rich.query.selection_rects";
    type Output = Vec<Bounds<Pixels>>;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.selection_rects()
    }
}

/// Query returning the window-space bounding box of the active selection (falling back to caret bounds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionBounds;

impl ViewQuery for SelectionBounds {
    const ID: &'static str = "rich.query.selection_bounds";
    type Output = Option<Bounds<Pixels>>;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.selection_bounds()
    }
}

/// Query returning the extension validation diagnostics recorded during the most recent prepaint pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtensionDiagnostics;

impl ViewQuery for ExtensionDiagnostics {
    const ID: &'static str = "rich.query.extension_diagnostics";
    type Output = Vec<ExtensionDiagnostic>;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        view.extension_diagnostics().to_vec()
    }
}

/// Query checking whether a user-initiated [`RichCommand`] passes registry resolution and
/// [`InteractionPolicy`] checks without dispatching it.
#[derive(Clone, Debug, PartialEq)]
pub struct CanExecute(pub RichCommand);

impl ViewQuery for CanExecute {
    const ID: &'static str = "rich.query.can_execute";
    type Output = bool;

    fn execute(self, view: &RichDocumentView) -> Self::Output {
        match view.extensions.resolve_command(&self.0) {
            CommandResolution::BuiltIn | CommandResolution::RegisteredExtension => view
                .policy
                .check_command(&self.0, CommandSource::User)
                .is_none(),
            CommandResolution::UnregisteredExtension | CommandResolution::MismatchedExtension => {
                false
            }
        }
    }
}

impl RichDocumentView {
    /// Executes a built-in [`ViewQuery`] or pure-model [`ModelQuery`] against this view.
    pub fn query<Q: ViewQuery>(&self, query: Q) -> Q::Output {
        query.execute(self)
    }

    /// Registers a typed read-only [`ExtensionQuery`] handler on this view.
    pub fn register_query<Q: ExtensionQuery>(
        &mut self,
        handler: impl Fn(&Q, &QueryContext<'_>) -> Q::Output + 'static,
    ) -> Result<(), ExtensionRegistrationError> {
        self.extensions.register_query::<Q>(handler)
    }

    /// Unregisters a typed [`ExtensionQuery`] handler from this view.
    pub fn unregister_query<Q: ExtensionQuery>(&mut self) -> bool {
        self.extensions.unregister_query::<Q>()
    }

    /// Executes a registered typed [`ExtensionQuery`] against this view.
    pub fn query_extension<Q: ExtensionQuery>(&self, query: &Q) -> Result<Q::Output, QueryError> {
        let context = QueryContext::new(
            &self.document,
            self.selection.as_ref(),
            self.policy,
            self.search_state(),
            self.active_table_context(),
        );
        self.extensions.execute_extension_query(query, &context)
    }
}
