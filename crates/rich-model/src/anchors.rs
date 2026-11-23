//! Durable document bookmarks and pageless footnote bodies.
use crate::block::BlockId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnchorId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundaryAffinity {
    Before,
    After,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorPosition {
    pub block_id: BlockId,
    pub offset: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: AnchorId,
    pub name: String,
    pub position: AnchorPosition,
    pub affinity: BoundaryAffinity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footnote {
    pub id: AnchorId,
    pub block_id: BlockId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberedFootnote {
    pub id: AnchorId,
    pub number: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FootnoteRefData {
    pub id: AnchorId,
}

pub const FOOTNOTE_REFERENCE_KIND: &str = "rich.footnote";
pub const FOOTNOTE_BODY_KIND: &str = "rich.footnote.body";

pub(crate) fn is_footnote_body(block: &crate::BlockNode) -> bool {
    matches!(&block.kind, crate::BlockKind::Container { kind, .. } if kind.as_ref() == FOOTNOTE_BODY_KIND)
}

impl crate::Document {
    pub fn content_end_index(&self) -> usize {
        if self.anchors.footnotes().next().is_none() {
            return self.blocks.len();
        }
        self.blocks
            .iter()
            .position(is_footnote_body)
            .unwrap_or(self.blocks.len())
    }
}

pub fn footnote_reference(id: AnchorId) -> crate::inline::InlineObject {
    crate::inline::InlineObject::raw_custom(
        FOOTNOTE_REFERENCE_KIND,
        crate::payload::CustomPayload::typed(FootnoteRefData { id }),
    )
}

pub fn footnote_body_kind(children: Vec<crate::block::BlockNode>) -> crate::block::BlockKind {
    crate::block::BlockKind::raw_container(
        FOOTNOTE_BODY_KIND,
        crate::payload::CustomPayload::empty(),
        children,
    )
}

pub fn footnote_reference_id(object: &crate::inline::InlineObject) -> Option<AnchorId> {
    match object {
        crate::inline::InlineObject::Custom { kind, data }
            if kind.as_ref() == FOOTNOTE_REFERENCE_KIND =>
        {
            data.as_typed::<FootnoteRefData>()
                .map(|reference| reference.id)
        }
        _ => None,
    }
}

pub fn footnote_reference_id_from_payload(
    kind: &str,
    data: &crate::payload::CustomPayload,
) -> Option<AnchorId> {
    if kind != FOOTNOTE_REFERENCE_KIND {
        return None;
    }
    data.as_typed::<FootnoteRefData>()
        .map(|reference| reference.id)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorCommand {
    AddBookmark {
        name: String,
        position: AnchorPosition,
        affinity: BoundaryAffinity,
    },
    RemoveBookmark {
        id: AnchorId,
    },
    AddFootnote {
        body: String,
    },
    InsertFootnoteReference {
        block_id: BlockId,
        offset: usize,
        body: String,
    },
    SetFootnoteBody {
        id: AnchorId,
        body: String,
    },
    RemoveFootnote {
        id: AnchorId,
    },
    Reveal {
        position: AnchorPosition,
    },
    RevealBookmark {
        id: AnchorId,
    },
    RevealFootnote {
        id: AnchorId,
    },
    RevealFootnoteReference {
        id: AnchorId,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorStore {
    bookmarks: BTreeMap<AnchorId, Bookmark>,
    notes: BTreeMap<AnchorId, Footnote>,
    next_id: u64,
}
impl AnchorStore {
    /// Capture before a transaction; restore it from the transaction's inverse for exact undo.
    pub fn checkpoint(&self) -> Self {
        self.clone()
    }
    pub fn restore(&mut self, checkpoint: Self) {
        *self = checkpoint;
    }
    fn allocate(&mut self) -> AnchorId {
        self.next_id = self.next_id.max(1);
        let id = AnchorId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }
    pub fn add_bookmark(
        &mut self,
        name: impl Into<String>,
        position: AnchorPosition,
        affinity: BoundaryAffinity,
    ) -> AnchorId {
        let id = self.allocate();
        self.bookmarks.insert(
            id,
            Bookmark {
                id,
                name: name.into(),
                position,
                affinity,
            },
        );
        id
    }
    pub fn bookmark(&self, id: AnchorId) -> Option<&Bookmark> {
        self.bookmarks.get(&id)
    }
    pub fn set_bookmark_position(&mut self, id: AnchorId, position: AnchorPosition) {
        if let Some(bookmark) = self.bookmarks.get_mut(&id) {
            bookmark.position = position;
        }
    }
    pub fn bookmarks(&self) -> impl Iterator<Item = &Bookmark> {
        self.bookmarks.values()
    }
    pub fn remove_bookmark(&mut self, id: AnchorId) -> Option<Bookmark> {
        self.bookmarks.remove(&id)
    }
    pub fn add_footnote_block(&mut self, block_id: BlockId) -> AnchorId {
        let id = self.allocate();
        self.notes.insert(id, Footnote { id, block_id });
        id
    }
    pub fn footnote(&self, id: AnchorId) -> Option<&Footnote> {
        self.notes.get(&id)
    }
    pub fn footnote_mut(&mut self, id: AnchorId) -> Option<&mut Footnote> {
        self.notes.get_mut(&id)
    }
    pub fn footnotes(&self) -> impl Iterator<Item = &Footnote> {
        self.notes.values()
    }
    pub fn remove_footnote(&mut self, id: AnchorId) -> Option<Footnote> {
        self.notes.remove(&id)
    }
    /// Number referenced notes in current document reference order; duplicates get one number.
    pub fn numbered_notes(
        &self,
        reference_order: impl IntoIterator<Item = AnchorId>,
    ) -> Vec<NumberedFootnote> {
        let mut seen = std::collections::BTreeSet::new();
        reference_order
            .into_iter()
            .filter(|id| self.notes.contains_key(id) && seen.insert(*id))
            .enumerate()
            .map(|(i, id)| NumberedFootnote {
                id,
                number: i as u32 + 1,
            })
            .collect()
    }
    pub fn numbered_document_notes(
        &self,
        document: &crate::block::Document,
    ) -> Vec<NumberedFootnote> {
        self.numbered_notes(
            document
                .collect_leaf_blocks()
                .into_iter()
                .flat_map(|block| {
                    block.kind.inline_content().into_iter().flat_map(|content| {
                        content
                            .objects
                            .iter()
                            .filter_map(|entry| footnote_reference_id(&entry.object))
                    })
                }),
        )
    }
    /// Current display numbers for valid footnote references, in reading order.
    pub fn footnote_number_map(
        &self,
        document: &crate::block::Document,
    ) -> std::collections::HashMap<AnchorId, u32> {
        self.numbered_document_notes(document)
            .into_iter()
            .map(|note| (note.id, note.number))
            .collect()
    }
    pub fn footnote_reference_fallback(
        &self,
        document: &crate::block::Document,
        id: AnchorId,
    ) -> String {
        self.footnote_number_map(document)
            .get(&id)
            .map(|number| format!("[{number}]"))
            .unwrap_or_else(|| "[?]".to_owned())
    }
    pub fn map_insert(&mut self, block_id: BlockId, at: usize, len: usize) {
        for a in self
            .bookmarks
            .values_mut()
            .filter(|a| a.position.block_id == block_id)
        {
            if a.position.offset > at
                || (a.position.offset == at && a.affinity == BoundaryAffinity::After)
            {
                a.position.offset += len;
            }
        }
    }
    pub fn map_delete(&mut self, block_id: BlockId, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let len = end - start;
        for a in self
            .bookmarks
            .values_mut()
            .filter(|a| a.position.block_id == block_id)
        {
            if a.position.offset > end {
                a.position.offset -= len
            } else if a.position.offset >= start {
                a.position.offset = start
            }
        }
    }
    pub fn map_split(&mut self, left: BlockId, at: usize, right: BlockId) {
        for a in self.bookmarks.values_mut().filter(|a| {
            a.position.block_id == left
                && (a.position.offset > at
                    || (a.position.offset == at && a.affinity == BoundaryAffinity::After))
        }) {
            a.position.block_id = right;
            a.position.offset -= at
        }
    }
    pub fn map_merge(&mut self, left: BlockId, left_len: usize, right: BlockId) {
        self.map_merge_suffix(left, left_len, right, 0)
    }
    /// Merge a source suffix into destination at `offset`; source-local positions before
    /// `source_offset` are clamped to the insertion boundary.
    pub fn map_merge_suffix(
        &mut self,
        destination: BlockId,
        offset: usize,
        source: BlockId,
        source_offset: usize,
    ) {
        for a in self
            .bookmarks
            .values_mut()
            .filter(|a| a.position.block_id == source)
        {
            a.position.block_id = destination;
            a.position.offset =
                offset.saturating_add(a.position.offset.saturating_sub(source_offset));
        }
    }
    pub fn map_remove_block(&mut self, removed: BlockId, fallback: AnchorPosition) {
        for a in self
            .bookmarks
            .values_mut()
            .filter(|a| a.position.block_id == removed)
        {
            a.position = fallback
        }
    }
}

impl AnchorCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::AddBookmark { .. } => "anchor.add_bookmark",
            Self::RemoveBookmark { .. } => "anchor.remove_bookmark",
            Self::AddFootnote { .. } => "anchor.add_footnote",
            Self::InsertFootnoteReference { .. } => "anchor.insert_footnote_reference",
            Self::SetFootnoteBody { .. } => "anchor.set_footnote_body",
            Self::RemoveFootnote { .. } => "anchor.remove_footnote",
            Self::Reveal { .. } => "anchor.reveal",
            Self::RevealBookmark { .. } => "anchor.reveal_bookmark",
            Self::RevealFootnote { .. } => "anchor.reveal_footnote",
            Self::RevealFootnoteReference { .. } => "anchor.reveal_footnote_reference",
        }
    }
    pub fn is_document_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Reveal { .. }
                | Self::RevealBookmark { .. }
                | Self::RevealFootnote { .. }
                | Self::RevealFootnoteReference { .. }
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_merge_and_undo_mapping() {
        let mut s = AnchorStore::default();
        let l = BlockId(1);
        let r = BlockId(2);
        let b = s.add_bookmark(
            "x",
            AnchorPosition {
                block_id: l,
                offset: 5,
            },
            BoundaryAffinity::After,
        );
        s.map_split(l, 4, r);
        assert_eq!(
            s.bookmark(b).unwrap().position,
            AnchorPosition {
                block_id: r,
                offset: 1
            }
        );
        s.map_merge(l, 4, r);
        assert_eq!(
            s.bookmark(b).unwrap().position,
            AnchorPosition {
                block_id: l,
                offset: 5
            }
        );
        s.map_split(l, 4, r);
        s.map_merge(l, 4, r);
        assert_eq!(s.bookmark(b).unwrap().position.block_id, l);
    }
    #[test]
    fn footnote_reference_order_and_undo() {
        let mut s = AnchorStore::default();
        let a = s.add_footnote_block(BlockId(10));
        let b = s.add_footnote_block(BlockId(11));
        let mut doc = crate::block::Document::new();
        let mut text = crate::inline::InlineContent::empty();
        text.push_object(footnote_reference(b), crate::style::InlineStyle::default());
        text.push_object(footnote_reference(a), crate::style::InlineStyle::default());
        doc.add_paragraph(text, crate::style::BlockStyle::default());
        assert_eq!(
            s.numbered_document_notes(&doc)
                .iter()
                .map(|n| (n.id, n.number))
                .collect::<Vec<_>>(),
            vec![(b, 1), (a, 2)]
        );
        let before = s.checkpoint();
        s.remove_footnote(a).unwrap();
        assert_eq!(s.numbered_document_notes(&doc)[0].id, b);
        s.restore(before);
        assert_eq!(s.numbered_document_notes(&doc)[1].id, a);
        assert_eq!(footnote_reference_id(&footnote_reference(a)), Some(a));
    }
}
