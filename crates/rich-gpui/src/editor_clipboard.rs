//! Clipboard-only helpers shared by copy and paste command paths.

use super::RichDocumentView;
use rich_model::{
    AnchorId, ClipboardBlock, ClipboardCustomFormatter, ClipboardFragment, ComponentStore,
    CustomPayload, DROPDOWN_KIND, InlineObject, PLACEHOLDER_KIND, VARIABLE_KIND,
};
use serde_json::Value;
use std::cell::OnceCell;
use std::collections::HashMap;

pub(super) struct ComponentClipboardFormatter<'a> {
    view: &'a RichDocumentView,
    note_numbers: OnceCell<HashMap<AnchorId, u32>>,
}

impl<'a> ComponentClipboardFormatter<'a> {
    pub(super) fn new(view: &'a RichDocumentView) -> Self {
        Self {
            view,
            note_numbers: OnceCell::new(),
        }
    }

    fn footnote_label(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        if kind != rich_model::FOOTNOTE_REFERENCE_KIND {
            return None;
        }
        let id = rich_model::footnote_reference_id(&InlineObject::custom(kind, data.clone()))?;
        let numbers = self.note_numbers.get_or_init(|| {
            self.view
                .document
                .anchors
                .footnote_number_map(&self.view.document)
        });
        Some(
            numbers
                .get(&id)
                .map_or_else(|| "[?]".to_owned(), |number| format!("[{number}]")),
        )
    }
}

impl ClipboardCustomFormatter for ComponentClipboardFormatter<'_> {
    fn plain_text_block(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.view.extensions.plain_text_block(kind, data)
    }

    fn html_block(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        self.view.extensions.html_block(kind, data)
    }

    fn plain_text_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        if matches!(kind, DROPDOWN_KIND | VARIABLE_KIND | PLACEHOLDER_KIND) {
            return Some(
                self.view
                    .document
                    .components
                    .inline_fallback(kind, &data.to_json()),
            );
        }
        self.footnote_label(kind, data)
            .or_else(|| self.view.extensions.plain_text_inline(kind, data))
    }

    fn html_inline(&self, kind: &str, data: &CustomPayload) -> Option<String> {
        if matches!(kind, DROPDOWN_KIND | VARIABLE_KIND | PLACEHOLDER_KIND) {
            let store = &self.view.document.components;
            let mut payload = data.to_json();
            let label = store.inline_fallback(kind, &payload);
            let definition = match kind {
                DROPDOWN_KIND => {
                    store.export_dropdown_metadata(payload.get("definition_id")?.as_str()?)
                }
                VARIABLE_KIND => {
                    store.export_variable_metadata(payload.get("variable_id")?.as_str()?)
                }
                _ => None,
            };
            if let Some(definition) = definition {
                payload["clipboard_definition"] = definition;
            }
            return Some(format!(
                "<span data-gpui-rich-custom-inline=\"{kind}\" data-gpui-rich-payload=\"{}\">{}</span>",
                escape_html(&payload.to_string()),
                escape_html(&label),
            ));
        }
        self.footnote_label(kind, data)
            .map(|label| format!("<sup>{label}</sup>"))
            .or_else(|| self.view.extensions.html_inline(kind, data))
    }
}

pub(super) fn import_component_definitions(
    store: &ComponentStore,
    fragment: &mut ClipboardFragment,
) -> Option<ComponentStore> {
    let mut imported = None;
    let mut mapping = HashMap::<(bool, String), String>::new();
    for_custom_objects_mut(&mut fragment.blocks, &mut |kind, data| {
        let dropdown = match kind {
            DROPDOWN_KIND => true,
            VARIABLE_KIND => false,
            _ => return,
        };
        let mut payload = data.to_json();
        let Some(object) = payload.as_object_mut() else {
            return;
        };
        let Some(definition) = object.remove("clipboard_definition") else {
            return;
        };
        let key = if dropdown {
            "definition_id"
        } else {
            "variable_id"
        };
        let Some(id) = object.get(key).and_then(Value::as_str) else {
            return;
        };
        let scoped_id = (dropdown, id.to_owned());
        let resolved_id = if let Some(existing) = mapping.get(&scoped_id) {
            existing.clone()
        } else {
            let next = imported.get_or_insert_with(|| store.clone());
            let imported_id = if dropdown {
                next.import_dropdown_metadata(&definition)
            } else {
                next.import_variable_metadata(&definition)
            };
            let Some(imported_id) = imported_id else {
                return;
            };
            mapping.insert(scoped_id, imported_id.clone());
            imported_id
        };
        object.insert(key.to_owned(), Value::String(resolved_id));
        *data = CustomPayload::from_json(payload);
    });
    imported.filter(|next| next != store)
}

fn for_custom_objects_mut(
    blocks: &mut [ClipboardBlock],
    f: &mut impl FnMut(&str, &mut CustomPayload),
) {
    for block in blocks {
        match block {
            ClipboardBlock::Paragraph { content, .. }
            | ClipboardBlock::Heading { content, .. }
            | ClipboardBlock::ListItem { content, .. }
            | ClipboardBlock::CodeBlock { content, .. }
            | ClipboardBlock::CustomText { content, .. } => {
                for entry in &mut content.objects {
                    if let InlineObject::Custom { kind, data } = &mut entry.object {
                        f(kind, data);
                    }
                }
            }
            ClipboardBlock::Blockquote(children)
            | ClipboardBlock::CustomContainer { children, .. } => {
                for_custom_objects_mut(children, f);
            }
            ClipboardBlock::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        for_custom_objects_mut(&mut cell.blocks, f);
                    }
                }
            }
            ClipboardBlock::HorizontalRule | ClipboardBlock::Custom { .. } => {}
        }
    }
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
