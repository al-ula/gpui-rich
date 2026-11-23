//! Pluggable [`DocumentCodec`] abstraction with format-typed direct calls, dynamic
//! [`CodecRegistry`] dispatch, and built-in Markdown (GFM), HTML, and JSON codecs.
//!
//! Direct Rust codec calls tie [`EncodedDocument<Format>`] to a codec's associated [`DocumentCodec::Format`]
//! so encoded text from one codec cannot be passed to another by mistake:
//!
//! ```rust
//! use gpui_rich::model::{Document, DocumentCodec, HtmlCodec, InlineContent, MarkdownCodec};
//!
//! let mut doc = Document::new();
//! doc.add_paragraph(InlineContent::plain("Hello"), Default::default());
//! let encoded_md = MarkdownCodec.encode(&doc).unwrap();
//! let roundtrip = MarkdownCodec.decode(&encoded_md).unwrap();
//! assert_eq!(roundtrip.blocks.len(), 1);
//!
//! let encoded_html = HtmlCodec::default().encode(&doc).unwrap();
//! let _ = HtmlCodec::default().decode(&encoded_html).unwrap();
//! ```
//!
//! Mixing encoded formats fails at compile time:
//! ```compile_fail
//! use gpui_rich::model::{Document, DocumentCodec, HtmlCodec, MarkdownCodec};
//!
//! let doc = Document::new();
//! let encoded_md = MarkdownCodec.encode(&doc).unwrap();
//! let _ = HtmlCodec::default().decode(&encoded_md);
//! ```

use crate::model::block::{
    BlockKind, BlockNode, ColumnSpec, Document, ListId, ListKind, NumberStyle, TableBlock,
    TableBorderStyle, TableCell, TableRow, block_kinds,
};
use crate::model::clipboard::ClipboardFragment;
use crate::model::inline::{
    ChipKind, ImageAssetId, InlineContent, InlineObject, InlineVAlign, OBJECT_REPLACEMENT_CHAR,
};
use crate::model::payload::CustomPayload;
use crate::model::style::{
    BaselineShift, BlockStyle, FontWeight, InlineStyle, StyleId, TextAlign, UnderlineKind,
};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

/// Format marker for [`JsonCodec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JsonFormat;

/// Format marker for [`HtmlCodec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HtmlFormat;

/// Format marker for [`MarkdownCodec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MarkdownFormat;

/// Encoded document text tagged with its concrete codec `Format` marker.
///
/// Direct calls to [`DocumentCodec::encode`] return `EncodedDocument<Self::Format>`, preventing
/// callers from passing one codec's output to another codec's [`DocumentCodec::decode`].
/// Constructing an [`EncodedDocument`] from untrusted input via [`Self::from_raw`] is explicit
/// and still validated by [`DocumentCodec::decode`].
pub struct EncodedDocument<Format> {
    text: String,
    _format: PhantomData<fn() -> Format>,
}

impl<Format> EncodedDocument<Format> {
    /// Tags raw untrusted text as a candidate for `Format`. Validation happens in [`DocumentCodec::decode`].
    pub fn from_raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            _format: PhantomData,
        }
    }

    /// Borrows the encoded string content.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Unwraps the inner encoded string.
    pub fn into_string(self) -> String {
        self.text
    }
}

impl<Format> Clone for EncodedDocument<Format> {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            _format: PhantomData,
        }
    }
}

impl<Format> fmt::Debug for EncodedDocument<Format> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedDocument")
            .field("text", &self.text)
            .finish()
    }
}

impl<Format> PartialEq for EncodedDocument<Format> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl<Format> Eq for EncodedDocument<Format> {}

/// Stable codec format identifier such as `"markdown"`, `"html"`, or `"json"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormatId(String);

impl FormatId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for FormatId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for FormatId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// MIME media type lookup key such as `"text/markdown"` or `"text/html"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MimeType(String);

impl MimeType {
    pub fn new(mime: impl Into<String>) -> Self {
        Self(mime.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for MimeType {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for MimeType {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for MimeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// File suffix lookup key such as `"md"` or `"html"` (leading dots are normalized away).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileExtension(String);

impl FileExtension {
    pub fn new(extension: impl Into<String>) -> Self {
        let raw = extension.into();
        let trimmed = raw.trim_start_matches('.').to_owned();
        Self(trimmed)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for FileExtension {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for FileExtension {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for FileExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Structured error returned by [`DocumentCodec`] and [`CodecRegistry`] operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// Attempted to register a codec with a `format_id` already present in the registry.
    DuplicateFormatId { format_id: FormatId },
    /// No codec was registered for `format_id`.
    UnknownFormatId { format_id: FormatId },
    /// No codec claimed `mime_type`.
    UnknownMimeType { mime_type: MimeType },
    /// No codec claimed `extension`.
    UnknownFileExtension { extension: FileExtension },
    /// Multiple registered codecs claimed `mime_type`; select explicitly by [`FormatId`].
    AmbiguousMimeType {
        mime_type: MimeType,
        candidates: Vec<FormatId>,
    },
    /// Multiple registered codecs claimed `extension`; select explicitly by [`FormatId`].
    AmbiguousFileExtension {
        extension: FileExtension,
        candidates: Vec<FormatId>,
    },
    /// Input could not be parsed by the codec.
    Parse {
        format_id: FormatId,
        message: String,
    },
    /// Document could not be serialized by the codec.
    Serialize {
        format_id: FormatId,
        message: String,
    },
}

impl CodecError {
    /// Constructs a parse error for `format_id`.
    pub fn parse(format_id: impl Into<FormatId>, message: impl Into<String>) -> Self {
        Self::Parse {
            format_id: format_id.into(),
            message: message.into(),
        }
    }

    /// Constructs a serialization error for `format_id`.
    pub fn serialize(format_id: impl Into<FormatId>, message: impl Into<String>) -> Self {
        Self::Serialize {
            format_id: format_id.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateFormatId { format_id } => {
                write!(
                    formatter,
                    "duplicate document codec format ID '{format_id}'"
                )
            }
            Self::UnknownFormatId { format_id } => {
                write!(formatter, "unknown document codec format ID '{format_id}'")
            }
            Self::UnknownMimeType { mime_type } => {
                write!(
                    formatter,
                    "no document codec registered for MIME type '{mime_type}'"
                )
            }
            Self::UnknownFileExtension { extension } => {
                write!(
                    formatter,
                    "no document codec registered for file extension '{extension}'"
                )
            }
            Self::AmbiguousMimeType {
                mime_type,
                candidates,
            } => write!(
                formatter,
                "ambiguous MIME type '{mime_type}' matched multiple codecs: {}",
                candidates
                    .iter()
                    .map(FormatId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::AmbiguousFileExtension {
                extension,
                candidates,
            } => write!(
                formatter,
                "ambiguous file extension '{extension}' matched multiple codecs: {}",
                candidates
                    .iter()
                    .map(FormatId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Parse { format_id, message } => {
                write!(
                    formatter,
                    "codec '{format_id}' failed to parse input: {message}"
                )
            }
            Self::Serialize { format_id, message } => {
                write!(
                    formatter,
                    "codec '{format_id}' failed to serialize document: {message}"
                )
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// Pluggable document serializer and deserializer bound to an associated [`Self::Format`] marker.
pub trait DocumentCodec {
    /// Marker type distinguishing this codec's [`EncodedDocument`] from other formats at compile time.
    type Format: 'static;

    /// Stable short identifier such as `"markdown"`, `"html"`, or `"json"`.
    fn format_id(&self) -> &str;

    /// Supported MIME types such as `["text/markdown"]`.
    fn mime_types(&self) -> &[&str];

    /// Supported file extensions (without leading dot) such as `["md", "markdown"]`.
    fn file_extensions(&self) -> &[&str];

    /// Serializes `document` into a format-tagged [`EncodedDocument`].
    fn encode(&self, document: &Document) -> Result<EncodedDocument<Self::Format>, CodecError>;

    /// Parses a format-tagged [`EncodedDocument`] into a fresh [`Document`].
    fn decode(&self, input: &EncodedDocument<Self::Format>) -> Result<Document, CodecError>;

    /// Explicit fallible decode from untrusted raw text (for example from disk or network).
    fn decode_raw(&self, input: &str) -> Result<Document, CodecError> {
        let candidate = EncodedDocument::<Self::Format>::from_raw(input);
        self.decode(&candidate)
    }
}

/// Format-erased dynamic codec interface exposed only inside [`CodecRegistry`] lookup.
pub trait DynamicDocumentCodec {
    /// Stable short identifier such as `"markdown"`, `"html"`, or `"json"`.
    fn format_id(&self) -> &str;

    /// Supported MIME types such as `["text/markdown"]`.
    fn mime_types(&self) -> &[&str];

    /// Supported file extensions (without leading dot) such as `["md", "markdown"]`.
    fn file_extensions(&self) -> &[&str];

    /// Serializes `document` into raw text.
    fn encode_raw(&self, document: &Document) -> Result<String, CodecError>;

    /// Parses untrusted raw text into a fresh [`Document`].
    fn decode_raw(&self, input: &str) -> Result<Document, CodecError>;
}

struct ErasedCodec<C>(C);

impl<C: DocumentCodec> DynamicDocumentCodec for ErasedCodec<C> {
    fn format_id(&self) -> &str {
        self.0.format_id()
    }

    fn mime_types(&self) -> &[&str] {
        self.0.mime_types()
    }

    fn file_extensions(&self) -> &[&str] {
        self.0.file_extensions()
    }

    fn encode_raw(&self, document: &Document) -> Result<String, CodecError> {
        self.0.encode(document).map(EncodedDocument::into_string)
    }

    fn decode_raw(&self, input: &str) -> Result<Document, CodecError> {
        self.0.decode_raw(input)
    }
}

/// Registry of pluggable [`DocumentCodec`] implementations indexed by [`FormatId`], [`MimeType`],
/// or [`FileExtension`].
#[derive(Default)]
pub struct CodecRegistry {
    codecs: Vec<Box<dyn DynamicDocumentCodec>>,
}

impl CodecRegistry {
    /// Creates an empty codec registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry pre-populated with [`MarkdownCodec`], [`HtmlCodec`], and [`JsonCodec`].
    pub fn with_builtin_codecs() -> Self {
        let mut registry = Self::new();
        for registration in [
            registry.register(MarkdownCodec),
            registry.register(HtmlCodec::default()),
            registry.register(JsonCodec),
        ] {
            if let Err(error) = registration {
                debug_assert!(false, "built-in codec registration failed: {error}");
            }
        }
        registry
    }

    /// Registers `codec`, rejecting duplicate format identifiers (case-insensitive).
    pub fn register<C: DocumentCodec + 'static>(&mut self, codec: C) -> Result<(), CodecError> {
        let format_id = codec.format_id();
        if self
            .codecs
            .iter()
            .any(|existing| existing.format_id().eq_ignore_ascii_case(format_id))
        {
            return Err(CodecError::DuplicateFormatId {
                format_id: FormatId::new(format_id),
            });
        }
        self.codecs.push(Box::new(ErasedCodec(codec)));
        Ok(())
    }

    /// Looks up a registered codec by its [`FormatId`] (case-insensitive).
    pub fn get(&self, format_id: &FormatId) -> Result<&dyn DynamicDocumentCodec, CodecError> {
        self.codecs
            .iter()
            .find(|codec| codec.format_id().eq_ignore_ascii_case(format_id.as_str()))
            .map(Box::as_ref)
            .ok_or_else(|| CodecError::UnknownFormatId {
                format_id: format_id.clone(),
            })
    }

    /// Looks up a registered codec matching `mime_type` (case-insensitive), reporting ambiguity
    /// when multiple codecs claim the same MIME type.
    pub fn get_by_mime(
        &self,
        mime_type: &MimeType,
    ) -> Result<&dyn DynamicDocumentCodec, CodecError> {
        let matches: Vec<&dyn DynamicDocumentCodec> = self
            .codecs
            .iter()
            .filter(|codec| {
                codec
                    .mime_types()
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(mime_type.as_str()))
            })
            .map(Box::as_ref)
            .collect();
        match matches.as_slice() {
            [] => Err(CodecError::UnknownMimeType {
                mime_type: mime_type.clone(),
            }),
            [single] => Ok(*single),
            many => Err(CodecError::AmbiguousMimeType {
                mime_type: mime_type.clone(),
                candidates: many
                    .iter()
                    .map(|codec| FormatId::new(codec.format_id()))
                    .collect(),
            }),
        }
    }

    /// Looks up a registered codec matching `extension`, reporting ambiguity when multiple codecs
    /// claim the same suffix.
    pub fn get_by_extension(
        &self,
        extension: &FileExtension,
    ) -> Result<&dyn DynamicDocumentCodec, CodecError> {
        let trimmed = extension.as_str().trim_start_matches('.');
        let matches: Vec<&dyn DynamicDocumentCodec> = self
            .codecs
            .iter()
            .filter(|codec| {
                codec
                    .file_extensions()
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(trimmed))
            })
            .map(Box::as_ref)
            .collect();
        match matches.as_slice() {
            [] => Err(CodecError::UnknownFileExtension {
                extension: extension.clone(),
            }),
            [single] => Ok(*single),
            many => Err(CodecError::AmbiguousFileExtension {
                extension: extension.clone(),
                candidates: many
                    .iter()
                    .map(|codec| FormatId::new(codec.format_id()))
                    .collect(),
            }),
        }
    }

    /// Encodes `document` using the codec registered for `format_id`.
    pub fn encode_as(
        &self,
        format_id: &FormatId,
        document: &Document,
    ) -> Result<String, CodecError> {
        let codec = self.get(format_id)?;
        codec.encode_raw(document)
    }

    /// Decodes raw `input` using the codec registered for `format_id`.
    pub fn decode_as(&self, format_id: &FormatId, input: &str) -> Result<Document, CodecError> {
        let codec = self.get(format_id)?;
        codec.decode_raw(input)
    }

    /// Decodes raw `input` using the unique codec matching `mime_type`.
    pub fn decode_by_mime(
        &self,
        mime_type: &MimeType,
        input: &str,
    ) -> Result<Document, CodecError> {
        let codec = self.get_by_mime(mime_type)?;
        codec.decode_raw(input)
    }

    /// Decodes raw `input` using the unique codec matching `extension`.
    pub fn decode_by_extension(
        &self,
        extension: &FileExtension,
        input: &str,
    ) -> Result<Document, CodecError> {
        let codec = self.get_by_extension(extension)?;
        codec.decode_raw(input)
    }
}

/// Built-in JSON [`DocumentCodec`] wrapping [`Document::to_json_pretty`] and [`Document::from_json`].
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonCodec;

impl DocumentCodec for JsonCodec {
    type Format = JsonFormat;

    fn format_id(&self) -> &str {
        "json"
    }

    fn mime_types(&self) -> &[&str] {
        &["application/json"]
    }

    fn file_extensions(&self) -> &[&str] {
        &["json"]
    }

    fn encode(&self, document: &Document) -> Result<EncodedDocument<Self::Format>, CodecError> {
        let text = document
            .to_json_pretty()
            .map_err(|error| CodecError::serialize(self.format_id(), error.to_string()))?;
        Ok(EncodedDocument::from_raw(text))
    }

    fn decode(&self, input: &EncodedDocument<Self::Format>) -> Result<Document, CodecError> {
        Document::from_json(input.as_str())
            .map_err(|error| CodecError::parse(self.format_id(), error.to_string()))
    }
}

/// Built-in HTML [`DocumentCodec`] built over normalized semantic HTML serialization and parsing.
#[derive(Clone, Copy, Debug, Default)]
pub struct HtmlCodec {
    /// When true, wraps encoded HTML in a minimal `<!DOCTYPE html><html><body>...</body></html>` envelope.
    pub full_document: bool,
}

impl HtmlCodec {
    pub const fn new(full_document: bool) -> Self {
        Self { full_document }
    }
}

impl DocumentCodec for HtmlCodec {
    type Format = HtmlFormat;

    fn format_id(&self) -> &str {
        "html"
    }

    fn mime_types(&self) -> &[&str] {
        &["text/html"]
    }

    fn file_extensions(&self) -> &[&str] {
        &["html", "htm"]
    }

    fn encode(&self, document: &Document) -> Result<EncodedDocument<Self::Format>, CodecError> {
        let body = ClipboardFragment::from_document(document).to_html();
        let text = if self.full_document {
            format!(
                "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body>{body}</body></html>"
            )
        } else {
            body
        };
        Ok(EncodedDocument::from_raw(text))
    }

    fn decode(&self, input: &EncodedDocument<Self::Format>) -> Result<Document, CodecError> {
        let fragment = ClipboardFragment::from_html(input.as_str());
        let mut document = Document::new();
        for node in fragment.to_block_nodes(&mut document) {
            document.push_block(node);
        }
        Ok(document)
    }
}

/// Built-in GitHub-Flavored Markdown [`DocumentCodec`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownCodec;

impl DocumentCodec for MarkdownCodec {
    type Format = MarkdownFormat;

    fn format_id(&self) -> &str {
        "markdown"
    }

    fn mime_types(&self) -> &[&str] {
        &["text/markdown", "text/x-markdown"]
    }

    fn file_extensions(&self) -> &[&str] {
        &["md", "markdown"]
    }

    fn encode(&self, document: &Document) -> Result<EncodedDocument<Self::Format>, CodecError> {
        let mut output = String::new();
        encode_markdown_blocks(&document.blocks, "", &mut output)?;
        Ok(EncodedDocument::from_raw(output))
    }

    fn decode(&self, input: &EncodedDocument<Self::Format>) -> Result<Document, CodecError> {
        let normalized = input.as_str().replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized.split('\n').collect();
        let mut document = Document::new();
        let blocks = parse_markdown_blocks(&lines, &mut document);
        for block in blocks {
            document.push_block(block);
        }
        Ok(document)
    }
}

// ============================================================================
// Markdown Encoding
// ============================================================================

fn encode_markdown_blocks(
    blocks: &[BlockNode],
    line_prefix: &str,
    output: &mut String,
) -> Result<(), CodecError> {
    let mut ordered_counters: HashMap<(ListId, u8), usize> = HashMap::new();

    for (index, block) in blocks.iter().enumerate() {
        let is_list_item = block.kind.list_item_data().is_some();
        let prev_is_list_item = index
            .checked_sub(1)
            .and_then(|prev| blocks.get(prev))
            .is_some_and(|prev| prev.kind.list_item_data().is_some());

        if index > 0 {
            if is_list_item && prev_is_list_item {
                output.push('\n');
            } else {
                output.push('\n');
                if !line_prefix.is_empty() {
                    output.push_str(line_prefix.trim_end());
                }
                output.push('\n');
            }
        }

        match &block.kind {
            BlockKind::Text {
                kind,
                data,
                content,
            } => match kind.as_ref() {
                block_kinds::HEADING => {
                    let level = block.kind.heading_data().map(|h| h.level).unwrap_or(1);
                    output.push_str(line_prefix);
                    let clamped = level.clamp(1, 6) as usize;
                    for _ in 0..clamped {
                        output.push('#');
                    }
                    output.push(' ');
                    output.push_str(&encode_markdown_inline(content));
                }
                block_kinds::LIST_ITEM => {
                    if let Some(item) = block.kind.list_item_data() {
                        ordered_counters.retain(|&(id, level), _| {
                            id != item.list_id || level <= item.indent_level
                        });
                        output.push_str(line_prefix);
                        for _ in 0..(item.indent_level as usize) {
                            output.push_str("  ");
                        }
                        match item.kind {
                            ListKind::Bullet => output.push_str("- "),
                            ListKind::Checklist => {
                                if item.checked.unwrap_or(false) {
                                    output.push_str("- [x] ");
                                } else {
                                    output.push_str("- [ ] ");
                                }
                            }
                            ListKind::Ordered(_) => {
                                let counter = ordered_counters
                                    .entry((item.list_id, item.indent_level))
                                    .or_insert(0);
                                *counter += 1;
                                output.push_str(&format!("{counter}. "));
                            }
                        }
                        output.push_str(&encode_markdown_inline(content));
                    }
                }
                block_kinds::CODE_BLOCK => {
                    output.push_str(line_prefix);
                    output.push_str("```");
                    if let Some(code) = block.kind.code_block_data()
                        && let Some(lang) = &code.language
                    {
                        output.push_str(lang);
                    }
                    output.push('\n');
                    for line in content.text.split('\n') {
                        output.push_str(line_prefix);
                        output.push_str(line);
                        output.push('\n');
                    }
                    output.push_str(line_prefix);
                    output.push_str("```");
                }
                block_kinds::PARAGRAPH => {
                    let rendered = encode_markdown_inline(content);
                    for (line_idx, line) in rendered.split('\n').enumerate() {
                        if line_idx > 0 {
                            output.push_str("  \n");
                        }
                        output.push_str(line_prefix);
                        output.push_str(line);
                    }
                }
                _ => {
                    output.push_str(line_prefix);
                    let payload = serde_json::to_string(data)
                        .map_err(|error| CodecError::serialize("markdown", error.to_string()))?;
                    output.push_str(&format!("<!-- gpui-rich:{kind} {payload} -->\n"));
                    let rendered = encode_markdown_inline(content);
                    for (line_idx, line) in rendered.split('\n').enumerate() {
                        if line_idx > 0 {
                            output.push_str("  \n");
                        }
                        output.push_str(line_prefix);
                        output.push_str(line);
                    }
                }
            },
            BlockKind::Container {
                kind,
                data,
                children,
            } => {
                if kind.as_ref() == block_kinds::BLOCKQUOTE {
                    let nested_prefix = format!("{line_prefix}> ");
                    encode_markdown_blocks(children, &nested_prefix, output)?;
                } else {
                    output.push_str(line_prefix);
                    let payload = serde_json::to_string(data)
                        .map_err(|error| CodecError::serialize("markdown", error.to_string()))?;
                    output.push_str(&format!("<!-- gpui-rich:{kind} {payload} -->\n"));
                    let nested_prefix = format!("{line_prefix}> ");
                    encode_markdown_blocks(children, &nested_prefix, output)?;
                }
            }
            BlockKind::Grid { table, .. } => {
                encode_markdown_table(table, line_prefix, output);
            }
            BlockKind::Leaf { kind, data } => {
                if kind.as_ref() == block_kinds::HORIZONTAL_RULE {
                    output.push_str(line_prefix);
                    output.push_str("---");
                } else {
                    output.push_str(line_prefix);
                    let payload = serde_json::to_string(data)
                        .map_err(|error| CodecError::serialize("markdown", error.to_string()))?;
                    output.push_str(&format!("<!-- gpui-rich:{kind} {payload} -->"));
                }
            }
        }
    }
    Ok(())
}

fn encode_markdown_table(table: &TableBlock, line_prefix: &str, output: &mut String) {
    if table.rows.is_empty() {
        return;
    }
    let col_count = table.grid_col_count();
    let (_, placements, occupancy) = table.resolve_grid();

    let render_row = |row_idx: usize| -> Vec<(String, TextAlign)> {
        let mut cells = Vec::with_capacity(col_count);
        for col in 0..col_count {
            let text_and_align = occupancy
                .get(row_idx)
                .and_then(|row_slots| row_slots.get(col))
                .and_then(|slot| *slot)
                .and_then(|placement_idx| placements.get(placement_idx))
                .and_then(|placement| {
                    if placement.row_index != row_idx || placement.visual_col != col {
                        return None;
                    }
                    let cell = table
                        .rows
                        .get(placement.row_index)?
                        .cells
                        .get(placement.cell_index)?;
                    let align = cell
                        .blocks
                        .first()
                        .map(|block| block.style.align)
                        .unwrap_or(TextAlign::Left);
                    let joined: Vec<String> = cell
                        .blocks
                        .iter()
                        .filter_map(|block| block.kind.inline_content())
                        .map(encode_markdown_inline)
                        .collect();
                    Some((joined.join("<br>").replace('|', "\\|"), align))
                })
                .unwrap_or_else(|| (String::new(), TextAlign::Left));
            cells.push(text_and_align);
        }
        cells
    };

    for row_idx in 0..table.rows.len() {
        if row_idx > 0 {
            output.push('\n');
        }
        let cells = render_row(row_idx);
        output.push_str(line_prefix);
        output.push('|');
        for (cell_text, _) in &cells {
            output.push(' ');
            output.push_str(cell_text);
            output.push_str(" |");
        }
        if row_idx == 0 {
            output.push('\n');
            output.push_str(line_prefix);
            output.push('|');
            for (_, align) in &cells {
                let separator = match align {
                    TextAlign::Center => " :---: |",
                    TextAlign::Right => " ---: |",
                    TextAlign::Left | TextAlign::Justify => " --- |",
                };
                output.push_str(separator);
            }
        }
    }
}

fn encode_markdown_inline(content: &InlineContent) -> String {
    if content.is_empty() {
        return String::new();
    }
    let mut output = String::with_capacity(content.text.len());
    for span in &content.spans {
        let Some(slice) = content.text.get(span.range.clone()) else {
            continue;
        };
        let mut inner = String::new();
        for (rel_idx, character) in slice.char_indices() {
            let abs_idx = span.range.start + rel_idx;
            if character == OBJECT_REPLACEMENT_CHAR
                && let Some(entry) = content.object_at(abs_idx)
            {
                match &entry.object {
                    InlineObject::Image { asset_id, alt, .. } => {
                        inner.push_str(&format!("![{alt}]({asset_id})"));
                    }
                    InlineObject::SmartChip { label, .. } => {
                        inner.push_str(label);
                    }
                    InlineObject::FootnoteRef { index } => {
                        inner.push_str(&format!("[^{index}]"));
                    }
                    InlineObject::Custom { kind, .. } => {
                        inner.push_str(&format!("[{kind}]"));
                    }
                }
            } else {
                inner.push(character);
            }
        }

        let style = &span.style;
        let mut formatted = inner;
        if style.inline_code == Some(true) {
            formatted = format!("`{formatted}`");
        } else {
            if style
                .weight
                .is_some_and(|weight| weight >= FontWeight::BOLD)
            {
                formatted = format!("**{formatted}**");
            }
            if style.italic == Some(true) {
                formatted = format!("*{formatted}*");
            }
            if style.strikethrough == Some(true) {
                formatted = format!("~~{formatted}~~");
            }
            if style.underline.is_some() && style.link.is_none() {
                formatted = format!("<u>{formatted}</u>");
            }
            match style.baseline_shift {
                Some(BaselineShift::Superscript) => {
                    formatted = format!("<sup>{formatted}</sup>");
                }
                Some(BaselineShift::Subscript) => {
                    formatted = format!("<sub>{formatted}</sub>");
                }
                _ => {}
            }
        }
        if let Some(link) = &style.link {
            formatted = format!("[{formatted}]({})", link.href);
        }
        output.push_str(&formatted);
    }
    output
}

// ============================================================================
// Markdown Decoding (GFM)
// ============================================================================

fn parse_markdown_blocks(lines: &[&str], document: &mut Document) -> Vec<BlockNode> {
    let mut blocks = Vec::new();
    let mut index = 0usize;
    let mut active_list_id: Option<(bool, ListId)> = None;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            active_list_id = None;
            index += 1;
            continue;
        }

        // Custom block HTML comment round-trip.
        if let Some(rest) = trimmed.strip_prefix("<!-- gpui-rich:")
            && let Some(inner) = rest.strip_suffix("-->")
        {
            active_list_id = None;
            let inner_trimmed = inner.trim();
            let (kind, payload_str) = match inner_trimmed.split_once(' ') {
                Some((k, p)) => (k.trim(), p.trim()),
                None => (inner_trimmed, "null"),
            };
            let data = CustomPayload::from_json(
                serde_json::from_str(payload_str).unwrap_or(serde_json::Value::Null),
            );
            let id = document.alloc_block_id();
            blocks.push(BlockNode::new(
                id,
                BlockKind::raw_leaf(kind, data),
                BlockStyle {
                    space_before_pt: Some(6.0),
                    space_after_pt: Some(8.0),
                    ..BlockStyle::default()
                },
            ));
            index += 1;
            continue;
        }

        // Fenced code blocks (``` or ~~~).
        if let Some((fence_char, fence_len, language)) = parse_code_fence_open(trimmed) {
            active_list_id = None;
            index += 1;
            let mut code_lines = Vec::new();
            while index < lines.len() {
                if is_code_fence_close(lines[index].trim(), fence_char, fence_len) {
                    index += 1;
                    break;
                }
                code_lines.push(lines[index]);
                index += 1;
            }
            let id = document.alloc_block_id();
            blocks.push(BlockNode::new(
                id,
                BlockKind::code_block(language, InlineContent::plain(code_lines.join("\n"))),
                BlockStyle::named(StyleId::CODE_BLOCK),
            ));
            continue;
        }

        // Horizontal rule (`---`, `***`, `___`).
        if is_horizontal_rule(trimmed) {
            active_list_id = None;
            let id = document.alloc_block_id();
            blocks.push(BlockNode::new(
                id,
                BlockKind::horizontal_rule(),
                BlockStyle {
                    space_before_pt: Some(10.0),
                    space_after_pt: Some(10.0),
                    ..BlockStyle::default()
                },
            ));
            index += 1;
            continue;
        }

        // ATX Heading (`#` .. `######`).
        if let Some((level, text)) = parse_atx_heading(trimmed) {
            active_list_id = None;
            let id = document.alloc_block_id();
            let content = parse_markdown_inline(text);
            blocks.push(BlockNode::new(
                id,
                BlockKind::heading(level, false, content),
                BlockStyle::named(StyleId::heading(level)),
            ));
            index += 1;
            continue;
        }

        // Blockquote (`> ...`).
        if trimmed.starts_with('>') {
            active_list_id = None;
            let mut quote_lines = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim_start();
                if let Some(after_gt) = candidate.strip_prefix('>') {
                    quote_lines.push(after_gt.strip_prefix(' ').unwrap_or(after_gt));
                    index += 1;
                } else {
                    break;
                }
            }
            let mut children = parse_markdown_blocks(&quote_lines, document);
            if children.is_empty() {
                let child_id = document.alloc_block_id();
                children.push(BlockNode::new(
                    child_id,
                    BlockKind::paragraph(InlineContent::empty()),
                    BlockStyle::default(),
                ));
            }
            let id = document.alloc_block_id();
            blocks.push(BlockNode::new(
                id,
                BlockKind::blockquote(children),
                BlockStyle {
                    indent_start_pt: 16.0,
                    space_before_pt: Some(6.0),
                    space_after_pt: Some(10.0),
                    ..BlockStyle::default()
                },
            ));
            continue;
        }

        // GFM Table: header line + alignment separator line.
        if index + 1 < lines.len()
            && looks_like_table_row(trimmed)
            && let Some(alignments) = parse_table_separator(lines[index + 1].trim())
        {
            active_list_id = None;
            let header_cells = split_table_row(trimmed);
            let mut table_rows = Vec::new();
            let col_count = header_cells.len().max(alignments.len()).max(1);
            table_rows.push(build_markdown_table_row(
                &header_cells,
                &alignments,
                col_count,
                true,
                document,
            ));
            index += 2;
            while index < lines.len() {
                let row_trimmed = lines[index].trim();
                if row_trimmed.is_empty() || !looks_like_table_row(row_trimmed) {
                    break;
                }
                let row_cells = split_table_row(row_trimmed);
                table_rows.push(build_markdown_table_row(
                    &row_cells,
                    &alignments,
                    col_count,
                    false,
                    document,
                ));
                index += 1;
            }
            let id = document.alloc_block_id();
            blocks.push(BlockNode::new(
                id,
                BlockKind::table(TableBlock {
                    columns: vec![ColumnSpec::Auto; col_count],
                    rows: table_rows,
                    column_kinds: Default::default(),
                    border_style: TableBorderStyle::default(),
                }),
                BlockStyle::default(),
            ));
            continue;
        }

        // List items (bullet, checklist, ordered).
        if let Some((kind, indent_level, checked, body)) = parse_list_item_line(line) {
            let is_ordered = matches!(kind, ListKind::Ordered(_));
            let list_id = match active_list_id {
                Some((prev_ordered, id)) if prev_ordered == is_ordered => id,
                _ => {
                    let allocated = document.alloc_list_id();
                    let next_id = ListId(allocated.0.max(blocks.len() as u64 + 1));
                    active_list_id = Some((is_ordered, next_id));
                    next_id
                }
            };
            let id = document.alloc_block_id();
            let indent_pt = 24.0 * (f32::from(indent_level) + 1.0);
            let mut style = BlockStyle::default().with_indent(indent_pt, 0.0);
            style.space_after_pt = Some(4.0);
            blocks.push(BlockNode::new(
                id,
                BlockKind::list_item(
                    list_id,
                    kind,
                    indent_level,
                    checked,
                    parse_markdown_inline(body),
                ),
                style,
            ));
            index += 1;
            continue;
        }

        // Paragraph: collect contiguous plain lines until blank line or block starter.
        active_list_id = None;
        let mut paragraph_lines = Vec::new();
        while index < lines.len() {
            let current = lines[index];
            let current_trimmed = current.trim();
            if current_trimmed.is_empty()
                || (!paragraph_lines.is_empty()
                    && (parse_code_fence_open(current_trimmed).is_some()
                        || is_horizontal_rule(current_trimmed)
                        || parse_atx_heading(current_trimmed).is_some()
                        || current_trimmed.starts_with('>')
                        || parse_list_item_line(current).is_some()))
            {
                break;
            }
            paragraph_lines.push(current);
            index += 1;
        }

        let mut joined = String::new();
        for (line_idx, para_line) in paragraph_lines.iter().enumerate() {
            if line_idx > 0 {
                if paragraph_lines[line_idx - 1].ends_with("  ") {
                    joined.push('\n');
                } else {
                    joined.push(' ');
                }
            }
            joined.push_str(para_line.trim_end());
        }
        let id = document.alloc_block_id();
        blocks.push(BlockNode::new(
            id,
            BlockKind::paragraph(parse_markdown_inline(&joined)),
            BlockStyle::default(),
        ));
    }

    blocks
}

fn parse_code_fence_open(trimmed: &str) -> Option<(char, usize, Option<String>)> {
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let fence_len = trimmed.chars().take_while(|&ch| ch == first).count();
    if fence_len < 3 {
        return None;
    }
    let rest = trimmed[fence_len..].trim();
    if first == '`' && rest.contains('`') {
        return None;
    }
    let language = if rest.is_empty() {
        None
    } else {
        Some(rest.split_whitespace().next().unwrap_or(rest).to_string())
    };
    Some((first, fence_len, language))
}

fn is_code_fence_close(trimmed: &str, fence_char: char, min_len: usize) -> bool {
    let count = trimmed.chars().take_while(|&ch| ch == fence_char).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let mut marker: Option<char> = None;
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            continue;
        }
        if matches!(ch, '-' | '*' | '_') {
            match marker {
                None => {
                    marker = Some(ch);
                    count += 1;
                }
                Some(prev) if prev == ch => count += 1,
                _ => return false,
            }
        } else {
            return false;
        }
    }
    count >= 3
}

fn parse_atx_heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?;
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let content = rest.trim().trim_end_matches('#').trim_end();
    Some((hashes as u8, content))
}

fn parse_list_item_line(line: &str) -> Option<(ListKind, u8, Option<bool>, &str)> {
    let leading_spaces: usize = line
        .chars()
        .take_while(|&ch| ch == ' ' || ch == '\t')
        .map(|ch| if ch == '\t' { 2 } else { 1 })
        .sum();
    let indent_level = (leading_spaces / 2).min(6) as u8;
    let trimmed = line.trim_start();

    for bullet in ["- ", "* ", "+ "] {
        if let Some(body) = trimmed.strip_prefix(bullet) {
            if let Some(task_body) = body
                .strip_prefix("[ ] ")
                .or_else(|| body.strip_prefix("[] "))
            {
                return Some((ListKind::Checklist, indent_level, Some(false), task_body));
            }
            if let Some(task_body) = body
                .strip_prefix("[x] ")
                .or_else(|| body.strip_prefix("[X] "))
            {
                return Some((ListKind::Checklist, indent_level, Some(true), task_body));
            }
            return Some((ListKind::Bullet, indent_level, None, body));
        }
    }

    let digit_count = trimmed
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count > 0
        && let Some(after_digits) = trimmed.get(digit_count..)
        && let Some(body) = after_digits
            .strip_prefix(". ")
            .or_else(|| after_digits.strip_prefix(") "))
    {
        return Some((
            ListKind::Ordered(NumberStyle::Decimal),
            indent_level,
            None,
            body,
        ));
    }

    None
}

fn looks_like_table_row(trimmed: &str) -> bool {
    trimmed.contains('|')
}

fn split_table_row(trimmed: &str) -> Vec<String> {
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed.strip_prefix('|').unwrap_or(trimmed));
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            cells.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    cells.push(current.trim().to_string());
    cells
}

fn parse_table_separator(trimmed: &str) -> Option<Vec<TextAlign>> {
    if !trimmed.contains('-') || !trimmed.contains('|') {
        return None;
    }
    let parts = split_table_row(trimmed);
    if parts.is_empty() {
        return None;
    }
    let mut alignments = Vec::with_capacity(parts.len());
    for part in parts {
        let cell = part.trim();
        if cell.is_empty() {
            return None;
        }
        let left_colon = cell.starts_with(':');
        let right_colon = cell.ends_with(':');
        let core = cell.trim_matches(':');
        if core.is_empty() || !core.bytes().all(|byte| byte == b'-') {
            return None;
        }
        let align = match (left_colon, right_colon) {
            (true, true) => TextAlign::Center,
            (false, true) => TextAlign::Right,
            _ => TextAlign::Left,
        };
        alignments.push(align);
    }
    Some(alignments)
}

fn build_markdown_table_row(
    raw_cells: &[String],
    alignments: &[TextAlign],
    col_count: usize,
    is_header: bool,
    document: &mut Document,
) -> TableRow {
    let mut cells = Vec::with_capacity(col_count);
    for col in 0..col_count {
        let raw = raw_cells.get(col).map(String::as_str).unwrap_or("");
        let align = alignments.get(col).copied().unwrap_or(TextAlign::Left);
        let cell_id = document.alloc_block_id();
        let para_id = document.alloc_block_id();
        let normalized = raw.replace("<br>", "\n").replace("<br/>", "\n");
        let content = parse_markdown_inline(&normalized);
        cells.push(TableCell::simple(
            cell_id,
            vec![BlockNode::new(
                para_id,
                BlockKind::paragraph(content),
                BlockStyle::default().with_align(align),
            )],
        ));
    }
    TableRow { is_header, cells }
}

fn parse_markdown_inline(input: &str) -> InlineContent {
    let mut output = InlineContent::empty();
    parse_markdown_inline_into(input, &InlineStyle::default(), &mut output);
    output
}

fn parse_markdown_inline_into(input: &str, active_style: &InlineStyle, output: &mut InlineContent) {
    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    let mut plain_buffer = String::new();

    let flush_plain = |buffer: &mut String, out: &mut InlineContent| {
        if !buffer.is_empty() {
            out.push_text(buffer, active_style.clone());
            buffer.clear();
        }
    };

    while cursor < input.len() {
        let Some(rest) = input.get(cursor..) else {
            break;
        };

        // Backslash escape.
        if bytes[cursor] == b'\\'
            && let Some(next_char) = rest.chars().nth(1)
        {
            plain_buffer.push(next_char);
            cursor += 1 + next_char.len_utf8();
            continue;
        }

        // Inline code span: `code`.
        if bytes[cursor] == b'`'
            && let Some(close_rel) = rest.get(1..).and_then(|after| after.find('`'))
        {
            flush_plain(&mut plain_buffer, output);
            let code_text = &rest[1..(1 + close_rel)];
            let code_style = active_style.merge(&InlineStyle::code());
            output.push_text(code_text, code_style);
            cursor += close_rel + 2;
            continue;
        }

        // Image: ![alt](asset_id).
        if rest.starts_with("![")
            && let Some((alt, src, consumed)) = parse_md_link_or_image(rest, 2)
        {
            flush_plain(&mut plain_buffer, output);
            output.push_object(
                InlineObject::Image {
                    asset_id: ImageAssetId::new(src),
                    alt: alt.to_string(),
                    width_pt: 240.0,
                    height_pt: 150.0,
                    valign: InlineVAlign::Baseline,
                },
                active_style.clone(),
            );
            cursor += consumed;
            continue;
        }

        // Footnote reference: [^1].
        if rest.starts_with("[^")
            && let Some(close_rel) = rest.get(2..).and_then(|after| after.find(']'))
            && let Ok(index) = rest[2..(2 + close_rel)].parse::<u32>()
        {
            flush_plain(&mut plain_buffer, output);
            output.push_object(InlineObject::FootnoteRef { index }, active_style.clone());
            cursor += close_rel + 3;
            continue;
        }

        // Link: [label](href).
        if bytes[cursor] == b'['
            && let Some((label, href, consumed)) = parse_md_link_or_image(rest, 1)
        {
            flush_plain(&mut plain_buffer, output);
            let link_style = active_style.merge(&InlineStyle::default().with_link(href));
            parse_markdown_inline_into(label, &link_style, output);
            cursor += consumed;
            continue;
        }

        // Inline HTML tags for underline, superscript, subscript, smart chip fallback.
        if let Some((tag_inner, consumed, overlay)) = parse_inline_html_tag(rest) {
            flush_plain(&mut plain_buffer, output);
            let next_style = active_style.merge(&overlay);
            parse_markdown_inline_into(tag_inner, &next_style, output);
            cursor += consumed;
            continue;
        }

        // Bold (`**...**` or `__...__`).
        if (rest.starts_with("**") || rest.starts_with("__"))
            && let Some((inner, consumed)) = parse_delimited(rest, &rest[..2])
        {
            flush_plain(&mut plain_buffer, output);
            let next_style = active_style.merge(&InlineStyle::bold());
            parse_markdown_inline_into(inner, &next_style, output);
            cursor += consumed;
            continue;
        }

        // Strikethrough (`~~...~~`).
        if rest.starts_with("~~")
            && let Some((inner, consumed)) = parse_delimited(rest, "~~")
        {
            flush_plain(&mut plain_buffer, output);
            let next_style = active_style.merge(&InlineStyle::default().with_strikethrough(true));
            parse_markdown_inline_into(inner, &next_style, output);
            cursor += consumed;
            continue;
        }

        // Italic (`*...*` or `_..._`).
        if (bytes[cursor] == b'*' || bytes[cursor] == b'_')
            && !rest.starts_with("**")
            && !rest.starts_with("__")
            && let Some((inner, consumed)) = parse_delimited(rest, &rest[..1])
        {
            flush_plain(&mut plain_buffer, output);
            let next_style = active_style.merge(&InlineStyle::italic());
            parse_markdown_inline_into(inner, &next_style, output);
            cursor += consumed;
            continue;
        }

        if let Some(ch) = rest.chars().next() {
            plain_buffer.push(ch);
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }

    flush_plain(&mut plain_buffer, output);
}

fn parse_delimited<'a>(input: &'a str, marker: &str) -> Option<(&'a str, usize)> {
    let after_open = input.strip_prefix(marker)?;
    if after_open.is_empty() || after_open.starts_with(' ') {
        return None;
    }
    let close_pos = after_open.find(marker)?;
    if close_pos == 0 {
        return None;
    }
    let inner = &after_open[..close_pos];
    if inner.ends_with(' ') {
        return None;
    }
    Some((inner, marker.len() + close_pos + marker.len()))
}

fn parse_md_link_or_image(input: &str, open_len: usize) -> Option<(&str, &str, usize)> {
    let after_open = input.get(open_len..)?;
    let close_bracket = after_open.find(']')?;
    let label = &after_open[..close_bracket];
    let after_bracket = after_open.get(close_bracket + 1..)?;
    let after_paren = after_bracket.strip_prefix('(')?;
    let close_paren = after_paren.find(')')?;
    let target = after_paren[..close_paren].trim();
    if target.is_empty() || target.to_ascii_lowercase().starts_with("javascript:") {
        return None;
    }
    let consumed = open_len + close_bracket + 1 + 1 + close_paren + 1;
    Some((label, target, consumed))
}

fn parse_inline_html_tag(input: &str) -> Option<(&str, usize, InlineStyle)> {
    for (open, close, overlay) in [
        (
            "<u>",
            "</u>",
            InlineStyle::default().with_underline(UnderlineKind::Solid),
        ),
        (
            "<sup>",
            "</sup>",
            InlineStyle::default().with_baseline_shift(BaselineShift::Superscript),
        ),
        (
            "<sub>",
            "</sub>",
            InlineStyle::default().with_baseline_shift(BaselineShift::Subscript),
        ),
    ] {
        if let Some(after_open) = input.strip_prefix(open)
            && let Some(close_pos) = after_open.find(close)
        {
            return Some((
                &after_open[..close_pos],
                open.len() + close_pos + close.len(),
                overlay,
            ));
        }
    }
    let _ = ChipKind::Person;
    None
}
