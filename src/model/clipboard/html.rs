//! HTML/CSS serialization, tokenization, sanitization, and parsing for [`ClipboardFragment`].

use super::*;

pub(super) fn parse_html_fragment(html: &str) -> ClipboardFragment {
    let body = extract_html_fragment_slice(html);
    let tokens = tokenize_html(body);
    let has_block_boundary = tokens.iter().any(|token| match token {
        HtmlToken::StartTag { name, .. } | HtmlToken::EndTag { name } => matches!(
            name.as_str(),
            "p" | "div"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "pre"
                | "ul"
                | "ol"
                | "table"
                | "hr"
        ),
        HtmlToken::Text(_) => false,
    });
    let mut parser = HtmlFragmentParser::new();
    parser.parse(&tokens);
    let mut fragment = parser.finish();
    if !has_block_boundary
        && matches!(
            fragment.blocks.as_slice(),
            [ClipboardBlock::Paragraph { .. }]
        )
    {
        fragment.start_open = true;
        fragment.end_open = true;
    }
    fragment
}

// ============================================================================
// HTML Serialization
// ============================================================================

pub(super) fn write_blocks_html(
    blocks: &[ClipboardBlock],
    formatter: &dyn ClipboardCustomFormatter,
    output: &mut String,
) {
    let mut index = 0usize;
    while index < blocks.len() {
        if let ClipboardBlock::ListItem { kind, .. } = &blocks[index] {
            let is_ordered = matches!(kind, ListKind::Ordered(_));
            let tag = if is_ordered { "ol" } else { "ul" };
            let list_kind_attr = match kind {
                ListKind::Bullet => "bullet",
                ListKind::Ordered(NumberStyle::Decimal) => "decimal",
                ListKind::Ordered(NumberStyle::LowerAlpha) => "lower-alpha",
                ListKind::Ordered(NumberStyle::LowerRoman) => "lower-roman",
                ListKind::Checklist => "checklist",
            };
            output.push_str(&format!("<{tag} data-gpui-rich-list=\"{list_kind_attr}\">"));
            while let Some(ClipboardBlock::ListItem {
                kind: item_kind,
                indent_level,
                checked,
                content,
                align,
                ..
            }) = blocks.get(index)
            {
                if matches!(item_kind, ListKind::Ordered(_)) != is_ordered {
                    break;
                }
                output.push_str("<li");
                if *indent_level > 0 {
                    output.push_str(&format!(" data-gpui-rich-indent=\"{indent_level}\""));
                }
                if let Some(is_checked) = checked {
                    output.push_str(&format!(" data-checked=\"{is_checked}\""));
                }
                write_align_style_attr(*align, output);
                output.push('>');
                write_inline_html(content, formatter, output);
                output.push_str("</li>");
                index += 1;
            }
            output.push_str(&format!("</{tag}>"));
            continue;
        }

        if let Some(block) = blocks.get(index) {
            write_single_block_html(block, formatter, output);
        }
        index += 1;
    }
}

fn write_single_block_html(
    block: &ClipboardBlock,
    formatter: &dyn ClipboardCustomFormatter,
    output: &mut String,
) {
    match block {
        ClipboardBlock::Paragraph { content, align } => {
            output.push_str("<p");
            write_align_style_attr(*align, output);
            output.push('>');
            write_inline_html(content, formatter, output);
            output.push_str("</p>");
        }
        ClipboardBlock::Heading {
            level,
            content,
            align,
        } => {
            let clamped = (*level).clamp(1, 6);
            output.push_str(&format!("<h{clamped}"));
            write_align_style_attr(*align, output);
            output.push('>');
            write_inline_html(content, formatter, output);
            output.push_str(&format!("</h{clamped}>"));
        }
        ClipboardBlock::ListItem { content, .. } => {
            output.push_str("<li>");
            write_inline_html(content, formatter, output);
            output.push_str("</li>");
        }
        ClipboardBlock::Blockquote(children) => {
            output.push_str("<blockquote>");
            write_blocks_html(children, formatter, output);
            output.push_str("</blockquote>");
        }
        ClipboardBlock::CodeBlock { language, content } => {
            output.push_str("<pre><code");
            if let Some(lang) = language {
                output.push_str(" class=\"language-");
                push_escaped_html(lang, output);
                output.push('"');
            }
            output.push('>');
            push_escaped_html(&content.text, output);
            output.push_str("</code></pre>");
        }
        ClipboardBlock::Table(table) => {
            output.push_str("<table");
            if !table.column_kinds.is_empty() {
                output.push_str(" data-rich-column-kinds=\"");
                for (index, (column, kind)) in table.column_kinds.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(&column.to_string());
                    output.push(':');
                    output.push_str(kind.as_str());
                }
                output.push('"');
            }
            output.push_str("><tbody>");
            for row in &table.rows {
                output.push_str("<tr>");
                let cell_tag = if row.is_header { "th" } else { "td" };
                for cell in &row.cells {
                    output.push_str(&format!("<{cell_tag}"));
                    if cell.colspan > 1 {
                        output.push_str(&format!(" colspan=\"{}\"", cell.colspan));
                    }
                    if cell.rowspan > 1 {
                        output.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
                    }
                    output.push('>');
                    write_blocks_html(&cell.blocks, formatter, output);
                    output.push_str(&format!("</{cell_tag}>"));
                }
                output.push_str("</tr>");
            }
            output.push_str("</tbody></table>");
        }
        ClipboardBlock::HorizontalRule => {
            output.push_str("<hr>");
        }
        ClipboardBlock::Custom { kind, data } => {
            if let Some(custom_html) = formatter.html_block(BlockShapeKind::Leaf, kind, data) {
                output.push_str(&custom_html);
            } else {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
                output.push_str("<div data-gpui-rich-custom-block=\"");
                push_escaped_html(kind, output);
                output.push_str("\" data-gpui-rich-payload=\"");
                push_escaped_html(&payload, output);
                output.push_str("\">");
                push_escaped_html(
                    &formatter
                        .plain_text_block(BlockShapeKind::Leaf, kind, data)
                        .unwrap_or_else(|| format!("[{kind}]")),
                    output,
                );
                output.push_str("</div>");
            }
        }
        ClipboardBlock::CustomText {
            kind,
            data,
            content,
            align,
        } => {
            if let Some(custom_html) = formatter.html_block(BlockShapeKind::Text, kind, data) {
                output.push_str(&custom_html);
            } else {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
                output.push_str("<div data-gpui-rich-custom-text=\"");
                push_escaped_html(kind, output);
                output.push_str("\" data-gpui-rich-payload=\"");
                push_escaped_html(&payload, output);
                output.push('"');
                write_align_style_attr(*align, output);
                output.push('>');
                write_inline_html(content, formatter, output);
                output.push_str("</div>");
            }
        }
        ClipboardBlock::CustomContainer {
            kind,
            data,
            children,
            ..
        } => {
            if let Some(custom_html) = formatter.html_block(BlockShapeKind::Container, kind, data) {
                output.push_str(&custom_html);
            } else {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
                output.push_str("<div data-gpui-rich-custom-container=\"");
                push_escaped_html(kind, output);
                output.push_str("\" data-gpui-rich-payload=\"");
                push_escaped_html(&payload, output);
                output.push_str("\">");
                write_blocks_html(children, formatter, output);
                output.push_str("</div>");
            }
        }
        ClipboardBlock::CustomGrid { kind, data, table } => {
            if let Some(custom_html) = formatter.html_block(BlockShapeKind::Grid, kind, data) {
                output.push_str(&custom_html);
            } else {
                write_single_block_html(&ClipboardBlock::Table(table.clone()), formatter, output);
            }
        }
    }
}

fn write_align_style_attr(align: TextAlign, output: &mut String) {
    let css = match align {
        TextAlign::Left => return,
        TextAlign::Center => "center",
        TextAlign::Right => "right",
        TextAlign::Justify => "justify",
    };
    output.push_str(&format!(" style=\"text-align:{css}\""));
}

pub(super) fn write_inline_html(
    content: &InlineContent,
    formatter: &dyn ClipboardCustomFormatter,
    output: &mut String,
) {
    if content.is_empty() {
        return;
    }
    for span in &content.spans {
        let Some(slice) = content.text.get(span.range.clone()) else {
            continue;
        };
        let style = &span.style;
        let mut close_tags: Vec<&'static str> = Vec::new();

        if let Some(link) = &style.link
            && is_safe_href(&link.href)
        {
            output.push_str("<a href=\"");
            push_escaped_html(&link.href, output);
            output.push('"');
            if let Some(title) = &link.title {
                output.push_str(" title=\"");
                push_escaped_html(title, output);
                output.push('"');
            }
            output.push('>');
            close_tags.push("</a>");
        }

        let css_props = inline_style_css(style);
        if !css_props.is_empty() {
            output.push_str("<span style=\"");
            output.push_str(&css_props);
            output.push_str("\">");
            close_tags.push("</span>");
        }

        if style
            .weight
            .is_some_and(|weight| weight >= FontWeight::BOLD)
        {
            output.push_str("<strong>");
            close_tags.push("</strong>");
        }
        if style.italic == Some(true) {
            output.push_str("<em>");
            close_tags.push("</em>");
        }
        if style.underline.is_some() {
            output.push_str("<u>");
            close_tags.push("</u>");
        }
        if style.strikethrough == Some(true) {
            output.push_str("<s>");
            close_tags.push("</s>");
        }
        if style.inline_code == Some(true) {
            output.push_str("<code>");
            close_tags.push("</code>");
        }
        match style.baseline_shift {
            Some(BaselineShift::Superscript) => {
                output.push_str("<sup>");
                close_tags.push("</sup>");
            }
            Some(BaselineShift::Subscript) => {
                output.push_str("<sub>");
                close_tags.push("</sub>");
            }
            _ => {}
        }

        for (rel_index, character) in slice.char_indices() {
            let abs_index = span.range.start + rel_index;
            if character == OBJECT_REPLACEMENT_CHAR
                && let Some(entry) = content.object_at(abs_index)
            {
                write_inline_object_html(&entry.object, formatter, output);
            } else if character == '\n' {
                output.push_str("<br>");
            } else {
                push_escaped_char(character, output);
            }
        }

        for closing in close_tags.into_iter().rev() {
            output.push_str(closing);
        }
    }
}

fn write_inline_object_html(
    object: &InlineObject,
    formatter: &dyn ClipboardCustomFormatter,
    output: &mut String,
) {
    match object {
        InlineObject::Image {
            asset_id,
            alt,
            width_pt,
            height_pt,
            ..
        } => {
            let attribute = match asset_id.source() {
                ImageAssetSource::Linked(_) => "<img data-asset-id=\"",
                ImageAssetSource::Bundled(_) => "<img src=\"",
            };
            output.push_str(attribute);
            push_escaped_html(asset_id.as_str(), output);
            output.push_str("\" alt=\"");
            push_escaped_html(alt, output);
            output.push_str(&format!(
                "\" width=\"{}\" height=\"{}\">",
                width_pt.round() as i32,
                height_pt.round() as i32
            ));
        }
        InlineObject::SmartChip { kind, label, .. } => {
            let chip_kind = match kind {
                ChipKind::Person => "person",
                ChipKind::Date => "date",
                ChipKind::Document => "document",
                ChipKind::Tag => "tag",
            };
            output.push_str(&format!("<span data-gpui-rich-chip=\"{chip_kind}\">"));
            push_escaped_html(label, output);
            output.push_str("</span>");
        }
        InlineObject::FootnoteRef { index } => {
            output.push_str(&format!(
                "<sup data-gpui-rich-footnote=\"{index}\">[{index}]</sup>"
            ));
        }
        InlineObject::Custom { kind, data } => {
            if let Some(custom_html) = formatter.html_inline(kind, data) {
                output.push_str(&custom_html);
            } else {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
                output.push_str("<span data-gpui-rich-custom-inline=\"");
                push_escaped_html(kind, output);
                output.push_str("\" data-gpui-rich-payload=\"");
                push_escaped_html(&payload, output);
                output.push_str("\">");
                push_escaped_html(
                    &formatter
                        .plain_text_inline(kind, data)
                        .unwrap_or_else(|| format!("[{kind}]")),
                    output,
                );
                output.push_str("</span>");
            }
        }
    }
}

fn inline_style_css(style: &InlineStyle) -> String {
    let mut parts = Vec::new();
    if let Some(font_family) = &style.font_family {
        let mut escaped = String::new();
        push_escaped_html(font_family, &mut escaped);
        parts.push(format!("font-family:'{escaped}'"));
    }
    if let Some(color) = style.color {
        parts.push(format_rgba_css("color", color));
    }
    if let Some(background) = style.background {
        parts.push(format_rgba_css("background-color", background));
    }
    if let Some(font_size_pt) = style.font_size_pt {
        parts.push(format!("font-size:{font_size_pt:.1}pt"));
    }
    parts.join(";")
}

fn format_rgba_css(property: &str, color: RgbaColor) -> String {
    if color.a == 255 {
        format!("{property}:#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        let alpha = f32::from(color.a) / 255.0;
        format!(
            "{property}:rgba({},{},{},{alpha:.2})",
            color.r, color.g, color.b
        )
    }
}

fn push_escaped_html(input: &str, output: &mut String) {
    for character in input.chars() {
        push_escaped_char(character, output);
    }
}

fn push_escaped_char(character: char, output: &mut String) {
    match character {
        '&' => output.push_str("&amp;"),
        '<' => output.push_str("&lt;"),
        '>' => output.push_str("&gt;"),
        '"' => output.push_str("&quot;"),
        '\'' => output.push_str("&#39;"),
        _ => output.push(character),
    }
}

// ============================================================================
// Normalized HTML Parsing
// ============================================================================

#[derive(Debug)]
enum HtmlToken {
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Text(String),
}

fn extract_html_fragment_slice(html: &str) -> &str {
    const START_MARKER: &str = "<!--StartFragment-->";
    const END_MARKER: &str = "<!--EndFragment-->";
    if let Some(start_idx) = html.find(START_MARKER) {
        let after_start = start_idx + START_MARKER.len();
        if let Some(rest) = html.get(after_start..) {
            if let Some(end_rel) = rest.find(END_MARKER) {
                return rest.get(..end_rel).unwrap_or(rest);
            }
            return rest;
        }
    }
    html
}

fn tokenize_html(input: &str) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let rest = input.get(cursor..).unwrap_or("");
        let Some(lt_rel) = rest.find('<') else {
            if !rest.is_empty() {
                tokens.push(HtmlToken::Text(decode_html_entities(rest)));
            }
            break;
        };

        if lt_rel > 0
            && let Some(text_slice) = rest.get(..lt_rel)
        {
            tokens.push(HtmlToken::Text(decode_html_entities(text_slice)));
        }

        let tag_start = cursor + lt_rel;
        let from_tag = input.get(tag_start..).unwrap_or("");

        // Skip HTML comments (`<!-- ... -->`).
        if from_tag.starts_with("<!--") {
            if let Some(end_rel) = from_tag.find("-->") {
                cursor = tag_start + end_rel + 3;
            } else {
                break;
            }
            continue;
        }

        // Skip DOCTYPE or processing instructions (`<!...>` / `<?...>`).
        if from_tag.starts_with("<!") || from_tag.starts_with("<?") {
            if let Some(gt_rel) = from_tag.find('>') {
                cursor = tag_start + gt_rel + 1;
            } else {
                break;
            }
            continue;
        }

        let Some(gt_rel) = find_tag_close(from_tag) else {
            tokens.push(HtmlToken::Text(decode_html_entities(from_tag)));
            break;
        };

        let inner = from_tag.get(1..gt_rel).unwrap_or("").trim();
        cursor = tag_start + gt_rel + 1;

        if inner.is_empty() {
            continue;
        }

        if let Some(closing_name) = inner.strip_prefix('/') {
            let name = closing_name
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if !name.is_empty() {
                tokens.push(HtmlToken::EndTag { name });
            }
        } else {
            let self_closing = inner.ends_with('/');
            let trimmed = inner.trim_end_matches('/').trim();
            if let Some((name, attrs)) = parse_tag_and_attributes(trimmed) {
                let is_void = matches!(
                    name.as_str(),
                    "br" | "hr" | "img" | "meta" | "link" | "input" | "col"
                );
                tokens.push(HtmlToken::StartTag {
                    name,
                    attrs,
                    self_closing: self_closing || is_void,
                });
            }
        }
    }

    tokens
}

fn find_tag_close(slice: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, character) in slice.char_indices().skip(1) {
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == '>' {
            return Some(index);
        }
    }
    None
}

fn parse_tag_and_attributes(input: &str) -> Option<(String, Vec<(String, String)>)> {
    let mut chars = input.char_indices().peekable();
    let mut name_end = input.len();
    while let Some(&(index, character)) = chars.peek() {
        if character.is_ascii_whitespace() {
            name_end = index;
            break;
        }
        chars.next();
    }
    let name = input.get(..name_end)?.to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }

    let mut attrs = Vec::new();
    let mut rest = input.get(name_end..).unwrap_or("").trim_start();

    while !rest.is_empty() {
        let attr_name_end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=')
            .unwrap_or(rest.len());
        let attr_name = rest
            .get(..attr_name_end)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        rest = rest.get(attr_name_end..).unwrap_or("").trim_start();
        if attr_name.is_empty() {
            break;
        }
        if let Some(after_eq) = rest.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let (value, remaining) = if let Some(quoted) = after_eq.strip_prefix('"') {
                if let Some(end_quote) = quoted.find('"') {
                    (
                        quoted.get(..end_quote).unwrap_or(""),
                        quoted.get((end_quote + 1)..).unwrap_or(""),
                    )
                } else {
                    (quoted, "")
                }
            } else if let Some(quoted) = after_eq.strip_prefix('\'') {
                if let Some(end_quote) = quoted.find('\'') {
                    (
                        quoted.get(..end_quote).unwrap_or(""),
                        quoted.get((end_quote + 1)..).unwrap_or(""),
                    )
                } else {
                    (quoted, "")
                }
            } else {
                let end_val = after_eq
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(after_eq.len());
                (
                    after_eq.get(..end_val).unwrap_or(""),
                    after_eq.get(end_val..).unwrap_or(""),
                )
            };
            attrs.push((attr_name, decode_html_entities(value)));
            rest = remaining.trim_start();
        } else {
            attrs.push((attr_name, String::new()));
        }
    }

    Some((name, attrs))
}

fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rest) = input.get(cursor..) {
        let Some(amp_rel) = rest.find('&') else {
            output.push_str(rest);
            break;
        };
        if let Some(before) = rest.get(..amp_rel) {
            output.push_str(before);
        }
        let after_amp = rest.get((amp_rel + 1)..).unwrap_or("");
        if let Some(semi_rel) = after_amp.find(';')
            && semi_rel > 0
            && semi_rel <= 12
            && let Some(entity) = after_amp.get(..semi_rel)
            && let Some(decoded) = decode_single_entity(entity)
        {
            output.push_str(&decoded);
            cursor += amp_rel + 1 + semi_rel + 1;
            continue;
        }
        output.push('&');
        cursor += amp_rel + 1;
    }
    output
}

fn decode_single_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".into()),
        "lt" => Some("<".into()),
        "gt" => Some(">".into()),
        "quot" => Some("\"".into()),
        "apos" => Some("'".into()),
        "nbsp" => Some(" ".into()),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code_point =
                if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    num.parse::<u32>().ok()?
                };
            char::from_u32(code_point).map(|character| character.to_string())
        }
    }
}

fn attr_value<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[derive(Clone, Debug)]
enum PendingBlockKind {
    Paragraph {
        align: TextAlign,
    },
    Heading {
        level: u8,
        align: TextAlign,
    },
    ListItem {
        list_group: u64,
        kind: ListKind,
        indent_level: u8,
        checked: Option<bool>,
        align: TextAlign,
    },
    CodeBlock {
        language: Option<String>,
    },
}

struct PendingInlineBlock {
    kind: PendingBlockKind,
    content: InlineContent,
}

struct ListContext {
    group_id: u64,
    kind: ListKind,
}

struct TableBuildState {
    rows: Vec<ClipboardTableRow>,
    column_kinds: BTreeMap<usize, PlaceholderKind>,
    current_row: Option<ClipboardTableRow>,
    current_cell: Option<(u16, u16, Vec<ClipboardBlock>)>,
}

struct HtmlFragmentParser {
    blocks: Vec<ClipboardBlock>,
    blockquote_stack: Vec<Vec<ClipboardBlock>>,
    table_stack: Vec<TableBuildState>,
    list_stack: Vec<ListContext>,
    next_list_group: u64,
    style_stack: Vec<(String, InlineStyle)>,
    current_block: Option<PendingInlineBlock>,
    skip_depth: usize,
    in_pre: bool,
    pending_chip: Option<(ChipKind, String)>,
    pending_footnote: Option<u32>,
    skip_custom_inner_depth: usize,
}

impl HtmlFragmentParser {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            blockquote_stack: Vec::new(),
            table_stack: Vec::new(),
            list_stack: Vec::new(),
            next_list_group: 1,
            style_stack: Vec::new(),
            current_block: None,
            skip_depth: 0,
            in_pre: false,
            pending_chip: None,
            pending_footnote: None,
            skip_custom_inner_depth: 0,
        }
    }

    fn parse(&mut self, tokens: &[HtmlToken]) {
        for token in tokens {
            match token {
                HtmlToken::StartTag {
                    name,
                    attrs,
                    self_closing,
                } => self.handle_start_tag(name, attrs, *self_closing),
                HtmlToken::EndTag { name } => self.handle_end_tag(name),
                HtmlToken::Text(text) => self.handle_text(text),
            }
        }
    }

    fn finish(mut self) -> ClipboardFragment {
        self.flush_current_block();
        while let Some(table) = self.table_stack.pop() {
            let built = finish_table_state(table);
            if !built.rows.is_empty() {
                self.push_block(ClipboardBlock::Table(built));
            }
        }
        while let Some(quote_blocks) = self.blockquote_stack.pop() {
            if !quote_blocks.is_empty() {
                self.push_block(ClipboardBlock::Blockquote(quote_blocks));
            }
        }
        ClipboardFragment {
            blocks: self.blocks,
            start_open: false,
            end_open: false,
        }
    }

    fn current_inline_style(&self) -> InlineStyle {
        let mut combined = InlineStyle::default();
        for (_, overlay) in &self.style_stack {
            combined = combined.merge(overlay);
        }
        combined
    }

    fn push_block(&mut self, block: ClipboardBlock) {
        if let Some(table) = self.table_stack.last_mut()
            && let Some((_, _, cell_blocks)) = table.current_cell.as_mut()
        {
            cell_blocks.push(block);
            return;
        }
        if let Some(quote) = self.blockquote_stack.last_mut() {
            quote.push(block);
            return;
        }
        self.blocks.push(block);
    }

    fn flush_current_block(&mut self) {
        let Some(pending) = self.current_block.take() else {
            return;
        };
        let block = match pending.kind {
            PendingBlockKind::Paragraph { align } => ClipboardBlock::Paragraph {
                content: pending.content,
                align,
            },
            PendingBlockKind::Heading { level, align } => ClipboardBlock::Heading {
                level,
                content: pending.content,
                align,
            },
            PendingBlockKind::ListItem {
                list_group,
                kind,
                indent_level,
                checked,
                align,
            } => ClipboardBlock::ListItem {
                list_group,
                kind,
                indent_level,
                checked,
                content: pending.content,
                align,
            },
            PendingBlockKind::CodeBlock { language } => ClipboardBlock::CodeBlock {
                language,
                content: pending.content,
            },
        };
        self.push_block(block);
    }

    fn ensure_paragraph(&mut self) {
        if self.current_block.is_none() {
            self.current_block = Some(PendingInlineBlock {
                kind: PendingBlockKind::Paragraph {
                    align: TextAlign::Left,
                },
                content: InlineContent::empty(),
            });
        }
    }

    fn handle_start_tag(&mut self, name: &str, attrs: &[(String, String)], self_closing: bool) {
        if self.skip_depth > 0 {
            if !self_closing {
                self.skip_depth += 1;
            }
            return;
        }
        if self.skip_custom_inner_depth > 0 {
            if !self_closing {
                self.skip_custom_inner_depth += 1;
            }
            return;
        }

        if matches!(name, "script" | "style" | "head" | "title") {
            if !self_closing {
                self.skip_depth = 1;
            }
            return;
        }

        // Custom block round-trip (`<div data-gpui-rich-custom-block="...">`).
        if let Some(custom_block_kind) = attr_value(attrs, "data-gpui-rich-custom-block") {
            self.flush_current_block();
            let data = CustomPayload::from_json(
                attr_value(attrs, "data-gpui-rich-payload")
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or(serde_json::Value::Null),
            );
            self.push_block(ClipboardBlock::Custom {
                kind: SharedStr::from(custom_block_kind),
                data,
            });
            if !self_closing {
                self.skip_custom_inner_depth = 1;
            }
            return;
        }

        // Custom inline round-trip (`<span data-gpui-rich-custom-inline="...">`).
        if let Some(custom_inline_kind) = attr_value(attrs, "data-gpui-rich-custom-inline") {
            let data = CustomPayload::from_json(
                attr_value(attrs, "data-gpui-rich-payload")
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or(serde_json::Value::Null),
            );
            self.ensure_paragraph();
            let style = self.current_inline_style();
            if let Some(block) = self.current_block.as_mut() {
                block
                    .content
                    .push_object(InlineObject::raw_custom(custom_inline_kind, data), style);
            }
            if !self_closing {
                self.skip_custom_inner_depth = 1;
            }
            return;
        }

        // Smart chip round-trip.
        if let Some(chip_attr) = attr_value(attrs, "data-gpui-rich-chip") {
            let chip_kind = match chip_attr {
                "person" => ChipKind::Person,
                "date" => ChipKind::Date,
                "document" => ChipKind::Document,
                _ => ChipKind::Tag,
            };
            if !self_closing {
                self.pending_chip = Some((chip_kind, String::new()));
            }
            return;
        }

        // Footnote round-trip.
        if let Some(footnote_attr) = attr_value(attrs, "data-gpui-rich-footnote")
            && let Ok(index) = footnote_attr.parse::<u32>()
        {
            self.ensure_paragraph();
            let style = self.current_inline_style();
            if let Some(block) = self.current_block.as_mut() {
                block
                    .content
                    .push_object(InlineObject::FootnoteRef { index }, style);
            }
            if !self_closing {
                self.pending_footnote = Some(index);
            }
            return;
        }

        let css_style = attr_value(attrs, "style").unwrap_or("");
        let parsed_align = parse_css_text_align(css_style).unwrap_or(TextAlign::Left);

        match name {
            "p" | "div" => {
                self.flush_current_block();
                self.current_block = Some(PendingInlineBlock {
                    kind: PendingBlockKind::Paragraph {
                        align: parsed_align,
                    },
                    content: InlineContent::empty(),
                });
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush_current_block();
                let level = name
                    .as_bytes()
                    .get(1)
                    .map(|byte| byte.saturating_sub(b'0'))
                    .unwrap_or(1)
                    .clamp(1, 6);
                self.current_block = Some(PendingInlineBlock {
                    kind: PendingBlockKind::Heading {
                        level,
                        align: parsed_align,
                    },
                    content: InlineContent::empty(),
                });
            }
            "ul" | "ol" => {
                self.flush_current_block();
                let group_id = self.next_list_group;
                self.next_list_group += 1;
                let kind = match attr_value(attrs, "data-gpui-rich-list") {
                    Some("checklist") => ListKind::Checklist,
                    Some("lower-alpha") => ListKind::Ordered(NumberStyle::LowerAlpha),
                    Some("lower-roman") => ListKind::Ordered(NumberStyle::LowerRoman),
                    Some("decimal") => ListKind::Ordered(NumberStyle::Decimal),
                    Some("bullet") => ListKind::Bullet,
                    _ if name == "ol" => ListKind::Ordered(NumberStyle::Decimal),
                    _ => ListKind::Bullet,
                };
                self.list_stack.push(ListContext { group_id, kind });
            }
            "li" => {
                self.flush_current_block();
                let list_ctx = self.list_stack.last();
                let list_group = list_ctx.map(|ctx| ctx.group_id).unwrap_or(1);
                let mut kind = list_ctx.map(|ctx| ctx.kind).unwrap_or(ListKind::Bullet);
                let checked = match attr_value(attrs, "data-checked") {
                    Some("true") => {
                        kind = ListKind::Checklist;
                        Some(true)
                    }
                    Some("false") => {
                        kind = ListKind::Checklist;
                        Some(false)
                    }
                    _ => matches!(kind, ListKind::Checklist).then_some(false),
                };
                let indent_level = attr_value(attrs, "data-gpui-rich-indent")
                    .and_then(|raw| raw.parse::<u8>().ok())
                    .unwrap_or_else(|| self.list_stack.len().saturating_sub(1).min(6) as u8);
                self.current_block = Some(PendingInlineBlock {
                    kind: PendingBlockKind::ListItem {
                        list_group,
                        kind,
                        indent_level,
                        checked,
                        align: parsed_align,
                    },
                    content: InlineContent::empty(),
                });
            }
            "blockquote" => {
                self.flush_current_block();
                self.blockquote_stack.push(Vec::new());
            }
            "pre" => {
                self.flush_current_block();
                self.in_pre = true;
                self.current_block = Some(PendingInlineBlock {
                    kind: PendingBlockKind::CodeBlock { language: None },
                    content: InlineContent::empty(),
                });
            }
            "code" => {
                if self.in_pre {
                    if let Some(PendingInlineBlock {
                        kind: PendingBlockKind::CodeBlock { language },
                        ..
                    }) = self.current_block.as_mut()
                        && let Some(class_attr) = attr_value(attrs, "class")
                        && let Some(lang) = class_attr
                            .split_whitespace()
                            .find_map(|token| token.strip_prefix("language-"))
                    {
                        *language = Some(lang.to_owned());
                    }
                } else if !self_closing {
                    self.style_stack
                        .push((name.to_owned(), InlineStyle::code()));
                }
            }
            "table" => {
                self.flush_current_block();
                self.table_stack.push(TableBuildState {
                    rows: Vec::new(),
                    column_kinds: attr_value(attrs, "data-rich-column-kinds")
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|pair| {
                            let (column, kind) = pair.split_once(':')?;
                            Some((column.parse().ok()?, PlaceholderKind::from_name(kind)?))
                        })
                        .collect(),
                    current_row: None,
                    current_cell: None,
                });
            }
            "tr" => {
                self.flush_current_block();
                if let Some(table) = self.table_stack.last_mut() {
                    if let Some(prev_row) = table.current_row.take()
                        && !prev_row.cells.is_empty()
                    {
                        table.rows.push(prev_row);
                    }
                    table.current_row = Some(ClipboardTableRow {
                        is_header: false,
                        cells: Vec::new(),
                    });
                }
            }
            "th" | "td" => {
                self.flush_current_block();
                if let Some(table) = self.table_stack.last_mut() {
                    if table.current_row.is_none() {
                        table.current_row = Some(ClipboardTableRow {
                            is_header: name == "th",
                            cells: Vec::new(),
                        });
                    }
                    if name == "th"
                        && let Some(row) = table.current_row.as_mut()
                    {
                        row.is_header = true;
                    }
                    let colspan = attr_value(attrs, "colspan")
                        .and_then(|v| v.parse::<u16>().ok())
                        .unwrap_or(1)
                        .max(1);
                    let rowspan = attr_value(attrs, "rowspan")
                        .and_then(|v| v.parse::<u16>().ok())
                        .unwrap_or(1)
                        .max(1);
                    table.current_cell = Some((colspan, rowspan, Vec::new()));
                }
            }
            "hr" => {
                self.flush_current_block();
                self.push_block(ClipboardBlock::HorizontalRule);
            }
            "br" => {
                self.ensure_paragraph();
                let style = self.current_inline_style();
                if let Some(block) = self.current_block.as_mut() {
                    block.content.push_text("\n", style);
                }
            }
            "img" => {
                let asset_id = ["data-asset-id", "src"]
                    .into_iter()
                    .filter_map(|name| attr_value(attrs, name))
                    .find_map(|source| ImageAssetId::new(source).ok());
                let alt = attr_value(attrs, "alt").unwrap_or("");
                let width_pt = attr_value(attrs, "width")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(120.0)
                    .clamp(12.0, 640.0);
                let height_pt = attr_value(attrs, "height")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(32.0)
                    .clamp(12.0, 480.0);
                self.ensure_paragraph();
                let style = self.current_inline_style();
                if let Some(block) = self.current_block.as_mut() {
                    let Some(asset_id) = asset_id else {
                        block.content.push_text(alt, style);
                        return;
                    };
                    block.content.push_object(
                        InlineObject::Image {
                            asset_id,
                            alt: alt.to_owned(),
                            width_pt,
                            height_pt,
                            valign: InlineVAlign::Middle,
                        },
                        style,
                    );
                }
            }
            _ => {
                if !self_closing {
                    let overlay = inline_style_for_tag(name, attrs, css_style);
                    if overlay != InlineStyle::default() {
                        self.style_stack.push((name.to_owned(), overlay));
                    }
                }
            }
        }
    }

    fn handle_end_tag(&mut self, name: &str) {
        if self.skip_depth > 0 {
            self.skip_depth -= 1;
            return;
        }
        if self.skip_custom_inner_depth > 0 {
            self.skip_custom_inner_depth -= 1;
            return;
        }
        if name == "span"
            && let Some((kind, label)) = self.pending_chip.take()
        {
            self.ensure_paragraph();
            let style = self.current_inline_style();
            if let Some(block) = self.current_block.as_mut() {
                block.content.push_object(
                    InlineObject::SmartChip {
                        kind,
                        label,
                        accent: None,
                    },
                    style,
                );
            }
            return;
        }
        if name == "sup" && self.pending_footnote.take().is_some() {
            return;
        }

        match name {
            "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" => {
                self.flush_current_block();
            }
            "pre" => {
                self.flush_current_block();
                self.in_pre = false;
            }
            "ul" | "ol" => {
                self.flush_current_block();
                self.list_stack.pop();
            }
            "blockquote" => {
                self.flush_current_block();
                if let Some(children) = self.blockquote_stack.pop() {
                    self.push_block(ClipboardBlock::Blockquote(children));
                }
            }
            "td" | "th" => {
                self.flush_current_block();
                if let Some(table) = self.table_stack.last_mut()
                    && let Some((colspan, rowspan, blocks)) = table.current_cell.take()
                    && let Some(row) = table.current_row.as_mut()
                {
                    row.cells.push(ClipboardTableCell {
                        colspan,
                        rowspan,
                        blocks,
                    });
                }
            }
            "tr" => {
                self.flush_current_block();
                if let Some(table) = self.table_stack.last_mut() {
                    if let Some((colspan, rowspan, blocks)) = table.current_cell.take()
                        && let Some(row) = table.current_row.as_mut()
                    {
                        row.cells.push(ClipboardTableCell {
                            colspan,
                            rowspan,
                            blocks,
                        });
                    }
                    if let Some(row) = table.current_row.take()
                        && !row.cells.is_empty()
                    {
                        table.rows.push(row);
                    }
                }
            }
            "table" => {
                self.flush_current_block();
                if let Some(table_state) = self.table_stack.pop() {
                    let table = finish_table_state(table_state);
                    if !table.rows.is_empty() {
                        self.push_block(ClipboardBlock::Table(table));
                    }
                }
            }
            _ => {
                if let Some(position) = self
                    .style_stack
                    .iter()
                    .rposition(|(tag_name, _)| tag_name == name)
                {
                    self.style_stack.remove(position);
                }
            }
        }
    }

    fn handle_text(&mut self, raw_text: &str) {
        if self.skip_depth > 0
            || self.skip_custom_inner_depth > 0
            || self.pending_footnote.is_some()
        {
            return;
        }
        if let Some((_, label)) = self.pending_chip.as_mut() {
            label.push_str(raw_text);
            return;
        }

        let normalized = if self.in_pre {
            raw_text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            collapse_html_whitespace(raw_text)
        };

        if normalized.is_empty() {
            return;
        }
        if !self.in_pre && normalized == " " && self.current_block.is_none() {
            return;
        }
        if !self.in_pre
            && normalized == " "
            && self
                .current_block
                .as_ref()
                .is_some_and(|block| block.content.text.ends_with(' ') || block.content.is_empty())
        {
            return;
        }

        self.ensure_paragraph();
        let style = if self.in_pre {
            InlineStyle::default()
        } else {
            self.current_inline_style()
        };
        if let Some(block) = self.current_block.as_mut() {
            let trimmed = if !self.in_pre && block.content.is_empty() {
                normalized.trim_start_matches(' ')
            } else {
                normalized.as_str()
            };
            if !trimmed.is_empty() {
                block.content.push_text(trimmed, style);
            }
        }
    }
}

fn finish_table_state(mut state: TableBuildState) -> ClipboardTable {
    if let Some((colspan, rowspan, blocks)) = state.current_cell.take()
        && let Some(row) = state.current_row.as_mut()
    {
        row.cells.push(ClipboardTableCell {
            colspan,
            rowspan,
            blocks,
        });
    }
    if let Some(row) = state.current_row.take()
        && !row.cells.is_empty()
    {
        state.rows.push(row);
    }
    ClipboardTable {
        rows: state.rows,
        column_kinds: state.column_kinds,
    }
}

fn inline_style_for_tag(name: &str, attrs: &[(String, String)], css_style: &str) -> InlineStyle {
    let mut style = InlineStyle::default();
    match name {
        "b" | "strong" => style.weight = Some(FontWeight::BOLD),
        "i" | "em" => style.italic = Some(true),
        "u" => style.underline = Some(UnderlineKind::Solid),
        "s" | "del" | "strike" => style.strikethrough = Some(true),
        "code" => style.inline_code = Some(true),
        "sup" => style.baseline_shift = Some(BaselineShift::Superscript),
        "sub" => style.baseline_shift = Some(BaselineShift::Subscript),
        "a" => {
            if let Some(href) = attr_value(attrs, "href")
                && is_safe_href(href)
            {
                style.link = Some(LinkTarget {
                    href: href.to_owned(),
                    title: attr_value(attrs, "title").map(ToOwned::to_owned),
                });
            }
        }
        _ => {}
    }

    if !css_style.is_empty() {
        apply_css_inline_style(css_style, &mut style);
    }
    style
}

fn apply_css_inline_style(css: &str, style: &mut InlineStyle) {
    for declaration in css.split(';') {
        let Some((raw_prop, raw_val)) = declaration.split_once(':') else {
            continue;
        };
        let prop = raw_prop.trim().to_ascii_lowercase();
        let value = raw_val.trim();
        let value_lower = value.to_ascii_lowercase();
        match prop.as_str() {
            "font-weight" => {
                if value_lower == "normal" || value_lower == "lighter" {
                    style.weight = Some(FontWeight(400));
                } else if value_lower == "bold" || value_lower == "bolder" {
                    style.weight = Some(FontWeight::BOLD);
                } else if let Ok(numeric) = value_lower.parse::<u16>() {
                    style.weight = Some(FontWeight(numeric.clamp(1, 1000)));
                }
            }
            "font-style" => {
                style.italic = Some(value_lower == "italic" || value_lower == "oblique");
            }
            "text-decoration" | "text-decoration-line" => {
                if value_lower.contains("underline") {
                    style.underline = Some(UnderlineKind::Solid);
                }
                style.strikethrough = Some(value_lower.contains("line-through"));
            }
            "vertical-align" => {
                if value_lower == "super" {
                    style.baseline_shift = Some(BaselineShift::Superscript);
                } else if value_lower == "sub" {
                    style.baseline_shift = Some(BaselineShift::Subscript);
                } else if value_lower == "baseline" {
                    style.baseline_shift = Some(BaselineShift::Normal);
                }
            }
            "color" => {
                if let Some(color) = parse_css_color(&value_lower) {
                    style.color = Some(color);
                }
            }
            "background-color" | "background" => {
                if let Some(color) = parse_css_color(&value_lower) {
                    style.background = Some(color);
                }
            }
            "font-family" => {
                if let Some(primary) = value
                    .split(',')
                    .next()
                    .map(|entry| entry.trim().trim_matches(|c| c == '\'' || c == '"').trim())
                    .filter(|entry| !entry.is_empty())
                {
                    style.font_family = Some(std::sync::Arc::from(primary));
                }
            }
            "font-size" => {
                if let Some(pt_str) = value_lower.strip_suffix("pt")
                    && let Ok(pt) = pt_str.trim().parse::<f32>()
                    && (6.0..=96.0).contains(&pt)
                {
                    style.font_size_pt = Some(pt);
                } else if let Some(px_str) = value_lower.strip_suffix("px")
                    && let Ok(px_val) = px_str.trim().parse::<f32>()
                {
                    let pt = px_val * 0.75;
                    if (6.0..=96.0).contains(&pt) {
                        style.font_size_pt = Some(pt);
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_css_text_align(css: &str) -> Option<TextAlign> {
    for declaration in css.split(';') {
        let Some((raw_prop, raw_val)) = declaration.split_once(':') else {
            continue;
        };
        if raw_prop.trim().eq_ignore_ascii_case("text-align") {
            return match raw_val.trim().to_ascii_lowercase().as_str() {
                "center" => Some(TextAlign::Center),
                "right" | "end" => Some(TextAlign::Right),
                "justify" => Some(TextAlign::Justify),
                "left" | "start" => Some(TextAlign::Left),
                _ => None,
            };
        }
    }
    None
}

fn parse_css_color(value: &str) -> Option<RgbaColor> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
            let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
            let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
            return Some(RgbaColor::rgb(r, g, b));
        }
        if hex.len() == 3 {
            let r = u8::from_str_radix(hex.get(0..1)?, 16).ok()? * 17;
            let g = u8::from_str_radix(hex.get(1..2)?, 16).ok()? * 17;
            let b = u8::from_str_radix(hex.get(2..3)?, 16).ok()? * 17;
            return Some(RgbaColor::rgb(r, g, b));
        }
        return None;
    }
    if let Some(inner) = trimmed
        .strip_prefix("rgb(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut parts = inner.split(',').map(str::trim);
        let r = parts.next()?.parse::<u8>().ok()?;
        let g = parts.next()?.parse::<u8>().ok()?;
        let b = parts.next()?.parse::<u8>().ok()?;
        return Some(RgbaColor::rgb(r, g, b));
    }
    if let Some(inner) = trimmed
        .strip_prefix("rgba(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut parts = inner.split(',').map(str::trim);
        let r = parts.next()?.parse::<u8>().ok()?;
        let g = parts.next()?.parse::<u8>().ok()?;
        let b = parts.next()?.parse::<u8>().ok()?;
        let alpha = parts.next()?.parse::<f32>().ok()?.clamp(0.0, 1.0);
        return Some(RgbaColor::rgba(r, g, b, (alpha * 255.0).round() as u8));
    }
    None
}

fn collapse_html_whitespace(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut previous_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !previous_space {
                output.push(' ');
                previous_space = true;
            }
        } else {
            output.push(character);
            previous_space = false;
        }
    }
    output
}

fn is_safe_href(href: &str) -> bool {
    let trimmed = href.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("doc://")
        || lower.starts_with('#')
        || lower.starts_with('/')
    {
        return true;
    }
    !lower.contains(':')
}
