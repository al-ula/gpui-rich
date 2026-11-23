//! Component command execution and custom inline display resolution.
use super::*;
use gpui_kit::StatefulInteractiveElement;
use rich_model::{
    CustomPayload, InlineContent, InlineObject, Transaction,
    components::{
        BuildingBlock, ComponentCommand, DROPDOWN_KIND, DropdownOption, PLACEHOLDER_KIND,
        VARIABLE_KIND,
    },
};

pub(super) fn execute_component_command(
    view: &mut RichDocumentView,
    command: &ComponentCommand,
    cx: &mut Context<RichDocumentView>,
) -> CommandOutcome {
    use ComponentCommand as C;
    let mut steps = Vec::new();
    match command {
        C::DefineDropdown(definition) => {
            let mut definitions = view.document.components.clone();
            definitions
                .dropdowns
                .insert(definition.id.clone(), definition.clone());
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::RenameDropdownOption {
            definition_id,
            option_id,
            label,
        } => {
            let mut definitions = view.document.components.clone();
            let Some(option) = definitions
                .dropdowns
                .get_mut(definition_id)
                .and_then(|d| d.options.iter_mut().find(|o| o.id == *option_id))
            else {
                return CommandOutcome::NoOp;
            };
            if option.label == *label {
                return CommandOutcome::NoOp;
            }
            option.label = label.clone();
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::ReorderDropdownOptions {
            definition_id,
            option_ids,
        } => {
            let mut definitions = view.document.components.clone();
            let Some(definition) = definitions.dropdowns.get_mut(definition_id) else {
                return CommandOutcome::NoOp;
            };
            if option_ids.len() != definition.options.len() {
                return CommandOutcome::NoOp;
            }
            let mut reordered = Vec::with_capacity(option_ids.len());
            for id in option_ids {
                let Some(option) = definition.options.iter().find(|o| o.id == *id).cloned() else {
                    return CommandOutcome::NoOp;
                };
                if reordered
                    .iter()
                    .any(|entry: &DropdownOption| entry.id == *id)
                {
                    return CommandOutcome::NoOp;
                }
                reordered.push(option);
            }
            if reordered == definition.options {
                return CommandOutcome::NoOp;
            }
            definition.options = reordered;
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::RemoveDropdownOption {
            definition_id,
            option_id,
            replacement_option_id,
        } => {
            let mut definitions = view.document.components.clone();
            let Some(definition) = definitions.dropdowns.get_mut(definition_id) else {
                return CommandOutcome::NoOp;
            };
            if !definition.options.iter().any(|o| o.id == *option_id) {
                return CommandOutcome::NoOp;
            }
            let selected = selected_dropdown_objects(&view.document, definition_id, option_id);
            if !selected.is_empty() {
                let Some(replacement) = replacement_option_id else {
                    return CommandOutcome::NoOp;
                };
                if replacement == option_id
                    || !definition.options.iter().any(|o| o.id == *replacement)
                {
                    return CommandOutcome::NoOp;
                }
                for (block_id, object_id, object) in selected {
                    let mut data = object_data(&object);
                    data["selected_option_id"] = replacement.clone().into();
                    steps.push(Step::UpdateInlineObject {
                        block_id,
                        object_id,
                        object: ComponentCommand::dropdown_object(data),
                    });
                }
            }
            definition.options.retain(|o| o.id != *option_id);
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::InsertDropdown {
            block_id,
            offset,
            definition_id,
            selected_option_id,
        } => {
            if view
                .document
                .components
                .dropdown_label(definition_id, selected_option_id)
                .is_none()
                || !valid_inline_offset(&view.document, *block_id, *offset)
            {
                return CommandOutcome::NoOp;
            }
            let mut content = InlineContent::empty();
            content.push_object(
                ComponentCommand::dropdown_object(ComponentCommand::dropdown_payload(
                    definition_id.clone(),
                    selected_option_id.clone(),
                )),
                Default::default(),
            );
            steps.push(Step::InsertContent {
                block_id: *block_id,
                offset: *offset,
                content,
            });
        }
        C::SelectDropdownOption {
            block_id,
            object_id,
            option_id,
        } => {
            let Some(block) = view.document.find_block(*block_id) else {
                return CommandOutcome::NoOp;
            };
            let Some(entry) = block
                .kind
                .inline_content()
                .and_then(|c| c.objects.iter().find(|e| e.id == *object_id))
            else {
                return CommandOutcome::NoOp;
            };
            let InlineObject::Custom { kind, data } = &entry.object else {
                return CommandOutcome::NoOp;
            };
            if kind.as_ref() != DROPDOWN_KIND {
                return CommandOutcome::NoOp;
            }
            let Ok(mut payload) = data.to_json() else {
                return CommandOutcome::NoOp;
            };
            let definition_id = payload
                .get("definition_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if view
                .document
                .components
                .dropdown_label(definition_id, option_id)
                .is_none()
            {
                return CommandOutcome::NoOp;
            }
            if payload
                .get("selected_option_id")
                .and_then(serde_json::Value::as_str)
                == Some(option_id)
            {
                return CommandOutcome::NoOp;
            }
            payload["selected_option_id"] = option_id.clone().into();
            steps.push(Step::UpdateInlineObject {
                block_id: *block_id,
                object_id: *object_id,
                object: ComponentCommand::dropdown_object(payload),
            });
        }
        C::DefineVariable(definition) => {
            let mut definitions = view.document.components.clone();
            definitions
                .variables
                .insert(definition.id.clone(), definition.clone());
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::SetVariableValue { variable_id, value } => {
            let mut definitions = view.document.components.clone();
            let Some(definition) = definitions.variables.get_mut(variable_id) else {
                return CommandOutcome::NoOp;
            };
            if definition.value == *value {
                return CommandOutcome::NoOp;
            }
            definition.value = value.clone();
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::RemoveVariable { variable_id } => {
            let mut definitions = view.document.components.clone();
            if definitions.variables.remove(variable_id).is_none() {
                return CommandOutcome::NoOp;
            }
            let mut references = Vec::new();
            for block in &view.document.blocks {
                collect_variable_refs(block, variable_id, &mut references);
            }
            references.sort_by(|a, b| a.0.0.cmp(&b.0.0).then_with(|| b.1.cmp(&a.1)));
            for (block_id, offset) in references {
                steps.push(Step::DeleteRange {
                    block_id,
                    range: offset..offset + '\u{fffc}'.len_utf8(),
                });
            }
            steps.push(Step::SetComponents {
                components: definitions,
            });
        }
        C::InsertVariable {
            block_id,
            offset,
            variable_id,
        } => {
            let Some(definition) = view.document.components.variables.get(variable_id) else {
                return CommandOutcome::NoOp;
            };
            if !valid_inline_offset(&view.document, *block_id, *offset) {
                return CommandOutcome::NoOp;
            }
            let mut content = InlineContent::empty();
            content.push_object(
                ComponentCommand::variable_object(ComponentCommand::variable_payload(
                    variable_id.clone(),
                    definition.value.clone(),
                )),
                Default::default(),
            );
            steps.push(Step::InsertContent {
                block_id: *block_id,
                offset: *offset,
                content,
            });
        }
        C::InsertPlaceholder {
            block_id,
            offset,
            kind,
            label,
        } => {
            if !valid_inline_offset(&view.document, *block_id, *offset) {
                return CommandOutcome::NoOp;
            }
            let mut content = InlineContent::empty();
            content.push_object(kind.object(label), Default::default());
            steps.push(Step::InsertContent {
                block_id: *block_id,
                offset: *offset,
                content,
            });
        }
        C::FillPlaceholder {
            block_id,
            object_id,
            value,
        } => {
            let Some(block) = view.document.find_block(*block_id) else {
                return CommandOutcome::NoOp;
            };
            let Some(entry) = block
                .kind
                .inline_content()
                .and_then(|c| c.objects.iter().find(|e| e.id == *object_id))
            else {
                return CommandOutcome::NoOp;
            };
            let InlineObject::Custom { kind, data } = &entry.object else {
                return CommandOutcome::NoOp;
            };
            if kind.as_ref() != PLACEHOLDER_KIND {
                return CommandOutcome::NoOp;
            }
            let Ok(mut payload) = data.to_json() else {
                return CommandOutcome::NoOp;
            };
            if payload.get("value").and_then(serde_json::Value::as_str) == Some(value) {
                return CommandOutcome::NoOp;
            }
            payload["value"] = value.clone().into();
            steps.push(Step::UpdateInlineObject {
                block_id: *block_id,
                object_id: *object_id,
                object: rich_model::PlaceholderKind::filled_object(payload),
            });
        }
        C::CaptureBuildingBlock { id, name, blocks } => {
            let mut components = view.document.components.clone();
            components.building_blocks.insert(
                id.clone(),
                BuildingBlock {
                    id: id.clone(),
                    name: name.clone(),
                    blocks: blocks.clone(),
                },
            );
            steps.push(Step::SetComponents { components });
        }
        C::CaptureSelectionAsBuildingBlock { id, name } => {
            let Some(fragment) = view.selected_fragment() else {
                return CommandOutcome::NoOp;
            };
            let blocks = fragment.to_block_nodes(&mut Document::new());
            let mut components = view.document.components.clone();
            components.building_blocks.insert(
                id.clone(),
                BuildingBlock {
                    id: id.clone(),
                    name: name.clone(),
                    blocks,
                },
            );
            steps.push(Step::SetComponents { components });
        }
        C::InsertBuildingBlock { id, target, index } => {
            let Some(templates) = view
                .document
                .components
                .building_blocks
                .get(id)
                .map(|f| f.blocks.clone())
            else {
                return CommandOutcome::NoOp;
            };
            let mut lists = HashMap::new();
            let mut next_list = view.document.alloc_list_id().0;
            for (offset, mut block) in templates.into_iter().enumerate() {
                assign_fresh_block_ids(&mut view.document, &mut block, &mut lists, &mut next_list);
                steps.push(Step::InsertBlockIn {
                    target: *target,
                    index: index + offset,
                    block,
                });
            }
        }
    }
    if steps.is_empty() {
        return CommandOutcome::NoOp;
    }
    let change = view.apply_transaction_grouped(
        Transaction::new(steps),
        Some(HistoryMergeGroup::Atomic),
        cx,
    );
    if change.dirty_blocks.is_empty() && !change.structure_changed {
        CommandOutcome::NoOp
    } else {
        CommandOutcome::Executed
    }
}

pub(crate) fn build_dropdown_options(
    popover: &ActiveSmartChipPopover,
    view: gpui_kit::Entity<RichDocumentView>,
    theme: &RichTheme,
    cx: &mut gpui_kit::App,
) -> Option<(gpui_kit::AnyElement, usize)> {
    let this = view.read(cx);
    let (_, selected_id, options) =
        dropdown_options_for_inline(&this.document, popover.block_id, popover.object_id)?;
    let editing = this.policy.editing;
    let can_choose = editing.enabled && editing.components;
    let option_count = options.len();
    let mut menu = gpui_kit::div()
        .id("rich-dropdown-options")
        .p(gpui_kit::px(6.0))
        .rounded(gpui_kit::px(7.0))
        .bg(theme.surface_background)
        .border_1()
        .border_color(theme.paper_border)
        .flex()
        .max_h(gpui_kit::px(240.0))
        .overflow_y_scroll()
        .flex_col();
    for option in options {
        let selected = option.id == selected_id;
        let row = gpui_kit::div()
            .id(format!("rich-dropdown-option-{}", option.id))
            .px(gpui_kit::px(8.0))
            .py(gpui_kit::px(5.0))
            .rounded(gpui_kit::px(4.0))
            .text_color(if selected { theme.accent } else { theme.text })
            .child(option.label);
        menu = if can_choose {
            let view_for_option = view.clone();
            let block_id = popover.block_id;
            let object_id = popover.object_id;
            let option_id = option.id;
            menu.child(row.cursor_pointer().on_click(move |_, _, cx| {
                view_for_option.update(cx, |this, cx| {
                    this.dispatch_command(
                        RichCommand::Component(ComponentCommand::SelectDropdownOption {
                            block_id,
                            object_id,
                            option_id: option_id.clone(),
                        }),
                        cx,
                    );
                    this.dismiss_chip_popover(cx);
                });
            }))
        } else {
            menu.child(row)
        };
    }
    Some((menu.into_any_element(), option_count))
}
pub(super) fn dropdown_options_for_inline(
    document: &Document,
    block_id: BlockId,
    object_id: InlineObjectId,
) -> Option<(String, String, Vec<DropdownOption>)> {
    let block = document.find_block(block_id)?;
    let entry = block
        .kind
        .inline_content()?
        .objects
        .iter()
        .find(|entry| entry.id == object_id)?;
    let InlineObject::Custom { kind, data } = &entry.object else {
        return None;
    };
    if kind.as_ref() != DROPDOWN_KIND {
        return None;
    }
    let json = data.to_json().ok()?;
    let definition_id = json.get("definition_id")?.as_str()?.to_owned();
    let selected_id = json.get("selected_option_id")?.as_str()?.to_owned();
    let definition = document.components.dropdowns.get(&definition_id)?;
    Some((definition_id, selected_id, definition.options.clone()))
}

fn valid_inline_offset(document: &Document, block_id: BlockId, offset: usize) -> bool {
    document
        .find_block(block_id)
        .and_then(|block| block.kind.inline_content())
        .is_some_and(|content| {
            offset <= content.text.len() && content.text.is_char_boundary(offset)
        })
}

fn object_data(object: &InlineObject) -> serde_json::Value {
    match object {
        InlineObject::Custom { data, .. } => data.to_json().unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

fn selected_dropdown_objects(
    document: &Document,
    definition: &str,
    option: &str,
) -> Vec<(BlockId, InlineObjectId, InlineObject)> {
    let mut selected = Vec::new();
    for block in &document.blocks {
        collect_dropdowns(block, definition, option, &mut selected);
    }
    selected
}
fn collect_dropdowns(
    block: &BlockNode,
    definition: &str,
    option: &str,
    out: &mut Vec<(BlockId, InlineObjectId, InlineObject)>,
) {
    match &block.kind {
        BlockKind::Text { content, .. } => {
            for entry in &content.objects {
                if let InlineObject::Custom { kind, data } = &entry.object
                    && let Ok(value) = data.to_json()
                    && kind.as_ref() == DROPDOWN_KIND
                    && value
                        .get("definition_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(definition)
                    && value
                        .get("selected_option_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(option)
                {
                    out.push((block.id, entry.id, entry.object.clone()));
                }
            }
        }
        BlockKind::Container { children, .. } => {
            for child in children {
                collect_dropdowns(child, definition, option, out);
            }
        }
        BlockKind::Grid { table, .. } => {
            for row in &table.rows {
                for cell in &row.cells {
                    for child in &cell.blocks {
                        collect_dropdowns(child, definition, option, out);
                    }
                }
            }
        }
        BlockKind::Leaf { .. } => {}
    }
}
fn collect_variable_refs(block: &BlockNode, variable_id: &str, out: &mut Vec<(BlockId, usize)>) {
    match &block.kind {
        BlockKind::Text { content, .. } => {
            for entry in &content.objects {
                if let InlineObject::Custom { kind, data } = &entry.object
                    && kind.as_ref() == VARIABLE_KIND
                    && data
                        .to_json()
                        .ok()
                        .and_then(|value| {
                            value
                                .get("variable_id")
                                .and_then(serde_json::Value::as_str)
                                .map(|id| id == variable_id)
                        })
                        .unwrap_or(false)
                {
                    out.push((block.id, entry.byte_offset));
                }
            }
        }
        BlockKind::Container { children, .. } => {
            for child in children {
                collect_variable_refs(child, variable_id, out);
            }
        }
        BlockKind::Grid { table, .. } => {
            for row in &table.rows {
                for cell in &row.cells {
                    for child in &cell.blocks {
                        collect_variable_refs(child, variable_id, out);
                    }
                }
            }
        }
        BlockKind::Leaf { .. } => {}
    }
}

fn assign_fresh_block_ids(
    document: &mut Document,
    block: &mut BlockNode,
    lists: &mut HashMap<rich_model::ListId, rich_model::ListId>,
    next_list: &mut u64,
) {
    block.id = document.alloc_block_id();
    block.version = 1;
    if let Some(mut item) = block.kind.list_item_data() {
        item.list_id = *lists.entry(item.list_id).or_insert_with(|| {
            let id = rich_model::ListId(*next_list);
            *next_list += 1;
            id
        });
        *block.kind.data_mut() = CustomPayload::typed(item);
    }
    match &mut block.kind {
        BlockKind::Container { children, .. } => {
            for child in children {
                assign_fresh_block_ids(document, child, lists, next_list);
            }
        }
        BlockKind::Grid { table, .. } => {
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                cell.id = document.alloc_block_id();
                for child in &mut cell.blocks {
                    assign_fresh_block_ids(document, child, lists, next_list);
                }
            }
        }
        BlockKind::Text { content, .. } if !content.objects.is_empty() => {
            let mut fresh = InlineContent::empty();
            fresh.insert_content(0, content);
            *content = fresh;
        }
        _ => {}
    }
}
