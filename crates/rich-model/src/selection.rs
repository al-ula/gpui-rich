use crate::block::BlockId;
use crate::style::InlineStyle;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PathSegment {
    Block(BlockId),
    TableCell {
        table_id: BlockId,
        row: usize,
        col: usize,
    },
}

/// Logical document coordinate supporting top-level blocks and arbitrarily nested containers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocPos {
    pub path: SmallVec<[PathSegment; 4]>,
    pub offset: usize,
}

impl DocPos {
    pub fn top_level(block_id: BlockId, offset: usize) -> Self {
        Self {
            path: smallvec![PathSegment::Block(block_id)],
            offset,
        }
    }

    pub fn leaf_block_id(&self) -> Option<BlockId> {
        self.path.iter().rev().find_map(|seg| match seg {
            PathSegment::Block(id) => Some(*id),
            PathSegment::TableCell { .. } => None,
        })
    }

    pub fn top_level_block_id(&self) -> Option<BlockId> {
        self.path.first().map(|seg| match seg {
            PathSegment::Block(id) => *id,
            PathSegment::TableCell { table_id, .. } => *table_id,
        })
    }
}

/// Disambiguates visual caret placement at soft line-wrap boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Affinity {
    Upstream,
    #[default]
    Downstream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DocumentSelection {
    Text {
        anchor: DocPos,
        focus: DocPos,
        affinity: Affinity,
        goal_x: Option<f32>,
        stored_marks: Option<InlineStyle>,
    },
    TableCells {
        table_id: BlockId,
        anchor_cell: (usize, usize),
        focus_cell: (usize, usize),
    },
}

impl DocumentSelection {
    pub fn caret(pos: DocPos, affinity: Affinity) -> Self {
        Self::Text {
            anchor: pos.clone(),
            focus: pos,
            affinity,
            goal_x: None,
            stored_marks: None,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        match self {
            Self::Text { anchor, focus, .. } => anchor == focus,
            Self::TableCells { .. } => false,
        }
    }
}

/// Represents whether a formatting attribute has one uniform value across a selection
/// or spans multiple distinct values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionValue<T> {
    /// Every covered span or block shares this exact value.
    Uniform(T),
    /// The selection covers multiple distinct values.
    Mixed,
}

impl<T> SelectionValue<T> {
    /// Returns `Some(value)` when uniform, or `None` when mixed.
    pub fn uniform(self) -> Option<T> {
        match self {
            Self::Uniform(value) => Some(value),
            Self::Mixed => None,
        }
    }

    /// Returns true when the selection covers multiple distinct values.
    pub fn is_mixed(&self) -> bool {
        matches!(self, Self::Mixed)
    }
}
