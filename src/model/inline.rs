use crate::model::block::NodeDataError;
use crate::model::payload::{CustomData, CustomPayload};
use crate::model::style::{InlineStyle, RgbaColor, SharedStr};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// Unicode Object Replacement Character used as a 1-scalar (3 UTF-8 bytes, 1 UTF-16 code unit)
/// sentinel for every inline atomic object (image, smart chip, footnote reference).
pub const OBJECT_REPLACEMENT_CHAR: char = '\u{FFFC}';
pub const OBJECT_REPLACEMENT_UTF8_LEN: usize = 3;

/// Typed identifier for an inline image asset.
///
/// Serializes transparently as its inner string to preserve existing native JSON and HTML/Markdown
/// representations. An `ImageAssetId` identifies an image to an asset provider and is not required
/// to be a filesystem path or URL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageAssetId(pub String);

impl ImageAssetId {
    /// Creates an [`ImageAssetId`] from an arbitrary string identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrows the underlying identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` when the identifier string is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for ImageAssetId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ImageAssetId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for ImageAssetId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageAssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// GPUI-free specification binding a custom inline object kind identifier to its typed payload.
///
/// Downstream crates implement this trait once per custom inline node kind and pass the
/// specification type to [`InlineObject::custom`], [`InlineObject::read_data`],
/// [`crate::model::Step::update_inline_data`], and GPUI renderers.
pub trait InlineSpec: 'static {
    /// Stable persisted kind identifier stored in [`InlineObject::Custom::kind`].
    const KIND: &'static str;
    /// Typed payload persisted as JSON and borrowed directly when stored in memory.
    type Data: CustomData + Clone + DeserializeOwned + 'static;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InlineObjectId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InlineVAlign {
    #[default]
    Baseline,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChipKind {
    Person,
    Date,
    Document,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InlineObject {
    Image {
        asset_id: ImageAssetId,
        alt: String,
        width_pt: f32,
        height_pt: f32,
        valign: InlineVAlign,
    },
    SmartChip {
        kind: ChipKind,
        label: String,
        accent: Option<RgbaColor>,
    },
    FootnoteRef {
        index: u32,
    },
    /// Host-defined custom inline atomic node rendered and measured via `ExtensionRegistry`.
    Custom {
        kind: SharedStr,
        #[serde(default)]
        data: CustomPayload,
    },
}

impl InlineObject {
    /// Constructs a custom inline object bound to [`InlineSpec`] `S`.
    pub fn custom<S: InlineSpec>(data: S::Data) -> Self {
        Self::Custom {
            kind: Arc::from(S::KIND),
            data: CustomPayload::typed(data),
        }
    }

    /// Interchange constructor used by deserializers and built-in internal objects.
    pub(crate) fn raw_custom(kind: impl Into<SharedStr>, data: CustomPayload) -> Self {
        Self::Custom {
            kind: kind.into(),
            data,
        }
    }

    /// Performs a checked read of this inline object's payload against [`InlineSpec`] `S`.
    ///
    /// Distinguishes a wrong object kind from a corrupt or incompatible payload.
    pub fn read_data<S: InlineSpec>(&self) -> Result<Cow<'_, S::Data>, NodeDataError> {
        match self {
            Self::Custom { kind, data } => {
                if kind.as_ref() != S::KIND {
                    return Err(NodeDataError::KindMismatch {
                        expected: S::KIND,
                        actual: kind.to_string(),
                    });
                }
                data.try_as_typed::<S::Data>()
                    .map_err(|error| NodeDataError::InvalidData {
                        kind: S::KIND,
                        message: error.to_string(),
                    })
            }
            Self::Image { .. } => Err(NodeDataError::KindMismatch {
                expected: S::KIND,
                actual: "image".to_owned(),
            }),
            Self::SmartChip { .. } => Err(NodeDataError::KindMismatch {
                expected: S::KIND,
                actual: "smart_chip".to_owned(),
            }),
            Self::FootnoteRef { .. } => Err(NodeDataError::KindMismatch {
                expected: S::KIND,
                actual: "footnote_ref".to_owned(),
            }),
        }
    }

    /// Returns `true` when `self` and `other` have the same built-in variant or custom kind string.
    pub fn same_kind(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Image { .. }, Self::Image { .. })
            | (Self::SmartChip { .. }, Self::SmartChip { .. })
            | (Self::FootnoteRef { .. }, Self::FootnoteRef { .. }) => true,
            (Self::Custom { kind: left, .. }, Self::Custom { kind: right, .. }) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InlineObjectEntry {
    pub id: InlineObjectId,
    /// UTF-8 byte offset of the corresponding `\u{FFFC}` in `InlineContent::text`.
    pub byte_offset: usize,
    pub object: InlineObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleSpan {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

/// Inline rich text stream for a single text-bearing block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InlineContent {
    pub text: String,
    pub spans: Vec<StyleSpan>,
    pub objects: Vec<InlineObjectEntry>,
    pub utf16_len: usize,
    pub is_ascii: bool,
    next_object_id: u64,
}

impl Default for InlineContent {
    fn default() -> Self {
        Self::empty()
    }
}

impl InlineContent {
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            spans: Vec::new(),
            objects: Vec::new(),
            utf16_len: 0,
            is_ascii: true,
            next_object_id: 1,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        let mut content = Self::empty();
        content.push_text(&text.into(), InlineStyle::default());
        content
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn push_text(&mut self, text: &str, style: InlineStyle) -> &mut Self {
        if text.is_empty() {
            return self;
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        self.utf16_len += text.encode_utf16().count();
        if !text.is_ascii() {
            self.is_ascii = false;
        }
        if let Some(last) = self.spans.last_mut()
            && last.range.end == start
            && last.style == style
        {
            last.range.end = end;
        } else {
            self.spans.push(StyleSpan {
                range: start..end,
                style,
            });
        }
        self
    }

    pub fn push_object(&mut self, object: InlineObject, style: InlineStyle) -> InlineObjectId {
        let id = InlineObjectId(self.next_object_id);
        self.next_object_id += 1;
        let byte_offset = self.text.len();
        let mut buf = [0u8; 4];
        let sentinel = OBJECT_REPLACEMENT_CHAR.encode_utf8(&mut buf);
        self.push_text(sentinel, style);
        self.objects.push(InlineObjectEntry {
            id,
            byte_offset,
            object,
        });
        id
    }

    pub fn object_at(&self, byte_offset: usize) -> Option<&InlineObjectEntry> {
        self.objects
            .binary_search_by_key(&byte_offset, |e| e.byte_offset)
            .ok()
            .map(|idx| &self.objects[idx])
    }

    /// Looks up an inline object entry by its [`InlineObjectId`].
    pub fn object_by_id(&self, id: InlineObjectId) -> Option<&InlineObjectEntry> {
        self.objects.iter().find(|entry| entry.id == id)
    }

    /// Replaces the payload of the inline object matching `id`, returning the previous payload.
    pub fn update_object(
        &mut self,
        id: InlineObjectId,
        object: InlineObject,
    ) -> Option<InlineObject> {
        let entry = self.objects.iter_mut().find(|entry| entry.id == id)?;
        Some(std::mem::replace(&mut entry.object, object))
    }

    /// Replaces the payload of the inline object matching `id` only when its kind matches `object`.
    pub fn update_object_checked(
        &mut self,
        id: InlineObjectId,
        object: InlineObject,
    ) -> Option<InlineObject> {
        let entry = self.objects.iter_mut().find(|entry| entry.id == id)?;
        if !entry.object.same_kind(&object) {
            return None;
        }
        Some(std::mem::replace(&mut entry.object, object))
    }

    /// Returns the inline style active at `byte_offset` (or the preceding style at `len()`).
    pub fn style_at(&self, byte_offset: usize) -> InlineStyle {
        if self.spans.is_empty() {
            return InlineStyle::default();
        }
        let query = byte_offset.min(self.text.len().saturating_sub(1));
        for span in &self.spans {
            if span.range.contains(&query) {
                return span.style.clone();
            }
        }
        self.spans
            .last()
            .map(|span| span.style.clone())
            .unwrap_or_default()
    }

    /// Returns the inherited inline style at a collapsed caret offset (matching `insert_text`).
    pub fn style_at_caret(&self, byte_offset: usize) -> InlineStyle {
        let clamped = byte_offset.min(self.text.len());
        if clamped > 0 {
            self.style_at(clamped - 1)
        } else {
            self.style_at(0)
        }
    }

    /// Builds a contiguous list of [`StyleSpan`]s covering `range` after applying `transform`
    /// to each intersecting span's style (suitable for `Step::RestoreSpans`).
    pub fn mapped_spans_in_range(
        &self,
        range: Range<usize>,
        mut transform: impl FnMut(&mut InlineStyle),
    ) -> Vec<StyleSpan> {
        let start = self.clamp_to_char_boundary(range.start);
        let end = self.clamp_to_char_boundary(range.end).max(start);
        if start >= end {
            return Vec::new();
        }
        let mut mapped: Vec<StyleSpan> = Vec::new();
        for span in &self.spans {
            if span.range.end <= start || span.range.start >= end {
                continue;
            }
            let overlap_start = span.range.start.max(start);
            let overlap_end = span.range.end.min(end);
            if overlap_start < overlap_end {
                let mut next_style = span.style.clone();
                transform(&mut next_style);
                if let Some(last) = mapped.last_mut()
                    && last.range.end == overlap_start
                    && last.style == next_style
                {
                    last.range.end = overlap_end;
                } else {
                    mapped.push(StyleSpan {
                        range: overlap_start..overlap_end,
                        style: next_style,
                    });
                }
            }
        }
        if mapped.is_empty() {
            let mut next_style = InlineStyle::default();
            transform(&mut next_style);
            mapped.push(StyleSpan {
                range: start..end,
                style: next_style,
            });
        }
        mapped
    }

    fn clamp_to_char_boundary(&self, byte_offset: usize) -> usize {
        let mut offset = byte_offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// Converts a UTF-8 byte offset within this block to a UTF-16 code unit offset.
    pub fn utf8_to_utf16(&self, byte_offset: usize) -> usize {
        let clamped = byte_offset.min(self.text.len());
        if self.is_ascii {
            return clamped;
        }
        debug_assert!(self.text.is_char_boundary(clamped));
        self.text[..clamped].encode_utf16().count()
    }

    /// Converts a UTF-16 code unit offset within this block to a valid UTF-8 byte offset.
    pub fn utf16_to_utf8(&self, utf16_offset: usize) -> usize {
        if self.is_ascii {
            return utf16_offset.min(self.text.len());
        }
        let mut units = 0usize;
        for (byte_idx, ch) in self.text.char_indices() {
            if units >= utf16_offset {
                return byte_idx;
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }

    /// Inserts UTF-8 text at `offset` with `style` (or inheriting local style if `None`).
    pub fn insert_text(&mut self, offset: usize, text: &str, style: Option<InlineStyle>) {
        if text.is_empty() {
            return;
        }
        assert!(self.text.is_char_boundary(offset));
        let applied_style = style.unwrap_or_else(|| {
            if offset > 0 {
                self.style_at(offset - 1)
            } else {
                self.style_at(0)
            }
        });

        let delta = text.len();
        self.text.insert_str(offset, text);

        for obj in &mut self.objects {
            if obj.byte_offset >= offset {
                obj.byte_offset += delta;
            }
        }

        let mut new_spans = Vec::with_capacity(self.spans.len() + 2);
        let mut inserted = false;
        for span in self.spans.drain(..) {
            if span.range.end <= offset {
                new_spans.push(span);
            } else if span.range.start >= offset {
                if !inserted {
                    new_spans.push(StyleSpan {
                        range: offset..(offset + delta),
                        style: applied_style.clone(),
                    });
                    inserted = true;
                }
                new_spans.push(StyleSpan {
                    range: (span.range.start + delta)..(span.range.end + delta),
                    style: span.style,
                });
            } else {
                // Split span around `offset`.
                new_spans.push(StyleSpan {
                    range: span.range.start..offset,
                    style: span.style.clone(),
                });
                new_spans.push(StyleSpan {
                    range: offset..(offset + delta),
                    style: applied_style.clone(),
                });
                new_spans.push(StyleSpan {
                    range: (offset + delta)..(span.range.end + delta),
                    style: span.style,
                });
                inserted = true;
            }
        }
        if !inserted {
            new_spans.push(StyleSpan {
                range: offset..(offset + delta),
                style: applied_style,
            });
        }
        self.spans = new_spans;
        self.normalize();
    }

    /// Deletes `range` and returns the removed `InlineContent` slice (for Undo/Redo).
    pub fn delete_range(&mut self, range: Range<usize>) -> InlineContent {
        let removed = self.slice(range.clone());
        if range.is_empty() {
            return removed;
        }
        assert!(self.text.is_char_boundary(range.start));
        assert!(self.text.is_char_boundary(range.end));

        let delta = range.end - range.start;
        self.text.replace_range(range.clone(), "");

        self.objects
            .retain(|obj| obj.byte_offset < range.start || obj.byte_offset >= range.end);
        for obj in &mut self.objects {
            if obj.byte_offset >= range.end {
                obj.byte_offset -= delta;
            }
        }

        let mut new_spans = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..) {
            if span.range.end <= range.start {
                new_spans.push(span);
            } else if span.range.start >= range.end {
                new_spans.push(StyleSpan {
                    range: (span.range.start - delta)..(span.range.end - delta),
                    style: span.style,
                });
            } else {
                if span.range.start < range.start {
                    new_spans.push(StyleSpan {
                        range: span.range.start..range.start,
                        style: span.style.clone(),
                    });
                }
                if span.range.end > range.end {
                    new_spans.push(StyleSpan {
                        range: range.start..(span.range.end - delta),
                        style: span.style,
                    });
                }
            }
        }
        self.spans = new_spans;
        self.normalize();
        removed
    }

    /// Inserts a previously captured `InlineContent` slice at `offset`.
    /// Inserts captured content as a copy, assigning destination-local object IDs.
    pub fn insert_content(&mut self, offset: usize, slice: &InlineContent) {
        self.insert_content_with_identity(offset, slice, false);
    }

    /// Restores removed content with its original object IDs (for undo).
    pub fn insert_content_preserving_object_ids(&mut self, offset: usize, slice: &InlineContent) {
        self.insert_content_with_identity(offset, slice, true);
    }

    fn insert_content_with_identity(
        &mut self,
        offset: usize,
        slice: &InlineContent,
        preserve_object_ids: bool,
    ) {
        if slice.is_empty() {
            return;
        }
        assert!(self.text.is_char_boundary(offset));
        let delta = slice.text.len();
        self.text.insert_str(offset, &slice.text);

        for obj in &mut self.objects {
            if obj.byte_offset >= offset {
                obj.byte_offset += delta;
            }
        }
        for obj in &slice.objects {
            let id = if preserve_object_ids {
                self.next_object_id = self.next_object_id.max(obj.id.0.saturating_add(1));
                obj.id
            } else {
                let id = InlineObjectId(self.next_object_id);
                self.next_object_id = self.next_object_id.saturating_add(1);
                id
            };
            self.objects.push(InlineObjectEntry {
                id,
                byte_offset: offset + obj.byte_offset,
                object: obj.object.clone(),
            });
        }
        self.objects.sort_by_key(|e| e.byte_offset);

        let mut new_spans = Vec::with_capacity(self.spans.len() + slice.spans.len() + 1);
        let mut inserted = false;
        let push_slice = |out: &mut Vec<StyleSpan>| {
            for s in &slice.spans {
                out.push(StyleSpan {
                    range: (offset + s.range.start)..(offset + s.range.end),
                    style: s.style.clone(),
                });
            }
        };

        for span in self.spans.drain(..) {
            if span.range.end <= offset {
                new_spans.push(span);
            } else if span.range.start >= offset {
                if !inserted {
                    push_slice(&mut new_spans);
                    inserted = true;
                }
                new_spans.push(StyleSpan {
                    range: (span.range.start + delta)..(span.range.end + delta),
                    style: span.style,
                });
            } else {
                new_spans.push(StyleSpan {
                    range: span.range.start..offset,
                    style: span.style.clone(),
                });
                push_slice(&mut new_spans);
                new_spans.push(StyleSpan {
                    range: (offset + delta)..(span.range.end + delta),
                    style: span.style,
                });
                inserted = true;
            }
        }
        if !inserted {
            push_slice(&mut new_spans);
        }
        self.spans = new_spans;
        self.normalize();
    }

    /// Merges `overlay` style onto `range`, returning the previous spans inside `range` for Undo.
    pub fn apply_style(&mut self, range: Range<usize>, overlay: &InlineStyle) -> Vec<StyleSpan> {
        if range.is_empty() {
            return Vec::new();
        }
        assert!(self.text.is_char_boundary(range.start));
        assert!(self.text.is_char_boundary(range.end));

        let mut previous = Vec::new();
        let mut new_spans = Vec::with_capacity(self.spans.len() + 2);

        for span in self.spans.drain(..) {
            if span.range.end <= range.start || span.range.start >= range.end {
                new_spans.push(span);
                continue;
            }
            if span.range.start < range.start {
                new_spans.push(StyleSpan {
                    range: span.range.start..range.start,
                    style: span.style.clone(),
                });
            }
            let mid_start = span.range.start.max(range.start);
            let mid_end = span.range.end.min(range.end);
            previous.push(StyleSpan {
                range: mid_start..mid_end,
                style: span.style.clone(),
            });
            new_spans.push(StyleSpan {
                range: mid_start..mid_end,
                style: span.style.merge(overlay),
            });
            if span.range.end > range.end {
                new_spans.push(StyleSpan {
                    range: range.end..span.range.end,
                    style: span.style,
                });
            }
        }

        self.spans = new_spans;
        self.normalize();
        previous
    }

    /// Replaces the exact styles covering `spans` (used when undoing `apply_style`).
    pub fn restore_spans(&mut self, restored: &[StyleSpan]) {
        let (Some(first), Some(last)) = (restored.first(), restored.last()) else {
            return;
        };
        let range = first.range.start..last.range.end;
        let mut new_spans = Vec::with_capacity(self.spans.len() + restored.len());
        let mut inserted = false;

        for span in self.spans.drain(..) {
            if span.range.end <= range.start {
                new_spans.push(span);
            } else if span.range.start >= range.end {
                if !inserted {
                    new_spans.extend(restored.iter().cloned());
                    inserted = true;
                }
                new_spans.push(span);
            } else {
                if span.range.start < range.start {
                    new_spans.push(StyleSpan {
                        range: span.range.start..range.start,
                        style: span.style.clone(),
                    });
                }
                if !inserted {
                    new_spans.extend(restored.iter().cloned());
                    inserted = true;
                }
                if span.range.end > range.end {
                    new_spans.push(StyleSpan {
                        range: range.end..span.range.end,
                        style: span.style,
                    });
                }
            }
        }
        if !inserted {
            new_spans.extend(restored.iter().cloned());
        }
        self.spans = new_spans;
        self.normalize();
    }

    pub fn slice(&self, range: Range<usize>) -> InlineContent {
        assert!(self.text.is_char_boundary(range.start));
        assert!(self.text.is_char_boundary(range.end));
        let text = self.text[range.clone()].to_string();
        let mut spans = Vec::new();
        for span in &self.spans {
            if span.range.end <= range.start || span.range.start >= range.end {
                continue;
            }
            let s = span.range.start.max(range.start) - range.start;
            let e = span.range.end.min(range.end) - range.start;
            if s < e {
                spans.push(StyleSpan {
                    range: s..e,
                    style: span.style.clone(),
                });
            }
        }
        let mut objects = Vec::new();
        let mut max_id = 0u64;
        for obj in &self.objects {
            if obj.byte_offset >= range.start && obj.byte_offset < range.end {
                max_id = max_id.max(obj.id.0);
                objects.push(InlineObjectEntry {
                    id: obj.id,
                    byte_offset: obj.byte_offset - range.start,
                    object: obj.object.clone(),
                });
            }
        }
        let mut out = InlineContent {
            text,
            spans,
            objects,
            utf16_len: 0,
            is_ascii: true,
            next_object_id: max_id + 1,
        };
        out.normalize();
        out
    }

    pub fn normalize(&mut self) {
        self.is_ascii = self.text.is_ascii();
        self.utf16_len = if self.is_ascii {
            self.text.len()
        } else {
            self.text.encode_utf16().count()
        };

        let mut merged: Vec<StyleSpan> = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..) {
            if span.range.is_empty() {
                continue;
            }
            if let Some(last) = merged.last_mut()
                && last.range.end == span.range.start
                && last.style == span.style
            {
                last.range.end = span.range.end;
            } else {
                merged.push(span);
            }
        }
        self.spans = merged;
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_objects_get_fresh_ids_and_restore_can_preserve_identity() {
        struct TestInlineSpec;
        impl InlineSpec for TestInlineSpec {
            const KIND: &'static str = "test";
            type Data = ();
        }

        let mut source = InlineContent::empty();
        let source_id = source.push_object(
            InlineObject::custom::<TestInlineSpec>(()),
            InlineStyle::default(),
        );
        let copied = source.slice(0..source.len());

        let mut destination = InlineContent::empty();
        let existing_id = destination.push_object(
            InlineObject::custom::<TestInlineSpec>(()),
            InlineStyle::default(),
        );
        destination.insert_content(destination.len(), &copied);
        let copied_id = destination.objects[1].id;
        assert_ne!(copied_id, existing_id);
        assert_eq!(destination.object_by_id(existing_id).unwrap().byte_offset, 0);
        assert_eq!(destination.object_by_id(copied_id).unwrap().byte_offset, 3);

        let mut restored = InlineContent::empty();
        restored.insert_content_preserving_object_ids(0, &copied);
        assert_eq!(restored.objects[0].id, source_id);
    }
}
