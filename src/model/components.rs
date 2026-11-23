//! Document-owned reusable component definitions and editing commands.
use crate::model::{
    BlockId, BlockKind, BlockNode, BlockStyle, ColumnSpec, CustomPayload, Document, InlineContent,
    InlineObject, InlineObjectId, TableBlock, TableBorderStyle, TableCell, TableRow,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DROPDOWN_KIND: &str = "component.dropdown";
pub const VARIABLE_KIND: &str = "component.variable";
pub const PLACEHOLDER_KIND: &str = "component.placeholder";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropdownOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropdownDefinition {
    pub id: String,
    pub label: String,
    pub options: Vec<DropdownOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableDefinition {
    pub id: String,
    pub name: String,
    pub value: String,
}

/// A captured fragment is stored as normal editable nodes; insertion always clones with fresh IDs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildingBlock {
    pub id: String,
    pub name: String,
    pub blocks: Vec<BlockNode>,
}

impl BuildingBlock {
    /// Builds an editable task table using a status definition already owned by the destination.
    /// Insert the returned nodes through the building-block commands to assign destination IDs.
    pub fn task_tracker(
        id: impl Into<String>,
        name: impl Into<String>,
        status: &DropdownDefinition,
        initial_status: &str,
    ) -> Option<Self> {
        let option = status
            .options
            .iter()
            .find(|option| option.id == initial_status)?;
        let mut document = Document::new();
        let table_id = document.alloc_block_id();
        let mut cell = |content| {
            let block = BlockNode::new(
                document.alloc_block_id(),
                BlockKind::paragraph(content),
                BlockStyle::default(),
            );
            TableCell::simple(document.alloc_block_id(), vec![block])
        };
        let mut status_data = ComponentCommand::dropdown_payload(&status.id, initial_status);
        status_data["fallback"] = option.label.clone().into();
        let mut status_value = InlineContent::empty();
        status_value.push_object(
            InlineObject::raw_custom(DROPDOWN_KIND, CustomPayload::from_json(status_data)),
            Default::default(),
        );
        let mut assignee = InlineContent::empty();
        assignee.push_object(
            PlaceholderKind::Person.object("Assignee"),
            Default::default(),
        );
        let mut due_date = InlineContent::empty();
        due_date.push_object(PlaceholderKind::Date.object("Due date"), Default::default());
        let heading = TableRow {
            is_header: true,
            cells: ["Task", "Status", "Assignee", "Due date"]
                .into_iter()
                .map(|title| cell(InlineContent::plain(title)))
                .collect(),
        };
        let task = TableRow {
            is_header: false,
            cells: [InlineContent::empty(), status_value, assignee, due_date]
                .into_iter()
                .map(cell)
                .collect(),
        };
        let table = TableBlock {
            columns: vec![ColumnSpec::Auto; 4],
            column_kinds: BTreeMap::from([
                (2, PlaceholderKind::Person),
                (3, PlaceholderKind::Date),
            ]),
            rows: vec![heading, task],
            border_style: TableBorderStyle::default(),
        };
        Some(Self {
            id: id.into(),
            name: name.into(),
            blocks: vec![BlockNode::new(
                table_id,
                BlockKind::table(table),
                BlockStyle::default(),
            )],
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentStore {
    #[serde(default)]
    pub dropdowns: BTreeMap<String, DropdownDefinition>,
    #[serde(default)]
    pub variables: BTreeMap<String, VariableDefinition>,
    #[serde(default)]
    pub building_blocks: BTreeMap<String, BuildingBlock>,
}

impl ComponentStore {
    pub fn dropdown_label(&self, definition_id: &str, option_id: &str) -> Option<&str> {
        self.dropdowns
            .get(definition_id)?
            .options
            .iter()
            .find(|option| option.id == option_id)
            .map(|option| option.label.as_str())
    }

    pub fn variable_value(&self, variable_id: &str) -> Option<&str> {
        self.variables
            .get(variable_id)
            .map(|definition| definition.value.as_str())
    }

    /// Returns a readable fallback even for unresolved imported component metadata.
    pub fn inline_fallback(&self, kind: &str, data: &serde_json::Value) -> String {
        let string = |key: &str| {
            data.get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        };
        match kind {
            DROPDOWN_KIND => self
                .dropdown_label(string("definition_id"), string("selected_option_id"))
                .unwrap_or_else(|| string("fallback"))
                .to_owned(),
            VARIABLE_KIND => self
                .variable_value(string("variable_id"))
                .unwrap_or_else(|| string("fallback"))
                .to_owned(),
            PLACEHOLDER_KIND => if string("value").is_empty() {
                string("label")
            } else {
                string("value")
            }
            .to_owned(),
            _ => string("fallback").to_owned(),
        }
    }
}
impl ComponentStore {
    pub fn export_dropdown_metadata(&self, definition_id: &str) -> Option<serde_json::Value> {
        serde_json::to_value(self.dropdowns.get(definition_id)?).ok()
    }

    /// Imports portable metadata. Identical same-ID definitions are shared; collisions get a
    /// fresh document-local ID and preserve the source definition's option identities.
    pub fn import_dropdown_metadata(&mut self, metadata: &serde_json::Value) -> Option<String> {
        let mut definition: DropdownDefinition = serde_json::from_value(metadata.clone()).ok()?;
        if let Some(existing) = self.dropdowns.get(&definition.id) {
            if existing == &definition {
                return Some(definition.id);
            }
            let base = definition.id.clone();
            let mut suffix = 2usize;
            while self.dropdowns.contains_key(&format!("{base}-{suffix}")) {
                suffix += 1;
            }
            definition.id = format!("{base}-{suffix}");
        }
        let id = definition.id.clone();
        self.dropdowns.insert(id.clone(), definition);
        Some(id)
    }

    pub fn export_variable_metadata(&self, variable_id: &str) -> Option<serde_json::Value> {
        serde_json::to_value(self.variables.get(variable_id)?).ok()
    }

    pub fn import_variable_metadata(&mut self, metadata: &serde_json::Value) -> Option<String> {
        let mut definition: VariableDefinition = serde_json::from_value(metadata.clone()).ok()?;
        if let Some(existing) = self.variables.get(&definition.id) {
            if existing == &definition {
                return Some(definition.id);
            }
            let base = definition.id.clone();
            let mut suffix = 2usize;
            while self.variables.contains_key(&format!("{base}-{suffix}")) {
                suffix += 1;
            }
            definition.id = format!("{base}-{suffix}");
        }
        let id = definition.id.clone();
        self.variables.insert(id.clone(), definition);
        Some(id)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ComponentCommand {
    DefineDropdown(DropdownDefinition),
    RenameDropdownOption {
        definition_id: String,
        option_id: String,
        label: String,
    },
    ReorderDropdownOptions {
        definition_id: String,
        option_ids: Vec<String>,
    },
    RemoveDropdownOption {
        definition_id: String,
        option_id: String,
        replacement_option_id: Option<String>,
    },
    InsertDropdown {
        block_id: BlockId,
        offset: usize,
        definition_id: String,
        selected_option_id: String,
    },
    SelectDropdownOption {
        block_id: BlockId,
        object_id: InlineObjectId,
        option_id: String,
    },
    DefineVariable(VariableDefinition),
    SetVariableValue {
        variable_id: String,
        value: String,
    },
    RemoveVariable {
        variable_id: String,
    },
    InsertVariable {
        block_id: BlockId,
        offset: usize,
        variable_id: String,
    },
    InsertPlaceholder {
        block_id: BlockId,
        offset: usize,
        kind: PlaceholderKind,
        label: String,
    },
    FillPlaceholder {
        block_id: BlockId,
        object_id: InlineObjectId,
        value: String,
    },
    CaptureBuildingBlock {
        id: String,
        name: String,
        blocks: Vec<BlockNode>,
    },
    CaptureSelectionAsBuildingBlock {
        id: String,
        name: String,
    },
    InsertBuildingBlock {
        id: String,
        target: crate::model::ContainerTarget,
        index: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaceholderKind {
    Date,
    Person,
    File,
}

impl PlaceholderKind {
    pub fn object(self, label: &str) -> InlineObject {
        InlineObject::raw_custom(
            PLACEHOLDER_KIND,
            CustomPayload::from_json(serde_json::json!({"kind": self.as_str(), "label": label})),
        )
    }

    pub fn filled_object(payload: serde_json::Value) -> InlineObject {
        InlineObject::raw_custom(PLACEHOLDER_KIND, CustomPayload::from_json(payload))
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Person => "person",
            Self::File => "file",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "date" => Some(Self::Date),
            "person" => Some(Self::Person),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

impl ComponentCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::DefineDropdown(_) => "rich.component.define_dropdown",
            Self::RenameDropdownOption { .. } => "rich.component.rename_dropdown_option",
            Self::ReorderDropdownOptions { .. } => "rich.component.reorder_dropdown_options",
            Self::RemoveDropdownOption { .. } => "rich.component.remove_dropdown_option",
            Self::InsertDropdown { .. } => "rich.component.insert_dropdown",
            Self::SelectDropdownOption { .. } => "rich.component.select_dropdown_option",
            Self::DefineVariable(_) => "rich.component.define_variable",
            Self::SetVariableValue { .. } => "rich.component.set_variable_value",
            Self::RemoveVariable { .. } => "rich.component.remove_variable",
            Self::FillPlaceholder { .. } => "rich.component.fill_placeholder",
            Self::InsertVariable { .. } => "rich.component.insert_variable",
            Self::InsertPlaceholder { .. } => "rich.component.insert_placeholder",
            Self::CaptureBuildingBlock { .. } => "rich.component.capture_building_block",
            Self::CaptureSelectionAsBuildingBlock { .. } => "rich.component.capture_selection",
            Self::InsertBuildingBlock { .. } => "rich.component.insert_building_block",
        }
    }

    pub fn is_document_mutation(&self) -> bool {
        true
    }

    /// Data shape stored in an existing Custom inline object. The displayed text is resolved
    /// from the document's ComponentStore at measurement/paint/export time, never duplicated here.
    pub fn dropdown_payload(
        definition_id: impl Into<String>,
        selected_option_id: impl Into<String>,
    ) -> serde_json::Value {
        serde_json::json!({"definition_id": definition_id.into(), "selected_option_id": selected_option_id.into()})
    }

    /// Constructs a built-in dropdown component inline object.
    pub fn dropdown_object(payload: serde_json::Value) -> InlineObject {
        InlineObject::raw_custom(DROPDOWN_KIND, CustomPayload::from_json(payload))
    }

    /// Constructs a built-in variable component inline object.
    pub fn variable_object(payload: serde_json::Value) -> InlineObject {
        InlineObject::raw_custom(VARIABLE_KIND, CustomPayload::from_json(payload))
    }

    pub fn variable_payload(
        variable_id: impl Into<String>,
        fallback: impl Into<String>,
    ) -> serde_json::Value {
        serde_json::json!({"variable_id": variable_id.into(), "fallback": fallback.into()})
    }
}
