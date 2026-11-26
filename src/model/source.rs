//! Source-backed editing, independent of the native rich document format.

use crate::model::{BlockStyle, RichCommand};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    /// UTF-8 byte range in the source before the transaction.
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceBias {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceChange {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// Applied changes in old/new source coordinates, in increasing order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceChangeSet {
    pub before_revision: u64,
    pub revision: u64,
    pub changes: Vec<SourceChange>,
}

impl SourceChangeSet {
    /// Returns `None` for a position strictly inside removed text. At the start of a change,
    /// `bias` selects the boundary before or after the replacement.
    pub fn map_offset(&self, offset: usize, bias: SourceBias) -> Option<usize> {
        let mut old_end = 0;
        let mut new_end = 0;
        for change in &self.changes {
            if offset < change.old.start {
                break;
            }
            if offset == change.old.start {
                return Some(match bias {
                    SourceBias::Before => change.new.start,
                    SourceBias::After => change.new.end,
                });
            }
            if offset < change.old.end {
                return None;
            }
            old_end = change.old.end;
            new_end = change.new.end;
        }
        new_end.checked_add(offset - old_end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEditError {
    NotSourceBacked,
    StaleRevision { expected: u64, actual: u64 },
    InvalidRange(Range<usize>),
    OverlappingEdits,
}

impl std::fmt::Display for SourceEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSourceBacked => formatter.write_str("view is not source-backed"),
            Self::StaleRevision { expected, actual } => {
                write!(
                    formatter,
                    "expected source revision {expected}, found {actual}"
                )
            }
            Self::InvalidRange(range) => write!(formatter, "invalid UTF-8 source range {range:?}"),
            Self::OverlappingEdits => formatter.write_str("source edits overlap"),
        }
    }
}

impl std::error::Error for SourceEditError {}

pub struct AppliedSourceEdit {
    pub changes: SourceChangeSet,
    pub inverse: Vec<SourceEdit>,
}

/// Exact UTF-8 file contents. Line separators are never normalized in storage.
#[derive(Clone, Debug)]
pub struct SourceBuffer {
    text: String,
    lines: Vec<Range<usize>>,
    revision: u64,
}

impl SourceBuffer {
    pub fn new(text: String) -> Self {
        let mut lines = Vec::new();
        index_lines(&text, &mut lines);
        Self {
            text,
            lines,
            revision: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Line contents, excluding the exact separators retained between ranges in `text()`.
    /// Includes a trailing empty line when the file ends with a separator.
    pub fn lines(&self) -> &[Range<usize>] {
        &self.lines
    }

    /// Line and local byte offset. A position inside CRLF resolves to the following line.
    pub fn line_position(&self, offset: usize) -> Option<(usize, usize)> {
        if !self.text.is_char_boundary(offset) {
            return None;
        }
        let index = self.lines.partition_point(|line| line.end < offset);
        let line = self.lines.get(index)?;
        Some((index, offset.saturating_sub(line.start)))
    }

    /// The local separator to use for Enter. Preserve adjacent line endings in mixed files.
    pub fn line_break_at(&self, line: usize) -> &str {
        let separator = |index: usize| {
            let end = self.lines.get(index)?.end;
            let next = self.lines.get(index + 1)?.start;
            self.text.get(end..next)
        };
        separator(line)
            .or_else(|| line.checked_sub(1).and_then(separator))
            .unwrap_or("\n")
    }

    /// Applies an atomic batch in pre-edit coordinates and returns its inverse in new coordinates.
    /// Adjacent edits are treated as one replacement; no-op batches do not advance the revision.
    pub fn apply(
        &mut self,
        expected_revision: u64,
        mut edits: Vec<SourceEdit>,
    ) -> Result<Option<AppliedSourceEdit>, SourceEditError> {
        if expected_revision != self.revision {
            return Err(SourceEditError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut merged: Vec<SourceEdit> = Vec::with_capacity(edits.len());
        for edit in edits {
            if self.text.get(edit.range.clone()).is_none() {
                return Err(SourceEditError::InvalidRange(edit.range));
            }
            if let Some(previous) = merged.last_mut() {
                if previous.range.end > edit.range.start {
                    return Err(SourceEditError::OverlappingEdits);
                }
                if previous.range.end == edit.range.start {
                    previous.range.end = edit.range.end;
                    previous.text.push_str(&edit.text);
                    continue;
                }
            }
            merged.push(edit);
        }
        merged.retain(|edit| self.text[edit.range.clone()] != edit.text);
        if merged.is_empty() {
            return Ok(None);
        }
        let mut inverse = Vec::with_capacity(merged.len());
        let mut changes = Vec::with_capacity(merged.len());
        let mut old_end = 0;
        let mut new_end = 0;
        for edit in &merged {
            let start = new_end + (edit.range.start - old_end);
            old_end = edit.range.end;
            new_end = start + edit.text.len();
            let new = start..new_end;
            inverse.push(SourceEdit {
                range: new.clone(),
                text: self.text[edit.range.clone()].to_owned(),
            });
            changes.push(SourceChange {
                old: edit.range.clone(),
                new,
            });
        }
        let [only] = merged.as_slice() else {
            let mut output = String::with_capacity(new_end + (self.text.len() - old_end));
            let mut cursor = 0;
            for edit in &merged {
                output.push_str(&self.text[cursor..edit.range.start]);
                output.push_str(&edit.text);
                cursor = edit.range.end;
            }
            output.push_str(&self.text[cursor..]);
            self.text = output;
            return Ok(Some(self.finish(changes, inverse)));
        };
        self.text.replace_range(only.range.clone(), &only.text);
        Ok(Some(self.finish(changes, inverse)))
    }

    fn finish(
        &mut self,
        changes: Vec<SourceChange>,
        inverse: Vec<SourceEdit>,
    ) -> AppliedSourceEdit {
        self.lines.clear();
        index_lines(&self.text, &mut self.lines);
        let before_revision = self.revision;
        self.revision += 1;
        AppliedSourceEdit {
            changes: SourceChangeSet {
                before_revision,
                revision: self.revision,
                changes,
            },
            inverse,
        }
    }
}

fn index_lines(text: &str, lines: &mut Vec<Range<usize>>) {
    let mut start = 0;
    let mut bytes = text.bytes().enumerate().peekable();
    while let Some((offset, byte)) = bytes.next() {
        if matches!(byte, b'\r' | b'\n') {
            lines.push(start..offset);
            start = offset + 1;
            if byte == b'\r' && bytes.next_if(|&(_, byte)| byte == b'\n').is_some() {
                start += 1;
            }
        }
    }
    lines.push(start..text.len());
}

/// Visual properties of a source line. Ranges are grapheme-aligned and line-local.
/// No part of this output is written to the source file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceLinePresentation {
    pub line: usize,
    pub style: BlockStyle,
    pub hidden: Vec<Range<usize>>,
}

/// A language-independent presentation extension. Results belong to the supplied revision.
/// Returning no presentation for a line leaves it as ordinary source text.
pub trait SourcePresentation {
    fn project(
        &mut self,
        source: &SourceBuffer,
        change: Option<&SourceChangeSet>,
    ) -> Vec<SourceLinePresentation>;

    /// Translates a rich edit intent into a source patch, if the dialect supports it.
    /// Returning `None` leaves unsupported structural edits disabled in source mode.
    fn plan_edit(
        &mut self,
        _source: &SourceBuffer,
        _selection: Range<usize>,
        _command: &RichCommand,
    ) -> Option<Vec<SourceEdit>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_preserving_transactions_round_trip_exact_bytes() {
        let original = "\u{feff}# T  \r\nname = \"é\"\nlast\rno final newline";
        let mut source = SourceBuffer::new(original.into());
        assert_eq!(source.text().as_bytes(), original.as_bytes());
        assert_eq!(source.line_break_at(0), "\r\n");
        assert_eq!(source.line_break_at(1), "\n");
        assert_eq!(source.line_break_at(2), "\r");
        let title = original.find('T').unwrap();
        let value = original.find('é').unwrap();
        let edit = source
            .apply(
                0,
                vec![
                    SourceEdit {
                        range: value..value + 'é'.len_utf8(),
                        text: "next".into(),
                    },
                    SourceEdit {
                        range: title - 1..title,
                        text: String::new(),
                    },
                ],
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            source.text(),
            "\u{feff}#T  \r\nname = \"next\"\nlast\rno final newline"
        );
        assert_eq!(
            edit.changes.map_offset(value, SourceBias::Before),
            Some(value - 1)
        );
        assert_eq!(
            edit.changes.map_offset(value, SourceBias::After),
            Some(value + 3)
        );
        assert_eq!(edit.changes.map_offset(value + 1, SourceBias::After), None);
        assert!(matches!(
            source.apply(0, Vec::new()),
            Err(SourceEditError::StaleRevision { .. })
        ));
        source.apply(1, edit.inverse).unwrap().unwrap();
        assert_eq!(source.text().as_bytes(), original.as_bytes());
        assert!(matches!(
            source.apply(
                2,
                vec![SourceEdit {
                    range: value + 1..value + 2,
                    text: "x".into()
                }]
            ),
            Err(SourceEditError::InvalidRange(_))
        ));
        assert_eq!(source.text().as_bytes(), original.as_bytes());
    }
}
