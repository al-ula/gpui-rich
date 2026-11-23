use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use rich_gpui::{
    BlockRenderContext, BlockRenderer, CustomRenderOutput, ExtensionDiagnosticKind,
    ExtensionNamespace, ExtensionNodeTarget, ExtensionRegistrationError, ImageAssetError,
    InlineRenderContext, InlineRenderer, LoadedAsset, MemoryAssetProvider, ReadCommandOutcome,
    RichDocumentEvent, RichDocumentView, register_default_keybindings,
};
use rich_layout::{InlineBoxMetrics, Insets, Size};
use rich_model::{
    Affinity, BlockByteOffset, BlockByteRange, BlockId, BlockKind, BlockNode, BlockShapeKind,
    BlockSpec, BlockStyle, ClipboardCustomFormatter, ColumnSpec, CommandDecision,
    CommandEffectKind, CommandMiddleware, CommandOutcome, CommandResolution, CommandSource, DocPos,
    Document, DocumentCodec, DocumentSelection, Edit, EncodedDocument, ExtensionCommand,
    ImageAssetId, InlineContent, InlineDecorationStyle, InlineObject, InlineObjectId, InlineSpec,
    InlineStyle, InlineVAlign, InteractionPolicy, JsonCodec, ReadOnly, ResolvedCommand, RgbaColor,
    RichCommand, Step, TableBlock, TableBorderStyle, TableCell, TableRow, Transaction, block_shape,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StatusBadgeData {
    label: String,
    tone: String,
}

struct StatusBadgeSpec;

impl InlineSpec for StatusBadgeSpec {
    const KIND: &'static str = "acme.status_badge";
    type Data = StatusBadgeData;
}

struct StatusBadgeRenderer;

impl InlineRenderer for StatusBadgeRenderer {
    type Spec = StatusBadgeSpec;

    fn measure(&self, data: &StatusBadgeData, zoom: f32, _window: &Window) -> InlineBoxMetrics {
        InlineBoxMetrics {
            width: ((data.label.len() as f32) * 7.0 + 16.0) * zoom,
            height: 18.0 * zoom,
            valign: InlineVAlign::Middle,
        }
    }

    fn render(
        &self,
        context: InlineRenderContext<'_, StatusBadgeData>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        Some(CustomRenderOutput::Element(
            div().child(context.data.label.clone()).into_any_element(),
        ))
    }

    fn plain_text(&self, data: &StatusBadgeData) -> Option<String> {
        Some(format!("[{}]", data.label))
    }

    // Intentionally omits any `data-gpui-rich-*` extension markers so same-process rich paste
    // must rely on the versioned structured fragment rather than HTML scraping.
    fn html(&self, data: &StatusBadgeData) -> Option<String> {
        Some(format!("<span>{}</span>", data.label))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TagPillData {
    tag: String,
}

struct TagPillSpec;

impl InlineSpec for TagPillSpec {
    const KIND: &'static str = "acme.tag_pill";
    type Data = TagPillData;
}

struct TagPillRenderer;

impl InlineRenderer for TagPillRenderer {
    type Spec = TagPillSpec;

    fn measure(&self, _data: &TagPillData, zoom: f32, _window: &Window) -> InlineBoxMetrics {
        InlineBoxMetrics {
            width: 48.0 * zoom,
            height: 18.0 * zoom,
            valign: InlineVAlign::Middle,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CalloutData {
    title: String,
}

struct CalloutSpec;

impl BlockSpec for CalloutSpec {
    const KIND: &'static str = "acme.callout";
    type Shape = block_shape::Container;
    type Data = CalloutData;
}

struct CalloutRenderer;

impl BlockRenderer for CalloutRenderer {
    type Spec = CalloutSpec;

    fn measure_chrome(
        &self,
        _data: &CalloutData,
        _max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Insets {
        Insets::xy(12.0 * zoom, 8.0 * zoom)
    }

    fn render(
        &self,
        _context: BlockRenderContext<'_, CalloutData>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        Some(CustomRenderOutput::DirectPaint(Box::new(
            |_bounds, _theme, _window, _cx| {},
        )))
    }
}

// Intentionally shares the exact `KIND` string with `StatusBadgeSpec` to verify namespace separation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SameNameBlockData {
    caption: String,
}

struct SameNameBlockSpec;

impl BlockSpec for SameNameBlockSpec {
    const KIND: &'static str = "acme.status_badge";
    type Shape = block_shape::Leaf;
    type Data = SameNameBlockData;
}

struct SameNameBlockRenderer;

impl BlockRenderer for SameNameBlockRenderer {
    type Spec = SameNameBlockSpec;

    fn measure_leaf(
        &self,
        _data: &SameNameBlockData,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new(max_width.min(180.0 * zoom), 36.0 * zoom)
    }
}

struct ReservedBlockSpec;

impl BlockSpec for ReservedBlockSpec {
    const KIND: &'static str = "paragraph";
    type Shape = block_shape::Leaf;
    type Data = SameNameBlockData;
}

struct ReservedBlockRenderer;

impl BlockRenderer for ReservedBlockRenderer {
    type Spec = ReservedBlockSpec;
}

struct ReservedInlineSpec;

impl InlineSpec for ReservedInlineSpec {
    const KIND: &'static str = rich_model::FOOTNOTE_REFERENCE_KIND;
    type Data = TagPillData;
}

struct ReservedInlineRenderer;

impl InlineRenderer for ReservedInlineRenderer {
    type Spec = ReservedInlineSpec;
}

#[derive(Debug)]
struct RenameBadge {
    block_id: BlockId,
    object_id: InlineObjectId,
    new_label: String,
}

impl ExtensionCommand for RenameBadge {
    const ID: &'static str = "acme.badge.rename";
    type Effect = Edit;
}

// Same ID as `RenameBadge` with a different Rust type to test `MismatchedCommandType`.
#[derive(Debug)]
struct WrongTypeRenameBadge;

impl ExtensionCommand for WrongTypeRenameBadge {
    const ID: &'static str = "acme.badge.rename";
    type Effect = Edit;
}

#[derive(Debug)]
struct UnregisteredBadgeCommand;

impl ExtensionCommand for UnregisteredBadgeCommand {
    const ID: &'static str = "acme.badge.unregistered";
    type Effect = ReadOnly;
}

#[derive(Debug)]
struct CountBadges;

impl ExtensionCommand for CountBadges {
    const ID: &'static str = "acme.badge.count";
    type Effect = ReadOnly;
}

#[derive(Debug)]
struct ReserveOnlyEdit;

impl ExtensionCommand for ReserveOnlyEdit {
    const ID: &'static str = "acme.badge.reserve_only";
    type Effect = Edit;
}

gpui_kit::actions!(acme_extension_tests, [RenameFirstBadge]);

struct ConsumerHost {
    view: Entity<RichDocumentView>,
    target: Rc<RefCell<Option<(BlockId, InlineObjectId, String)>>>,
}

impl Render for ConsumerHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.view.clone();
        let target = Rc::clone(&self.target);
        div()
            .size_full()
            .on_action(
                cx.listener(move |_this, _action: &RenameFirstBadge, _window, cx| {
                    if let Some((target_block, target_object, next_label)) =
                        target.borrow().clone()
                    {
                        view.update(cx, |document_view, cx| {
                            document_view.dispatch_extension_command(
                                RenameBadge {
                                    block_id: target_block,
                                    object_id: target_object,
                                    new_label: next_label,
                                },
                                cx,
                            );
                        });
                    }
                }),
            )
            .child(self.view.clone())
    }
}

fn find_first_badge(document: &Document) -> Option<(BlockId, InlineObjectId, StatusBadgeData)> {
    for block in document.collect_leaf_blocks() {
        let Some(inline) = block.kind.inline_content() else {
            continue;
        };
        for entry in &inline.objects {
            if let Ok(data) = entry.object.read_data::<StatusBadgeSpec>() {
                return Some((block.id, entry.id, data.into_owned()));
            }
        }
    }
    None
}

fn configure_badge_commands(
    view: &mut RichDocumentView,
    rename_planner_calls: Arc<AtomicUsize>,
    read_calls: Arc<AtomicUsize>,
) {
    let planner_calls = Arc::clone(&rename_planner_calls);
    view.register_edit_command::<RenameBadge>(move |command, context| {
        planner_calls.fetch_add(1, Ordering::SeqCst);
        let block = context.document().find_block(command.block_id)?;
        let inline = block.kind.inline_content()?;
        let existing = inline
            .objects
            .iter()
            .find(|entry| entry.id == command.object_id)?;
        let mut updated = existing
            .object
            .read_data::<StatusBadgeSpec>()
            .ok()?
            .into_owned();
        updated.label = command.new_label.clone();
        let step = Step::update_inline_data::<StatusBadgeSpec>(
            command.block_id,
            command.object_id,
            updated,
        );
        Some(Transaction::new(vec![step]))
    })
    .expect("register RenameBadge");

    let count_calls = Arc::clone(&read_calls);
    view.register_read_command::<CountBadges>(move |_command, context| {
        count_calls.fetch_add(1, Ordering::SeqCst);
        if find_first_badge(context.document()).is_some() {
            ReadCommandOutcome::Executed
        } else {
            ReadCommandOutcome::NoOp
        }
    })
    .expect("register CountBadges");

    view.register_edit_command::<ReserveOnlyEdit>(|_command, context| {
        let _reserved = context.reserve_block_id();
        None
    })
    .expect("register ReserveOnlyEdit");
}

struct TestCommandMiddleware {
    replace_count_with_rename: Rc<RefCell<Option<RenameBadge>>>,
}

impl CommandMiddleware for TestCommandMiddleware {
    fn before_command(
        &mut self,
        command: &ResolvedCommand<'_>,
        _source: CommandSource,
        _document: &Document,
        _selection: Option<&DocumentSelection>,
    ) -> CommandDecision {
        if let Some(rename) = command.args::<RenameBadge>()
            && rename.new_label == "FORBIDDEN"
        {
            return CommandDecision::Cancel;
        }
        if command.args::<CountBadges>().is_some()
            && let Some(replacement) = self.replace_count_with_rename.borrow().as_ref()
        {
            return CommandDecision::Replace(RichCommand::extension(RenameBadge {
                block_id: replacement.block_id,
                object_id: replacement.object_id,
                new_label: replacement.new_label.clone(),
            }));
        }
        CommandDecision::Continue
    }
}

static DECODE_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CountingPayload {
    title: String,
}

impl<'de> Deserialize<'de> for CountingPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            title: String,
        }
        DECODE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self { title: raw.title })
    }
}

struct CountingLeafSpec;

impl BlockSpec for CountingLeafSpec {
    const KIND: &'static str = "acme.counting_leaf";
    type Shape = block_shape::Leaf;
    type Data = CountingPayload;
}

struct CountingLeafRenderer;

impl BlockRenderer for CountingLeafRenderer {
    type Spec = CountingLeafSpec;

    fn measure_leaf(
        &self,
        data: &CountingPayload,
        max_width: f32,
        zoom: f32,
        _window: &Window,
    ) -> Size {
        Size::new((data.title.len() as f32 * 8.0).min(max_width), 32.0 * zoom)
    }

    fn render(
        &self,
        context: BlockRenderContext<'_, CountingPayload>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<CustomRenderOutput> {
        Some(CustomRenderOutput::Element(
            div().child(context.data.title.clone()).into_any_element(),
        ))
    }

    fn plain_text(&self, data: &CountingPayload) -> Option<String> {
        Some(data.title.clone())
    }
}

struct RecordingDecorator {
    visited_blocks: Arc<Mutex<Vec<BlockId>>>,
}

impl rich_model::DecorationProvider for RecordingDecorator {
    fn collect_decorations(
        &self,
        context: &rich_model::DecorationContext<'_>,
        writer: &mut rich_model::DecorationWriter<'_>,
    ) {
        if let Ok(mut visited) = self.visited_blocks.lock() {
            visited.push(context.block.id);
        }
        let len = context
            .block
            .kind
            .inline_content()
            .map(|inline| inline.len())
            .unwrap_or(0);
        if len > 0 {
            let _ = writer.push_inline(
                BlockByteRange::new(BlockByteOffset::new(0), BlockByteOffset::new(len.min(4))),
                InlineDecorationStyle::new().with_background(RgbaColor::rgb(255, 250, 180)),
            );
        }
    }
}

fn fixed_window_options(width: f32, height: f32) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(width), px(height)),
        ))),
        show: false,
        focus: false,
        ..WindowOptions::default()
    }
}

fn draw_window(window: gpui_kit::AnyWindowHandle, cx: &mut App) {
    window
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("draw window");
}

fn verify_two_views_badge_policy_middleware_and_clipboard(cx: &mut App) {
    let mut source_doc = Document::new();
    let paragraph_id =
        source_doc.add_paragraph(InlineContent::plain("Before "), BlockStyle::default());
    let mut callout_child = InlineContent::plain("Inside callout");
    callout_child.push_object(
        InlineObject::custom::<TagPillSpec>(TagPillData {
            tag: "v2".to_owned(),
        }),
        InlineStyle::default(),
    );
    let child_block = BlockNode::new(
        source_doc.alloc_block_id(),
        BlockKind::paragraph(callout_child),
        BlockStyle::default(),
    );
    source_doc.add_container_block::<CalloutSpec>(
        CalloutData {
            title: "Note".to_owned(),
        },
        vec![child_block],
    );
    source_doc.add_leaf_block::<SameNameBlockSpec>(SameNameBlockData {
        caption: "Shared spelling".to_owned(),
    });
    source_doc.add_paragraph(InlineContent::plain("After"), BlockStyle::default());

    let mut dest_doc = Document::new();
    let dest_para = dest_doc.add_paragraph(InlineContent::empty(), BlockStyle::default());

    let rename_calls = Arc::new(AtomicUsize::new(0));
    let read_calls = Arc::new(AtomicUsize::new(0));
    let replace_slot: Rc<RefCell<Option<RenameBadge>>> = Rc::new(RefCell::new(None));
    let events: Rc<RefCell<Vec<(String, CommandEffectKind, CommandResolution, CommandOutcome)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let mutations = Arc::new(AtomicUsize::new(0));

    let rename_target: Rc<RefCell<Option<(BlockId, InlineObjectId, String)>>> =
        Rc::new(RefCell::new(None));
    let rename_target_for_host = Rc::clone(&rename_target);
    let rename_for_build = Arc::clone(&rename_calls);
    let read_for_build = Arc::clone(&read_calls);
    let replace_for_build = Rc::clone(&replace_slot);
    let view_one_slot: Rc<RefCell<Option<Entity<RichDocumentView>>>> = Rc::new(RefCell::new(None));
    let view_one_capture = Rc::clone(&view_one_slot);
    let (window_one, _host_one) =
        gpui_kit::open_window(fixed_window_options(640.0, 480.0), cx, move |_window, cx| {
            let view = cx.new(|cx| {
                let mut view = RichDocumentView::new(source_doc, cx);
                view.set_policy(InteractionPolicy::editable(), cx);
                view.register_inline_renderer(StatusBadgeRenderer, cx)
                    .expect("register StatusBadgeRenderer");
                view.register_inline_renderer(TagPillRenderer, cx)
                    .expect("register TagPillRenderer");
                view.register_block_renderer(CalloutRenderer, cx)
                    .expect("register CalloutRenderer");
                view.register_block_renderer(SameNameBlockRenderer, cx)
                    .expect("same spelling in block namespace must not collide with inline");

                assert!(matches!(
                    view.register_inline_renderer(StatusBadgeRenderer, cx),
                    Err(ExtensionRegistrationError::DuplicateKey {
                        namespace: ExtensionNamespace::Inline,
                        ..
                    })
                ));
                assert!(matches!(
                    view.register_block_renderer(ReservedBlockRenderer, cx),
                    Err(ExtensionRegistrationError::ReservedKey {
                        namespace: ExtensionNamespace::Block,
                        ..
                    })
                ));
                assert!(matches!(
                    view.register_inline_renderer(ReservedInlineRenderer, cx),
                    Err(ExtensionRegistrationError::ReservedKey {
                        namespace: ExtensionNamespace::Inline,
                        ..
                    })
                ));

                configure_badge_commands(&mut view, rename_for_build, read_for_build);
                view.add_middleware(TestCommandMiddleware {
                    replace_count_with_rename: replace_for_build,
                });
                view
            });
            *view_one_capture.borrow_mut() = Some(view.clone());
            cx.new(|_| ConsumerHost {
                view,
                target: rename_target_for_host,
            })
        })
        .expect("open window one");
    let view_one = view_one_slot.borrow().clone().expect("view one entity");

    let events_for_sub = Rc::clone(&events);
    let mutations_for_sub = Arc::clone(&mutations);
    cx.subscribe(&view_one, move |_view, event, _cx| match event {
        RichDocumentEvent::CommandExecuted {
            command_id,
            effect,
            resolution,
            outcome,
            ..
        } => {
            events_for_sub.borrow_mut().push((
                (*command_id).to_owned(),
                *effect,
                *resolution,
                *outcome,
            ));
        }
        RichDocumentEvent::DocumentMutated { .. } => {
            mutations_for_sub.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    })
    .detach();

    // Insert a StatusBadgeSpec inline object at the end of the first paragraph and render it.
    let badge_object_id = view_one.update(cx, |view, cx| {
        let end_offset = view
            .document
            .find_block(paragraph_id)
            .and_then(|block| block.kind.inline_content())
            .map(|inline| inline.len())
            .unwrap_or(0);
        let pos = DocPos::top_level(paragraph_id, end_offset);
        view.dispatch_command(
            RichCommand::SetSelection(Some(DocumentSelection::caret(
                pos,
                Affinity::Downstream,
            ))),
            cx,
        );
        let insert_outcome = view.dispatch_command(
            RichCommand::InsertInlineObject(InlineObject::custom::<StatusBadgeSpec>(
                StatusBadgeData {
                    label: "Draft".to_owned(),
                    tone: "info".to_owned(),
                },
            )),
            cx,
        );
        assert_eq!(insert_outcome, CommandOutcome::Executed);
        find_first_badge(&view.document)
            .expect("inserted status badge")
            .1
    });

    draw_window(window_one, cx);

    // Consumer GPUI action renames the badge through the user dispatch path.
    *rename_target.borrow_mut() = Some((paragraph_id, badge_object_id, "Approved".to_owned()));
    window_one
        .update(cx, |_, window, cx| {
            let focus = view_one.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
            focus.dispatch_action(&RenameFirstBadge, window, cx);
        })
        .expect("dispatch RenameFirstBadge action");

    let (renamed_label, renamed_id) = {
        let view = view_one.read(cx);
        let (_, id, data) = find_first_badge(&view.document).expect("renamed badge");
        (data.label, id)
    };
    assert_eq!(renamed_label, "Approved");
    assert_eq!(renamed_id, badge_object_id);
    assert_eq!(rename_calls.load(Ordering::SeqCst), 1);

    // Undo restores "Draft", redo reapplies "Approved".
    view_one.update(cx, |view, cx| {
        assert_eq!(
            view.dispatch_command(RichCommand::Undo, cx),
            CommandOutcome::Executed
        );
    });
    assert_eq!(
        find_first_badge(&view_one.read(cx).document)
            .expect("undone badge")
            .2
            .label,
        "Draft"
    );

    view_one.update(cx, |view, cx| {
        assert_eq!(
            view.dispatch_command(RichCommand::Redo, cx),
            CommandOutcome::Executed
        );
    });
    assert_eq!(
        find_first_badge(&view_one.read(cx).document)
            .expect("redone badge")
            .2
            .label,
        "Approved"
    );

    // Reservation without returning a transaction must be a NoOp without mutating or adding history.
    let mutations_before_reserve = mutations.load(Ordering::SeqCst);
    view_one.update(cx, |view, cx| {
        assert_eq!(
            view.dispatch_extension_command(ReserveOnlyEdit, cx),
            CommandOutcome::NoOp
        );
    });
    assert_eq!(mutations.load(Ordering::SeqCst), mutations_before_reserve);

    // Unregistered and mismatched-type extension commands report distinct outcomes.
    view_one.update(cx, |view, cx| {
        assert_eq!(
            view.dispatch_extension_command(UnregisteredBadgeCommand, cx),
            CommandOutcome::UnregisteredCommand
        );
        assert_eq!(
            view.dispatch_extension_command(WrongTypeRenameBadge, cx),
            CommandOutcome::MismatchedCommandType
        );
    });

    // Middleware cancels forbidden rename args using checked `args::<RenameBadge>()`.
    let calls_before_cancel = rename_calls.load(Ordering::SeqCst);
    view_one.update(cx, |view, cx| {
        let outcome = view.dispatch_extension_command(
            RenameBadge {
                block_id: paragraph_id,
                object_id: badge_object_id,
                new_label: "FORBIDDEN".to_owned(),
            },
            cx,
        );
        assert_eq!(outcome, CommandOutcome::Cancelled);
    });
    assert_eq!(rename_calls.load(Ordering::SeqCst), calls_before_cancel);

    // Read-only mode blocks edit commands BEFORE invoking the planner, even when a ReadOnly
    // command is replaced with an Edit command by middleware.
    view_one.update(cx, |view, cx| {
        view.set_policy(InteractionPolicy::viewer(), cx);
        let direct_outcome = view.dispatch_extension_command(
            RenameBadge {
                block_id: paragraph_id,
                object_id: badge_object_id,
                new_label: "Blocked".to_owned(),
            },
            cx,
        );
        assert_eq!(direct_outcome, CommandOutcome::RejectedReadOnly);
    });
    assert_eq!(rename_calls.load(Ordering::SeqCst), calls_before_cancel);

    *replace_slot.borrow_mut() = Some(RenameBadge {
        block_id: paragraph_id,
        object_id: badge_object_id,
        new_label: "ReplacedEdit".to_owned(),
    });
    view_one.update(cx, |view, cx| {
        let replaced_outcome = view.dispatch_extension_command(CountBadges, cx);
        assert_eq!(replaced_outcome, CommandOutcome::RejectedReadOnly);
    });
    assert_eq!(rename_calls.load(Ordering::SeqCst), calls_before_cancel);
    assert_eq!(read_calls.load(Ordering::SeqCst), 0);
    *replace_slot.borrow_mut() = None;

    // Verify completion event recorded the resolved replacement command identity and effect.
    {
        let recorded_events = events.borrow();
        let last_event = recorded_events.last().expect("CommandExecuted event");
        assert_eq!(last_event.0, RenameBadge::ID);
        assert_eq!(last_event.1, CommandEffectKind::Edit);
        assert_eq!(last_event.2, CommandResolution::RegisteredExtension);
        assert_eq!(last_event.3, CommandOutcome::RejectedReadOnly);
    }

    // Copy from View 1 and paste into View 2; HTML formatter has no markers, so the structured
    // clipboard fragment must preserve the badge, callout, tag pill, and same-name leaf block.
    let copied_item = view_one.update(cx, |view, cx| {
        view.set_policy(InteractionPolicy::editable(), cx);
        assert_eq!(
            view.dispatch_command(RichCommand::SelectAll, cx),
            CommandOutcome::Executed
        );
        assert_eq!(
            view.dispatch_command(RichCommand::CopySelection, cx),
            CommandOutcome::Executed
        );
        view.clipboard_item_for_selection()
            .expect("clipboard item for selection")
    });

    // Confirm the HTML presentation in metadata omits extension markers while carrying the badge label.
    let metadata_json: serde_json::Value = serde_json::from_str(
        copied_item
            .metadata()
            .expect("clipboard item carries metadata"),
    )
    .expect("valid metadata json");
    let html_meta = metadata_json
        .get("gpui_rich_html")
        .and_then(serde_json::Value::as_str)
        .expect("html metadata");
    assert!(
        !html_meta.contains("data-gpui-rich-custom-inline=\"acme.status_badge\""),
        "badge HTML formatter intentionally omits extension markers: {html_meta}"
    );
    assert!(html_meta.contains("<span>Approved</span>"));

    let (window_two, view_two) =
        gpui_kit::open_window(fixed_window_options(640.0, 480.0), cx, move |_window, cx| {
            cx.new(|cx| {
                let mut view = RichDocumentView::new(dest_doc, cx);
                view.set_policy(InteractionPolicy::editable(), cx);
                view.register_inline_renderer(StatusBadgeRenderer, cx)
                    .expect("register StatusBadgeRenderer on view two");
                view.register_inline_renderer(TagPillRenderer, cx)
                    .expect("register TagPillRenderer on view two");
                view.register_block_renderer(CalloutRenderer, cx)
                    .expect("register CalloutRenderer on view two");
                view.register_block_renderer(SameNameBlockRenderer, cx)
                    .expect("register SameNameBlockRenderer on view two");
                view
            })
        })
        .expect("open window two");

    view_two.update(cx, |view, cx| {
        view.dispatch_command(
            RichCommand::SetSelection(Some(DocumentSelection::caret(
                DocPos::top_level(dest_para, 0),
                Affinity::Downstream,
            ))),
            cx,
        );
        assert_eq!(
            view.paste_clipboard_item(&copied_item, false, cx),
            CommandOutcome::Executed
        );
    });
    draw_window(window_two, cx);

    let view_two_ref = view_two.read(cx);
    let (_, _, pasted_badge) = find_first_badge(&view_two_ref.document)
        .expect("structured paste preserved status badge despite stripped HTML");
    assert_eq!(pasted_badge.label, "Approved");
    assert_eq!(pasted_badge.tone, "info");
    assert!(
        view_two_ref
            .document
            .blocks
            .iter()
            .any(|block| block.read_data::<CalloutSpec>().is_ok()),
        "structured paste preserved custom callout container"
    );
    assert!(
        view_two_ref
            .document
            .blocks
            .iter()
            .any(|block| block.read_data::<SameNameBlockSpec>().is_ok()),
        "structured paste preserved custom leaf block"
    );

    // Plain-text paste must ignore private structured metadata.
    let mut plain_doc = Document::new();
    let plain_para = plain_doc.add_paragraph(InlineContent::empty(), BlockStyle::default());
    let (window_three, view_three) =
        gpui_kit::open_window(fixed_window_options(640.0, 480.0), cx, move |_window, cx| {
            cx.new(|cx| {
                let mut view = RichDocumentView::new(plain_doc, cx);
                view.set_policy(InteractionPolicy::editable(), cx);
                view
            })
        })
        .expect("open window three");

    view_three.update(cx, |view, cx| {
        view.dispatch_command(
            RichCommand::SetSelection(Some(DocumentSelection::caret(
                DocPos::top_level(plain_para, 0),
                Affinity::Downstream,
            ))),
            cx,
        );
        assert_eq!(
            view.paste_clipboard_item(&copied_item, true, cx),
            CommandOutcome::Executed
        );
    });
    draw_window(window_three, cx);

    assert!(
        find_first_badge(&view_three.read(cx).document).is_none(),
        "plain-text paste must ignore private structured clipboard metadata"
    );
}

fn verify_payload_cache_unknown_fields_and_diagnostics(cx: &mut App) {
    DECODE_COUNTER.store(0, Ordering::SeqCst);

    // Construct a persisted JSON document containing:
    // 1. Valid raw payload with an extra unknown future field
    // 2. Corrupt payload for the same registered kind
    // 3. Wrong shape (Container instead of Leaf) for the same registered kind
    // 4. Unregistered custom leaf kind
    let mut template = Document::new();
    let valid_raw_id = template.add_leaf_block::<CountingLeafSpec>(CountingPayload {
        title: "placeholder".to_owned(),
    });
    let corrupt_registered_id = template.add_leaf_block::<CountingLeafSpec>(CountingPayload {
        title: "placeholder".to_owned(),
    });
    let wrong_shape_registered_id = template.add_container_block::<CalloutSpec>(
        CalloutData {
            title: "placeholder".to_owned(),
        },
        vec![],
    );
    let unregistered_id = template.add_leaf_block::<SameNameBlockSpec>(SameNameBlockData {
        caption: "placeholder".to_owned(),
    });

    let mut raw_doc_json: serde_json::Value =
        serde_json::from_str(JsonCodec.encode(&template).expect("encode template").as_str())
            .expect("parse template json");
    let blocks = raw_doc_json
        .get_mut("blocks")
        .and_then(serde_json::Value::as_array_mut)
        .expect("blocks array");
    blocks[0]["kind"]["Leaf"]["data"] = json!({
        "title": "Cached",
        "unknown_future_field": 42
    });
    blocks[1]["kind"]["Leaf"]["data"] = json!({ "wrong_field": true });
    blocks[2]["kind"]["Container"]["kind"] = json!(CountingLeafSpec::KIND);
    blocks[2]["kind"]["Container"]["data"] = json!({ "title": "WrongShape" });
    blocks[3]["kind"]["Leaf"]["kind"] = json!("acme.unregistered_widget");
    blocks[3]["kind"]["Leaf"]["data"] = json!({ "anything": [1, 2, 3] });

    let document = JsonCodec
        .decode(&EncodedDocument::from_raw(raw_doc_json.to_string()))
        .expect("decode persisted raw json document");

    let view_slot: Rc<RefCell<Option<Entity<RichDocumentView>>>> = Rc::new(RefCell::new(None));
    let view_capture = Rc::clone(&view_slot);
    let window = cx
        .open_window(fixed_window_options(640.0, 480.0), move |_window, cx| {
            let view = cx.new(|cx| {
                let mut view = RichDocumentView::new(document, cx);
                view.register_block_renderer(CountingLeafRenderer, cx)
                    .expect("register CountingLeafRenderer");
                view
            });
            *view_capture.borrow_mut() = Some(view.clone());
            view
        })
        .expect("open diagnostics window");
    let view = view_slot.borrow().clone().expect("diagnostics view");

    draw_window(window.into(), cx);

    let view_ref = view.read(cx);
    let block = view_ref.document.find_block(valid_raw_id).expect("valid block");
    let BlockKind::Leaf { kind, data } = &block.kind else {
        panic!("expected BlockKind::Leaf");
    };
    let clipboard_plain = view_ref
        .extensions()
        .plain_text_block(BlockShapeKind::Leaf, kind, data);
    assert_eq!(clipboard_plain.as_deref(), Some("Cached"));

    // Valid raw block decoded once across measure, render, and clipboard; corrupt raw block
    // attempted decode once and cached its error. Wrong-shape block never decoded.
    assert_eq!(DECODE_COUNTER.load(Ordering::SeqCst), 2);

    let exported = view_ref.export_document(&JsonCodec).expect("export json");
    assert!(exported.as_str().contains("\"unknown_future_field\": 42"));

    let diagnostics = view_ref.extension_diagnostics();
    assert!(
        !diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.target,
            ExtensionNodeTarget::Block { block_id } if block_id == unregistered_id
        )),
        "unregistered kind must not produce a diagnostic"
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.target
            == ExtensionNodeTarget::Block {
                block_id: corrupt_registered_id,
            }
            && matches!(
                diagnostic.detail,
                ExtensionDiagnosticKind::InvalidPayload { .. }
            )
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.target
            == ExtensionNodeTarget::Block {
                block_id: wrong_shape_registered_id,
            }
            && matches!(
                diagnostic.detail,
                ExtensionDiagnosticKind::ShapeMismatch {
                    expected: BlockShapeKind::Leaf,
                    actual: BlockShapeKind::Container,
                }
            )
    }));
}

fn verify_large_table_decoration_culling(cx: &mut App) {
    let mut document = Document::new();
    let mut rows = Vec::new();
    let mut first_row_cell_block = None;
    let mut last_row_cell_block = None;

    for row_index in 0..80 {
        let cell_id = document.alloc_block_id();
        let cell_block_id = document.alloc_block_id();
        if row_index == 0 {
            first_row_cell_block = Some(cell_block_id);
        }
        if row_index == 79 {
            last_row_cell_block = Some(cell_block_id);
        }
        let cell_paragraph = BlockNode::new(
            cell_block_id,
            BlockKind::paragraph(InlineContent::plain(format!(
                "Table row {row_index} content"
            ))),
            BlockStyle::default(),
        );
        rows.push(TableRow {
            cells: vec![TableCell::simple(cell_id, vec![cell_paragraph])],
            is_header: row_index == 0,
        });
    }
    let table_id = document.alloc_block_id();
    document.push_block(BlockNode::new(
        table_id,
        BlockKind::table(TableBlock {
            columns: vec![ColumnSpec::Ratio(1.0)],
            column_kinds: std::collections::BTreeMap::new(),
            rows,
            border_style: TableBorderStyle::default(),
        }),
        BlockStyle::default(),
    ));

    let visited_blocks = Arc::new(Mutex::new(Vec::new()));
    let visited_for_provider = Arc::clone(&visited_blocks);

    let window = cx
        .open_window(fixed_window_options(640.0, 240.0), move |_window, cx| {
            cx.new(|cx| {
                RichDocumentView::new(document, cx).with_decoration_provider(RecordingDecorator {
                    visited_blocks: visited_for_provider,
                })
            })
        })
        .expect("open table window");

    draw_window(window.into(), cx);

    let visited = visited_blocks.lock().expect("visited lock");
    let first_id = first_row_cell_block.expect("first cell block id");
    let last_id = last_row_cell_block.expect("last cell block id");
    assert!(
        visited.contains(&first_id),
        "visible top table cell must be passed to decoration providers"
    );
    assert!(
        !visited.contains(&last_id),
        "off-screen bottom table cell must be culled before invoking decoration providers"
    );
}

#[test]
fn gpui_extension_type_safety_end_to_end() {
    let app = gpui_kit::platform::headless();

    let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let failure_for_run = Arc::clone(&failure);

    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        register_default_keybindings(cx);

        let sync_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verify_two_views_badge_policy_middleware_and_clipboard(cx);
            verify_payload_cache_unknown_fields_and_diagnostics(cx);
            verify_large_table_decoration_culling(cx);
        }));
        if let Err(payload) = sync_result {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "panic in synchronous GPUI test".to_owned());
            if let Ok(mut slot) = failure_for_run.lock() {
                *slot = Some(message);
            }
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
            return;
        }

        // Start async missing vs undecodable image loading scenario.
        let missing_id = ImageAssetId::new("missing.png");
        let broken_id = ImageAssetId::new("broken.png");
        let provider = MemoryAssetProvider::new();
        provider.insert_encoded(
            broken_id.clone(),
            gpui_kit::ImageFormat::Png,
            b"not a valid png payload".to_vec(),
        );

        let mut image_doc = Document::new();
        let mut inline = InlineContent::empty();
        inline.push_object(
            InlineObject::Image {
                asset_id: missing_id.clone(),
                width_pt: 40.0,
                height_pt: 40.0,
                alt: "missing".to_owned(),
                valign: InlineVAlign::Baseline,
            },
            InlineStyle::default(),
        );
        inline.push_object(
            InlineObject::Image {
                asset_id: broken_id.clone(),
                width_pt: 40.0,
                height_pt: 40.0,
                alt: "broken".to_owned(),
                valign: InlineVAlign::Baseline,
            },
            InlineStyle::default(),
        );
        image_doc.add_paragraph(inline, BlockStyle::default());
        let initial_snapshot = JsonCodec
            .encode(&image_doc)
            .expect("initial json")
            .into_string();

        let image_view_slot: Rc<RefCell<Option<Entity<RichDocumentView>>>> =
            Rc::new(RefCell::new(None));
        let image_view_capture = Rc::clone(&image_view_slot);
        let image_window = cx
            .open_window(fixed_window_options(640.0, 240.0), move |_window, cx| {
                let view = cx.new(|cx| {
                    RichDocumentView::new(image_doc, cx).with_asset_provider(provider)
                });
                *image_view_capture.borrow_mut() = Some(view.clone());
                view
            })
            .expect("open image window");
        let image_view = image_view_slot.borrow().clone().expect("image view");
        draw_window(image_window.into(), cx);

        let failure_for_async = Arc::clone(&failure_for_run);
        cx.spawn(async move |cx| {
            // Wait up to 2 seconds for background decode tasks to settle.
            for _ in 0..40 {
                let settled = cx.update(|cx| {
                    let view = image_view.read(cx);
                    matches!(view.asset_state(&missing_id), Some(LoadedAsset::Failed(_)))
                        && matches!(view.asset_state(&broken_id), Some(LoadedAsset::Failed(_)))
                });
                if settled {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }

            let async_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cx.update(|cx| {
                    let view = image_view.read(cx);
                    match view.asset_state(&missing_id) {
                        Some(LoadedAsset::Failed(ImageAssetError::NotFound {
                            asset_id, ..
                        })) => {
                            assert_eq!(asset_id, &missing_id);
                        }
                        other => {
                            panic!("expected NotFound failure for missing asset, got {other:?}")
                        }
                    }
                    match view.asset_state(&broken_id) {
                        Some(LoadedAsset::Failed(
                            error @ ImageAssetError::Decode { asset_id, .. },
                        )) => {
                            assert_eq!(asset_id, &broken_id);
                            assert!(error.source().is_some(), "decode failure must retain cause");
                        }
                        other => {
                            panic!("expected Decode failure for broken asset, got {other:?}")
                        }
                    }
                    assert!(
                        !view.history.can_undo(),
                        "asset loading must not add history"
                    );
                    let after_snapshot = JsonCodec
                        .encode(&view.document)
                        .expect("after json")
                        .into_string();
                    assert_eq!(
                        after_snapshot, initial_snapshot,
                        "asset loading must not mutate document"
                    );
                });
            }));

            if let Err(payload) = async_result {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "panic in async GPUI test".to_owned());
                if let Ok(mut slot) = failure_for_async.lock() {
                    *slot = Some(message);
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });

    if let Some(message) = failure.lock().expect("failure lock").take() {
        panic!("GPUI integration test failed: {message}");
    }
}
