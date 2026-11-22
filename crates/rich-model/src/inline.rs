use crate::payload::CustomPayload;
use crate::style::{InlineStyle, RgbaColor, SharedStr};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Unicode Object Replacement Character used as a 1-scalar (3 UTF-8 bytes, 1 UTF-16 code unit)
/// sentinel for every inline atomic object (image, smart chip, footnote reference).
pub const OBJECT_REPLACEMENT_CHAR: char = '\u{FFFC}';
pub const OBJECT_REPLACEMENT_UTF8_LEN: usize = 3;

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
        asset_id: String,
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
    pub fn custom(kind: impl Into<SharedStr>, data: CustomPayload) -> Self {
        Self::Custom {
            kind: kind.into(),
            data,
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
    pub fn insert_content(&mut self, offset: usize, slice: &InlineContent) {
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
            self.objects.push(InlineObjectEntry {
                id: obj.id,
                byte_offset: offset + obj.byte_offset,
                object: obj.object.clone(),
            });
            self.next_object_id = self.next_object_id.max(obj.id.0 + 1);
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
