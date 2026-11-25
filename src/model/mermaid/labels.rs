//! Pure-Rust SVG label extraction and Mermaid source label rewriting for direct inline editing.

use std::collections::HashMap;
use std::ops::Range;

/// Root SVG coordinate space (`viewBox="x y width height"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgViewBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SvgViewBox {
    /// Creates a `viewBox` with positive finite dimensions, falling back to `1.0` when needed.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x: if x.is_finite() { x } else { 0.0 },
            y: if y.is_finite() { y } else { 0.0 },
            width: if width.is_finite() && width > 0.0 {
                width
            } else {
                1.0
            },
            height: if height.is_finite() && height > 0.0 {
                height
            } else {
                1.0
            },
        }
    }
}

/// Text label located inside a compiled Mermaid SVG document.
#[derive(Clone, Debug, PartialEq)]
pub struct MermaidSvgLabel {
    /// Plain display text extracted from the SVG label.
    pub text: String,
    /// Zero-based occurrence index among labels sharing the same normalized text.
    pub occurrence: usize,
    /// Bounding box X coordinate in root SVG `viewBox` space.
    pub x: f32,
    /// Bounding box Y coordinate in root SVG `viewBox` space.
    pub y: f32,
    /// Bounding box width in root SVG `viewBox` space.
    pub width: f32,
    /// Bounding box height in root SVG `viewBox` space.
    pub height: f32,
}

/// Editable source location matched to a rendered Mermaid diagram label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidLabelMatch {
    /// Byte range in the original Mermaid source string that will be replaced.
    pub byte_range: Range<usize>,
    /// Unescaped display text used to seed an inline label editor.
    pub display_text: String,
    kind: LabelSpanKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LabelSpanKind {
    UnquotedBracket,
    QuotedString,
    UnquotedPipe,
    InlineArrow,
    PlainLineTail,
    BareFlowchartNode,
    BareSequenceParticipant,
}

#[derive(Clone, Debug)]
struct CandidateSpan {
    byte_range: Range<usize>,
    display_text: String,
    kind: LabelSpanKind,
    priority: u8,
}

/// Parses the root `<svg>` `viewBox` `(x, y, width, height)`, falling back to `width` and `height`
/// attributes at `(0, 0)` when `viewBox` is absent.
pub fn parse_svg_view_box(svg: &str) -> Option<SvgViewBox> {
    let open_start = find_svg_open_tag(svg)?;
    let tail = svg.get(open_start..)?;
    let open_end = tail.find('>')?;
    let header = tail.get(..open_end)?;

    if let Some(view_box) = extract_xml_attribute(header, "viewBox")
        && let Some(parsed) = parse_view_box_rect(view_box)
    {
        return Some(parsed);
    }

    let width = extract_xml_attribute(header, "width").and_then(parse_svg_length_px)?;
    let height = extract_xml_attribute(header, "height").and_then(parse_svg_length_px)?;
    Some(SvgViewBox::new(0.0, 0.0, width, height))
}

/// Extracts positioned text labels from a compiled Mermaid SVG document in `viewBox` coordinates.
pub fn extract_svg_labels(
    svg: &str,
    fallback_width: f32,
    fallback_height: f32,
) -> (SvgViewBox, Vec<MermaidSvgLabel>) {
    let view_box = parse_svg_view_box(svg)
        .unwrap_or_else(|| SvgViewBox::new(0.0, 0.0, fallback_width, fallback_height));

    enum RawLabelItem {
        Direct(MermaidSvgLabel),
        UseRef {
            href_id: String,
            use_x: f32,
            use_y: f32,
            transform: Affine2D,
        },
    }

    let mut raw_items: Vec<RawLabelItem> = Vec::new();
    let mut defs_templates: HashMap<String, DefsLabelTemplate> = HashMap::new();
    let mut stack: Vec<ElementFrame> = Vec::new();
    let mut active_fallback: Option<FallbackCollector> = None;
    let mut active_defs_group: Option<DefsGroupCollector> = None;
    let mut active_text: Option<NativeTextCollector> = None;

    let bytes = svg.as_bytes();
    let mut cursor = 0usize;
    let mut root_svg_seen = false;

    while cursor < bytes.len() {
        let Some(rel_lt) = svg.get(cursor..).and_then(|slice| slice.find('<')) else {
            break;
        };
        let lt = cursor + rel_lt;

        if lt > cursor
            && let Some(chunk) = svg.get(cursor..lt)
        {
            if let Some(collector) = active_defs_group.as_mut() {
                if collector.in_text_depth > 0 {
                    collector.current_line.push_str(&decode_xml_entities(chunk));
                }
            } else if let Some(collector) = active_fallback.as_mut() {
                if collector.in_text_depth > 0 {
                    collector.current_line.push_str(&decode_xml_entities(chunk));
                }
            } else if let Some(collector) = active_text.as_mut() {
                collector.push_chunk(&decode_xml_entities(chunk));
            }
        }

        let rest = svg.get(lt..).unwrap_or_default();
        if rest.starts_with("<!--") {
            if let Some(end) = rest.find("-->") {
                cursor = lt + end + 3;
                continue;
            }
            break;
        }
        if rest.starts_with("<![CDATA[") {
            if let Some(end) = rest.find("]]>") {
                let cdata = rest.get(9..end).unwrap_or_default();
                if let Some(collector) = active_defs_group.as_mut() {
                    if collector.in_text_depth > 0 {
                        collector.current_line.push_str(cdata);
                    }
                } else if let Some(collector) = active_fallback.as_mut() {
                    if collector.in_text_depth > 0 {
                        collector.current_line.push_str(cdata);
                    }
                } else if let Some(collector) = active_text.as_mut() {
                    collector.push_chunk(cdata);
                }
                cursor = lt + end + 3;
                continue;
            }
            break;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            if let Some(end) = find_tag_end(svg, lt) {
                cursor = end + 1;
                continue;
            }
            break;
        }

        let Some(tag_end) = find_tag_end(svg, lt) else {
            break;
        };
        let raw_tag = svg.get(lt..=tag_end).unwrap_or_default();
        cursor = tag_end + 1;

        if raw_tag.starts_with("</") {
            let closing_name = parse_tag_name(raw_tag.get(2..).unwrap_or_default());
            if let Some(collector) = active_defs_group.as_mut()
                && closing_name.eq_ignore_ascii_case("text")
                && collector.in_text_depth > 0
            {
                collector.in_text_depth -= 1;
                if collector.in_text_depth == 0 {
                    let line = std::mem::take(&mut collector.current_line);
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        collector.lines.push(trimmed.to_owned());
                    }
                }
            } else if let Some(collector) = active_fallback.as_mut()
                && closing_name.eq_ignore_ascii_case("text")
                && collector.in_text_depth > 0
            {
                collector.in_text_depth -= 1;
                if collector.in_text_depth == 0 {
                    let line = std::mem::take(&mut collector.current_line);
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        collector.lines.push(trimmed.to_owned());
                    }
                }
            }

            if let Some(collector) = active_text.as_mut()
                && closing_name.eq_ignore_ascii_case("tspan")
            {
                collector.finish_tspan();
            }

            let popped = pop_matching_frame(&mut stack, closing_name);
            if let Some(frame) = popped {
                if frame.is_defs_label_group
                    && let Some(collector) = active_defs_group.take()
                    && let Some((id, template)) = collector.finish()
                {
                    defs_templates.insert(id, template);
                }
                if frame.is_fallback_group
                    && let Some(collector) = active_fallback.take()
                    && let Some(label) = collector.finish()
                {
                    raw_items.push(RawLabelItem::Direct(label));
                }
                if frame.is_native_text
                    && let Some(collector) = active_text.take()
                    && let Some(label) = collector.finish()
                {
                    raw_items.push(RawLabelItem::Direct(label));
                }
            }
            continue;
        }

        let self_closing = raw_tag.ends_with("/>");
        let inner_header = raw_tag
            .get(
                1..raw_tag
                    .len()
                    .saturating_sub(if self_closing { 2 } else { 1 }),
            )
            .unwrap_or_default();
        let tag_name = parse_tag_name(inner_header);
        if tag_name.is_empty() {
            continue;
        }

        if tag_name.eq_ignore_ascii_case("style") && !self_closing {
            if let Some(close_rel) =
                find_ascii_case_insensitive(svg.get(cursor..).unwrap_or_default(), "</style>")
            {
                cursor += close_rel + "</style>".len();
            }
            continue;
        }

        let parent_transform = stack
            .last()
            .map(|frame| frame.transform)
            .unwrap_or(Affine2D::IDENTITY);
        let parent_font_size = stack.last().map(|frame| frame.font_size).unwrap_or(16.0);
        let parent_skip = stack.last().is_some_and(|frame| frame.skip_subtree);
        let parent_in_defs = stack.last().is_some_and(|frame| frame.in_defs);

        let in_defs = parent_in_defs || tag_name.eq_ignore_ascii_case("defs");
        let is_skipped_tag = matches!(
            tag_name.to_ascii_lowercase().as_str(),
            "defs" | "style" | "title" | "desc" | "metadata" | "clippath" | "mask" | "marker"
        );
        let skip_subtree = parent_skip || is_skipped_tag;

        let local_transform = parse_element_transform(inner_header, tag_name, root_svg_seen);
        let combined_transform = parent_transform.multiply(local_transform);
        let font_size = parse_element_font_size(inner_header).unwrap_or(parent_font_size);

        if tag_name.eq_ignore_ascii_case("svg") {
            if !root_svg_seen {
                root_svg_seen = true;
            } else if let Some(collector) = active_defs_group.as_mut()
                && collector.rect.is_none()
            {
                let x = extract_xml_attribute(inner_header, "x")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                let y = extract_xml_attribute(inner_header, "y")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                let width = extract_xml_attribute(inner_header, "width")
                    .and_then(parse_svg_length_px)
                    .unwrap_or(0.0);
                let height = extract_xml_attribute(inner_header, "height")
                    .and_then(parse_svg_length_px)
                    .unwrap_or(0.0);
                if width > 0.0 && height > 0.0 {
                    collector.rect = Some((x, y, width, height));
                }
            } else if let Some(collector) = active_fallback.as_mut()
                && collector.rect.is_none()
            {
                let x = extract_xml_attribute(inner_header, "x")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                let y = extract_xml_attribute(inner_header, "y")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                let width = extract_xml_attribute(inner_header, "width")
                    .and_then(parse_svg_length_px)
                    .unwrap_or(0.0);
                let height = extract_xml_attribute(inner_header, "height")
                    .and_then(parse_svg_length_px)
                    .unwrap_or(0.0);
                if width > 0.0 && height > 0.0 {
                    collector.rect = Some(parent_transform.transform_rect(x, y, width, height));
                }
            }
        }

        // Capture isolated label templates emitted by merman inside `<defs><g id="...">`.
        let is_defs_label_group = in_defs
            && !self_closing
            && active_defs_group.is_none()
            && tag_name.eq_ignore_ascii_case("g")
            && extract_xml_attribute(inner_header, "id").is_some();

        if is_defs_label_group && let Some(id) = extract_xml_attribute(inner_header, "id") {
            active_defs_group = Some(DefsGroupCollector {
                id: id.to_owned(),
                rect: None,
                lines: Vec::new(),
                current_line: String::new(),
                in_text_depth: 0,
            });
        } else if let Some(collector) = active_defs_group.as_mut()
            && !self_closing
            && tag_name.eq_ignore_ascii_case("text")
        {
            collector.in_text_depth += 1;
        }

        if !skip_subtree && tag_name.eq_ignore_ascii_case("use") {
            let href = extract_xml_attribute(inner_header, "href")
                .or_else(|| extract_xml_attribute(inner_header, "xlink:href"));
            if let Some(target_id) = href.and_then(|value| value.strip_prefix('#')) {
                let use_x = extract_xml_attribute(inner_header, "x")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                let use_y = extract_xml_attribute(inner_header, "y")
                    .and_then(parse_svg_number)
                    .unwrap_or(0.0);
                raw_items.push(RawLabelItem::UseRef {
                    href_id: target_id.to_owned(),
                    use_x,
                    use_y,
                    transform: combined_transform,
                });
            }
        }

        let is_fallback_group = !skip_subtree
            && !self_closing
            && active_fallback.is_none()
            && tag_name.eq_ignore_ascii_case("g")
            && extract_xml_attribute(inner_header, "data-merman-foreignobject")
                .is_some_and(|value| value.eq_ignore_ascii_case("fallback"));

        if is_fallback_group {
            active_fallback = Some(FallbackCollector::default());
        } else if let Some(collector) = active_fallback.as_mut()
            && !self_closing
            && tag_name.eq_ignore_ascii_case("text")
        {
            collector.in_text_depth += 1;
        }

        let is_native_text = !skip_subtree
            && !self_closing
            && active_fallback.is_none()
            && active_text.is_none()
            && tag_name.eq_ignore_ascii_case("text");

        if is_native_text {
            let x = extract_xml_attribute(inner_header, "x")
                .and_then(parse_svg_number)
                .unwrap_or(0.0);
            let y = extract_xml_attribute(inner_header, "y")
                .and_then(parse_svg_number)
                .unwrap_or(0.0);
            let anchor = extract_style_or_attr(inner_header, "text-anchor")
                .map(|value| TextAnchor::parse(&value))
                .unwrap_or(TextAnchor::Start);
            let baseline = extract_style_or_attr(inner_header, "dominant-baseline")
                .or_else(|| extract_style_or_attr(inner_header, "alignment-baseline"))
                .map(|value| DominantBaseline::parse(&value))
                .unwrap_or(DominantBaseline::Alphabetic);
            active_text = Some(NativeTextCollector::new(
                x,
                y,
                font_size,
                anchor,
                baseline,
                combined_transform,
            ));
        } else if let Some(collector) = active_text.as_mut()
            && tag_name.eq_ignore_ascii_case("tspan")
        {
            let tspan_x = extract_xml_attribute(inner_header, "x").and_then(parse_svg_number);
            let tspan_y = extract_xml_attribute(inner_header, "y").and_then(parse_svg_number);
            let tspan_dy = extract_xml_attribute(inner_header, "dy").and_then(parse_svg_dy);
            collector.start_tspan(tspan_x, tspan_y, tspan_dy, self_closing);
        }

        if !self_closing {
            stack.push(ElementFrame {
                tag_name: tag_name.to_owned(),
                transform: combined_transform,
                font_size,
                skip_subtree,
                in_defs,
                is_defs_label_group,
                is_fallback_group,
                is_native_text,
            });
        }
    }

    let mut labels = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        match item {
            RawLabelItem::Direct(label) => labels.push(label),
            RawLabelItem::UseRef {
                href_id,
                use_x,
                use_y,
                transform,
            } => {
                if let Some(template) = defs_templates.get(&href_id) {
                    let (x, y, width, height) = transform.transform_rect(
                        use_x + template.x,
                        use_y + template.y,
                        template.width,
                        template.height,
                    );
                    if x.is_finite()
                        && y.is_finite()
                        && width.is_finite()
                        && height.is_finite()
                        && width > 0.0
                        && height > 0.0
                    {
                        labels.push(MermaidSvgLabel {
                            text: template.text.clone(),
                            occurrence: 0,
                            x,
                            y,
                            width,
                            height,
                        });
                    }
                }
            }
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for label in &mut labels {
        let normalized = normalize_label_text(&label.text);
        let entry = counts.entry(normalized).or_insert(0);
        label.occurrence = *entry;
        *entry += 1;
    }

    (view_box, labels)
}

/// Normalizes label text for matching between rendered SVG lines and Mermaid source labels.
pub fn normalize_label_text(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut index = 0usize;
    let bytes = raw.as_bytes();
    while index < bytes.len() {
        let rest = raw.get(index..).unwrap_or_default();
        if let Some(br_len) = match_br_tag(rest) {
            cleaned.push(' ');
            index += br_len;
            continue;
        }
        if let Some(ch) = rest.chars().next() {
            if ch.is_whitespace() {
                cleaned.push(' ');
            } else {
                cleaned.push(ch);
            }
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Locates the editable span in `source` corresponding to `label_text` at `occurrence`.
pub fn find_editable_mermaid_label(
    source: &str,
    label_text: &str,
    occurrence: usize,
) -> Option<MermaidLabelMatch> {
    let target = normalize_label_text(label_text);
    if target.is_empty() {
        return None;
    }

    let candidates = collect_source_label_candidates(source);
    for priority in [1u8, 2u8] {
        let matched: Vec<&CandidateSpan> = candidates
            .iter()
            .filter(|candidate| {
                candidate.priority == priority
                    && normalize_label_text(&candidate.display_text) == target
            })
            .collect();
        if !matched.is_empty() {
            let chosen = matched.get(occurrence).or_else(|| matched.last())?;
            return Some(MermaidLabelMatch {
                byte_range: chosen.byte_range.clone(),
                display_text: chosen.display_text.clone(),
                kind: chosen.kind,
            });
        }
    }

    None
}

/// Replaces the label matching `(label_text, occurrence)` inside `source` with `new_text`.
///
/// Returns `None` when the label cannot be located safely or `new_text` is empty or unchanged.
pub fn replace_mermaid_label(
    source: &str,
    label_text: &str,
    occurrence: usize,
    new_text: &str,
) -> Option<String> {
    let trimmed = new_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let matched = find_editable_mermaid_label(source, label_text, occurrence)?;
    if matched.display_text.trim() == trimmed {
        return None;
    }
    let replacement = format_replacement(matched.kind, &matched.display_text, trimmed);
    let prefix = source.get(..matched.byte_range.start)?;
    let suffix = source.get(matched.byte_range.end..)?;
    let mut updated = String::with_capacity(prefix.len() + replacement.len() + suffix.len());
    updated.push_str(prefix);
    updated.push_str(&replacement);
    updated.push_str(suffix);
    Some(updated)
}

fn format_replacement(kind: LabelSpanKind, original_display: &str, new_text: &str) -> String {
    let inline_multiline = new_text.replace("\r\n", "\n").replace('\n', "<br/>");
    match kind {
        LabelSpanKind::QuotedString => escape_mermaid_quoted(&inline_multiline),
        LabelSpanKind::UnquotedBracket => {
            if needs_flowchart_quoting(&inline_multiline) {
                format!("\"{}\"", escape_mermaid_quoted(&inline_multiline))
            } else {
                inline_multiline
            }
        }
        LabelSpanKind::UnquotedPipe => {
            if needs_pipe_quoting(&inline_multiline) {
                format!("\"{}\"", escape_mermaid_quoted(&inline_multiline))
            } else {
                inline_multiline
            }
        }
        LabelSpanKind::InlineArrow => inline_multiline
            .replace("-->", "->")
            .replace("---", "--")
            .replace("==>", "=>")
            .replace(".->", "->"),
        LabelSpanKind::PlainLineTail => inline_multiline,
        LabelSpanKind::BareFlowchartNode => {
            let formatted = if needs_flowchart_quoting(&inline_multiline) {
                format!("\"{}\"", escape_mermaid_quoted(&inline_multiline))
            } else {
                inline_multiline
            };
            format!("{original_display}[{formatted}]")
        }
        LabelSpanKind::BareSequenceParticipant => {
            format!("{original_display} as {inline_multiline}")
        }
    }
}

fn escape_mermaid_quoted(raw: &str) -> String {
    raw.replace('"', "#quot;")
}

fn needs_flowchart_quoting(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(
            ch,
            '[' | ']' | '(' | ')' | '{' | '}' | '|' | '"' | '<' | '>' | ';' | '#'
        )
    }) || text.contains("-->")
        || text.contains("---")
}

fn needs_pipe_quoting(text: &str) -> bool {
    text.chars().any(|ch| matches!(ch, '|' | '"' | '\n' | '\r'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagramFamily {
    Flowchart,
    Sequence,
    State,
    Other,
}

fn detect_diagram_family(source: &str) -> DiagramFamily {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        let first = trimmed.split_whitespace().next().unwrap_or_default();
        if first.eq_ignore_ascii_case("flowchart") || first.eq_ignore_ascii_case("graph") {
            return DiagramFamily::Flowchart;
        }
        if first.eq_ignore_ascii_case("sequencediagram") {
            return DiagramFamily::Sequence;
        }
        if first.eq_ignore_ascii_case("statediagram")
            || first.eq_ignore_ascii_case("statediagram-v2")
        {
            return DiagramFamily::State;
        }
        return DiagramFamily::Other;
    }
    DiagramFamily::Other
}

fn collect_source_label_candidates(source: &str) -> Vec<CandidateSpan> {
    let family = detect_diagram_family(source);
    let mut candidates = Vec::new();
    let mut line_start = 0usize;
    let mut saw_header = false;

    for raw_line in source.split_inclusive('\n') {
        let line_no_nl = raw_line.trim_end_matches(['\r', '\n']);
        let leading_ws = line_no_nl.len() - line_no_nl.trim_start().len();
        let trimmed = line_no_nl.trim();
        let trimmed_start = line_start + leading_ws;

        if trimmed.is_empty() || trimmed.starts_with("%%") {
            line_start += raw_line.len();
            continue;
        }

        if !saw_header {
            saw_header = true;
            if is_diagram_header_line(trimmed) {
                line_start += raw_line.len();
                continue;
            }
        }

        scan_line_candidates(trimmed, trimmed_start, family, &mut candidates);
        line_start += raw_line.len();
    }

    candidates
}

fn is_diagram_header_line(trimmed: &str) -> bool {
    let first = trimmed.split_whitespace().next().unwrap_or_default();
    matches!(
        first.to_ascii_lowercase().as_str(),
        "flowchart"
            | "graph"
            | "sequencediagram"
            | "statediagram"
            | "statediagram-v2"
            | "classdiagram"
            | "erdiagram"
            | "pie"
            | "gantt"
            | "journey"
            | "gitgraph"
            | "mindmap"
            | "timeline"
            | "quadrantchart"
            | "requirementdiagram"
            | "c4context"
            | "c4container"
            | "c4component"
            | "sankey-beta"
            | "xychart-beta"
            | "block-beta"
    )
}

fn scan_line_candidates(
    trimmed: &str,
    base_offset: usize,
    family: DiagramFamily,
    candidates: &mut Vec<CandidateSpan>,
) {
    let first_word = trimmed.split_whitespace().next().unwrap_or_default();
    let lower_first = first_word.to_ascii_lowercase();

    // Skip non-label styling, class, and link directives.
    if matches!(
        lower_first.as_str(),
        "style" | "classdef" | "linkstyle" | "click" | "direction" | "end" | "autonumber"
    ) {
        return;
    }

    if lower_first == "title" || lower_first == "section" {
        if let Some((rel_start, rel_end)) = tail_after_keyword(trimmed, first_word.len()) {
            push_plain_candidate(
                trimmed,
                base_offset,
                rel_start,
                rel_end,
                LabelSpanKind::PlainLineTail,
                candidates,
            );
        }
        return;
    }

    if lower_first == "subgraph" {
        let rest = trimmed.get(first_word.len()..).unwrap_or_default();
        if rest.contains('[') {
            scan_flowchart_line(trimmed, base_offset, candidates);
        } else if let Some((rel_start, rel_end)) = tail_after_keyword(trimmed, first_word.len()) {
            push_maybe_quoted_candidate(
                trimmed,
                base_offset,
                rel_start,
                rel_end,
                LabelSpanKind::PlainLineTail,
                candidates,
            );
        }
        return;
    }

    match family {
        DiagramFamily::Sequence => {
            scan_sequence_line(trimmed, base_offset, &lower_first, candidates);
        }
        DiagramFamily::Flowchart => {
            scan_flowchart_line(trimmed, base_offset, candidates);
        }
        DiagramFamily::State => {
            scan_state_line(trimmed, base_offset, &lower_first, candidates);
        }
        DiagramFamily::Other => {
            scan_generic_line(trimmed, base_offset, candidates);
        }
    }
}

fn scan_sequence_line(
    trimmed: &str,
    base_offset: usize,
    lower_first: &str,
    candidates: &mut Vec<CandidateSpan>,
) {
    if lower_first == "participant" || lower_first == "actor" {
        let after_kw = trimmed
            .get(lower_first.len()..)
            .unwrap_or_default()
            .trim_start();
        let after_kw_offset = trimmed.len() - after_kw.len();
        if let Some(as_pos) = find_ascii_word_separator(after_kw, "as") {
            let alias_part = after_kw.get(as_pos + 4..).unwrap_or_default();
            let lead = alias_part.len() - alias_part.trim_start().len();
            let alias = alias_part.trim();
            if !alias.is_empty() {
                let start = after_kw_offset + as_pos + 4 + lead;
                let end = start + alias.len();
                candidates.push(CandidateSpan {
                    byte_range: (base_offset + start)..(base_offset + end),
                    display_text: alias.to_owned(),
                    kind: LabelSpanKind::PlainLineTail,
                    priority: 1,
                });
            }
        } else if !after_kw.is_empty() {
            candidates.push(CandidateSpan {
                byte_range: (base_offset + after_kw_offset)
                    ..(base_offset + after_kw_offset + after_kw.len()),
                display_text: after_kw.to_owned(),
                kind: LabelSpanKind::BareSequenceParticipant,
                priority: 2,
            });
        }
        return;
    }

    if matches!(
        lower_first,
        "loop" | "alt" | "else" | "opt" | "par" | "and" | "critical" | "option" | "break"
    ) {
        if let Some((rel_start, rel_end)) = tail_after_keyword(trimmed, lower_first.len()) {
            push_plain_candidate(
                trimmed,
                base_offset,
                rel_start,
                rel_end,
                LabelSpanKind::PlainLineTail,
                candidates,
            );
        }
        return;
    }

    if let Some(colon_index) = find_unquoted_char(trimmed, ':') {
        let after_colon = trimmed.get(colon_index + 1..).unwrap_or_default();
        let lead = after_colon.len() - after_colon.trim_start().len();
        let label = after_colon.trim();
        if !label.is_empty() {
            let start = colon_index + 1 + lead;
            let end = start + label.len();
            candidates.push(CandidateSpan {
                byte_range: (base_offset + start)..(base_offset + end),
                display_text: label.to_owned(),
                kind: LabelSpanKind::PlainLineTail,
                priority: 1,
            });
        }
    }
}

fn scan_state_line(
    trimmed: &str,
    base_offset: usize,
    lower_first: &str,
    candidates: &mut Vec<CandidateSpan>,
) {
    if lower_first == "state" {
        scan_quoted_strings(trimmed, base_offset, candidates);
        return;
    }
    if let Some(colon_index) = find_unquoted_char(trimmed, ':') {
        let after_colon = trimmed.get(colon_index + 1..).unwrap_or_default();
        let lead = after_colon.len() - after_colon.trim_start().len();
        let label = after_colon.trim();
        if !label.is_empty() {
            let start = colon_index + 1 + lead;
            let end = start + label.len();
            candidates.push(CandidateSpan {
                byte_range: (base_offset + start)..(base_offset + end),
                display_text: label.to_owned(),
                kind: LabelSpanKind::PlainLineTail,
                priority: 1,
            });
        }
    }
}

fn scan_generic_line(trimmed: &str, base_offset: usize, candidates: &mut Vec<CandidateSpan>) {
    scan_quoted_strings(trimmed, base_offset, candidates);
    scan_bracket_labels(trimmed, base_offset, candidates);
    if let Some(colon_index) = find_unquoted_char(trimmed, ':') {
        let after_colon = trimmed.get(colon_index + 1..).unwrap_or_default();
        let lead = after_colon.len() - after_colon.trim_start().len();
        let label = after_colon.trim();
        if !label.is_empty() && !label.starts_with('"') {
            let start = colon_index + 1 + lead;
            let end = start + label.len();
            candidates.push(CandidateSpan {
                byte_range: (base_offset + start)..(base_offset + end),
                display_text: label.to_owned(),
                kind: LabelSpanKind::PlainLineTail,
                priority: 1,
            });
        }
    }
}

fn scan_flowchart_line(trimmed: &str, base_offset: usize, candidates: &mut Vec<CandidateSpan>) {
    scan_bracket_labels(trimmed, base_offset, candidates);
    scan_pipe_edge_labels(trimmed, base_offset, candidates);
    scan_inline_arrow_labels(trimmed, base_offset, candidates);
    scan_bare_flowchart_nodes(trimmed, base_offset, candidates);
}

fn scan_quoted_strings(trimmed: &str, base_offset: usize, candidates: &mut Vec<CandidateSpan>) {
    let bytes = trimmed.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'"') {
            let content_start = index + 1;
            let mut cursor = content_start;
            while cursor < bytes.len() {
                if bytes.get(cursor) == Some(&b'\\') {
                    cursor = (cursor + 2).min(bytes.len());
                    continue;
                }
                if bytes.get(cursor) == Some(&b'"') {
                    break;
                }
                cursor += 1;
            }
            if cursor < bytes.len()
                && let Some(content) = trimmed.get(content_start..cursor)
                && !content.trim().is_empty()
            {
                candidates.push(CandidateSpan {
                    byte_range: (base_offset + content_start)..(base_offset + cursor),
                    display_text: unescape_mermaid_label(content),
                    kind: LabelSpanKind::QuotedString,
                    priority: 1,
                });
            }
            index = cursor + 1;
        } else {
            index += 1;
        }
    }
}

fn scan_bracket_labels(trimmed: &str, base_offset: usize, candidates: &mut Vec<CandidateSpan>) {
    const DELIMITERS: [(&str, &str); 14] = [
        ("(((", ")))"),
        ("((", "))"),
        ("([", "])"),
        ("[[", "]]"),
        ("[(", ")]"),
        ("{{", "}}"),
        ("[/", "/]"),
        ("[/", "\\]"),
        ("[\\", "\\]"),
        ("[\\", "/]"),
        ("[", "]"),
        ("(", ")"),
        ("{", "}"),
        (">", "]"),
    ];

    let bytes = trimmed.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'"') {
            index = skip_quoted_literal(bytes, index);
            continue;
        }

        // Only match node shape delimiters when immediately preceded by an alphanumeric or `_`
        // node-id character (so arrows such as `-->` or `==>` never match the `>...]` flag shape).
        let preceded_by_ident = index > 0
            && bytes
                .get(index - 1)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
        if !preceded_by_ident {
            index += 1;
            continue;
        }

        let rest = trimmed.get(index..).unwrap_or_default();
        let mut matched_pair: Option<(&str, &str, usize)> = None;
        for (open, close) in DELIMITERS {
            if rest.starts_with(open) {
                let inner_start = index + open.len();
                if let Some(close_pos) = find_closing_delimiter(trimmed, inner_start, close) {
                    matched_pair = Some((open, close, close_pos));
                    break;
                }
            }
        }

        if let Some((open, close, close_pos)) = matched_pair {
            let inner_start = index + open.len();
            let raw_inner = trimmed.get(inner_start..close_pos).unwrap_or_default();
            let trimmed_inner = raw_inner.trim();
            if !trimmed_inner.is_empty() {
                if trimmed_inner.starts_with('"')
                    && trimmed_inner.ends_with('"')
                    && trimmed_inner.len() >= 2
                {
                    let lead = raw_inner.len() - raw_inner.trim_start().len();
                    let quote_inner_start = inner_start + lead + 1;
                    let quote_inner_end = quote_inner_start + trimmed_inner.len() - 2;
                    if let Some(inner_text) = trimmed.get(quote_inner_start..quote_inner_end) {
                        candidates.push(CandidateSpan {
                            byte_range: (base_offset + quote_inner_start)
                                ..(base_offset + quote_inner_end),
                            display_text: unescape_mermaid_label(inner_text),
                            kind: LabelSpanKind::QuotedString,
                            priority: 1,
                        });
                    }
                } else {
                    candidates.push(CandidateSpan {
                        byte_range: (base_offset + inner_start)..(base_offset + close_pos),
                        display_text: unescape_mermaid_label(trimmed_inner),
                        kind: LabelSpanKind::UnquotedBracket,
                        priority: 1,
                    });
                }
            }
            index = close_pos + close.len();
            continue;
        }

        index += 1;
    }
}

fn scan_pipe_edge_labels(trimmed: &str, base_offset: usize, candidates: &mut Vec<CandidateSpan>) {
    let bytes = trimmed.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'"') {
            index = skip_quoted_literal(bytes, index);
            continue;
        }
        if bytes.get(index) == Some(&b'|') {
            let inner_start = index + 1;
            let mut cursor = inner_start;
            let mut in_quotes = false;
            while cursor < bytes.len() {
                match bytes.get(cursor) {
                    Some(b'"') => in_quotes = !in_quotes,
                    Some(b'|') if !in_quotes => break,
                    _ => {}
                }
                cursor += 1;
            }
            if cursor < bytes.len() {
                let raw_inner = trimmed.get(inner_start..cursor).unwrap_or_default();
                let trimmed_inner = raw_inner.trim();
                if !trimmed_inner.is_empty() {
                    if trimmed_inner.starts_with('"')
                        && trimmed_inner.ends_with('"')
                        && trimmed_inner.len() >= 2
                    {
                        let lead = raw_inner.len() - raw_inner.trim_start().len();
                        let quote_start = inner_start + lead + 1;
                        let quote_end = quote_start + trimmed_inner.len() - 2;
                        if let Some(inner_text) = trimmed.get(quote_start..quote_end) {
                            candidates.push(CandidateSpan {
                                byte_range: (base_offset + quote_start)..(base_offset + quote_end),
                                display_text: unescape_mermaid_label(inner_text),
                                kind: LabelSpanKind::QuotedString,
                                priority: 1,
                            });
                        }
                    } else {
                        candidates.push(CandidateSpan {
                            byte_range: (base_offset + inner_start)..(base_offset + cursor),
                            display_text: unescape_mermaid_label(trimmed_inner),
                            kind: LabelSpanKind::UnquotedPipe,
                            priority: 1,
                        });
                    }
                }
                index = cursor + 1;
                continue;
            }
        }
        index += 1;
    }
}

fn scan_inline_arrow_labels(
    trimmed: &str,
    base_offset: usize,
    candidates: &mut Vec<CandidateSpan>,
) {
    const PATTERNS: [(&str, &str); 4] = [
        (" -- ", " --> "),
        (" -- ", " --- "),
        (" -. ", " .-> "),
        (" == ", " ==> "),
    ];
    for (start_pat, end_pat) in PATTERNS {
        let mut search_from = 0usize;
        while let Some(rel_start) = trimmed
            .get(search_from..)
            .and_then(|slice| slice.find(start_pat))
        {
            let content_start = search_from + rel_start + start_pat.len();
            let Some(rel_end) = trimmed
                .get(content_start..)
                .and_then(|slice| slice.find(end_pat))
            else {
                break;
            };
            let content_end = content_start + rel_end;
            let raw = trimmed.get(content_start..content_end).unwrap_or_default();
            let label = raw.trim();
            if !label.is_empty() {
                let lead = raw.len() - raw.trim_start().len();
                candidates.push(CandidateSpan {
                    byte_range: (base_offset + content_start + lead)
                        ..(base_offset + content_start + lead + label.len()),
                    display_text: unescape_mermaid_label(label),
                    kind: LabelSpanKind::InlineArrow,
                    priority: 1,
                });
            }
            search_from = content_end + end_pat.len();
        }
    }
}

fn scan_bare_flowchart_nodes(
    trimmed: &str,
    base_offset: usize,
    candidates: &mut Vec<CandidateSpan>,
) {
    let bytes = trimmed.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'"') {
            index = skip_quoted_literal(bytes, index);
            continue;
        }
        if bytes.get(index) == Some(&b'|') {
            index += 1;
            while index < bytes.len() && bytes.get(index) != Some(&b'|') {
                index += 1;
            }
            index = (index + 1).min(bytes.len());
            continue;
        }
        if bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            let start = index;
            while index < bytes.len() && bytes.get(index).is_some_and(|byte| is_ident_byte(*byte)) {
                index += 1;
            }
            let next_byte = bytes.get(index).copied();
            let has_shape = matches!(next_byte, Some(b'[' | b'(' | b'{' | b'>'));
            if !has_shape
                && let Some(token) = trimmed.get(start..index)
                && !matches!(
                    token.to_ascii_lowercase().as_str(),
                    "subgraph" | "end" | "flowchart" | "graph" | "class"
                )
            {
                candidates.push(CandidateSpan {
                    byte_range: (base_offset + start)..(base_offset + index),
                    display_text: token.to_owned(),
                    kind: LabelSpanKind::BareFlowchartNode,
                    priority: 2,
                });
            }
            continue;
        }
        index += 1;
    }
}

fn push_plain_candidate(
    trimmed: &str,
    base_offset: usize,
    rel_start: usize,
    rel_end: usize,
    kind: LabelSpanKind,
    candidates: &mut Vec<CandidateSpan>,
) {
    if let Some(slice) = trimmed.get(rel_start..rel_end)
        && !slice.is_empty()
    {
        candidates.push(CandidateSpan {
            byte_range: (base_offset + rel_start)..(base_offset + rel_end),
            display_text: unescape_mermaid_label(slice),
            kind,
            priority: 1,
        });
    }
}

fn push_maybe_quoted_candidate(
    trimmed: &str,
    base_offset: usize,
    rel_start: usize,
    rel_end: usize,
    fallback_kind: LabelSpanKind,
    candidates: &mut Vec<CandidateSpan>,
) {
    let Some(slice) = trimmed.get(rel_start..rel_end) else {
        return;
    };
    if slice.starts_with('"') && slice.ends_with('"') && slice.len() >= 2 {
        let inner_start = rel_start + 1;
        let inner_end = rel_end - 1;
        push_plain_candidate(
            trimmed,
            base_offset,
            inner_start,
            inner_end,
            LabelSpanKind::QuotedString,
            candidates,
        );
    } else {
        push_plain_candidate(
            trimmed,
            base_offset,
            rel_start,
            rel_end,
            fallback_kind,
            candidates,
        );
    }
}

fn tail_after_keyword(trimmed: &str, keyword_len: usize) -> Option<(usize, usize)> {
    let rest = trimmed.get(keyword_len..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let lead = rest.len() - rest.trim_start().len();
    let label = rest.trim();
    if label.is_empty() {
        return None;
    }
    let start = keyword_len + lead;
    Some((start, start + label.len()))
}

fn find_ascii_word_separator(haystack: &str, word: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let needle = word.as_bytes();
    if bytes.len() < needle.len() + 2 {
        return None;
    }
    for index in 0..=(bytes.len() - needle.len() - 2) {
        let before_ws = bytes.get(index).is_some_and(u8::is_ascii_whitespace);
        let after_ws = bytes
            .get(index + 1 + needle.len())
            .is_some_and(u8::is_ascii_whitespace);
        if before_ws
            && after_ws
            && haystack
                .get(index + 1..index + 1 + needle.len())
                .is_some_and(|slice| slice.eq_ignore_ascii_case(word))
        {
            return Some(index);
        }
    }
    None
}

fn find_unquoted_char(input: &str, target: char) -> Option<usize> {
    let mut in_quotes = false;
    for (index, ch) in input.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == target && !in_quotes {
            return Some(index);
        }
    }
    None
}

fn find_closing_delimiter(input: &str, start: usize, close: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let close_bytes = close.as_bytes();
    let mut cursor = start;
    let mut in_quotes = false;
    while cursor + close_bytes.len() <= bytes.len() {
        if bytes.get(cursor) == Some(&b'"') {
            in_quotes = !in_quotes;
            cursor += 1;
            continue;
        }
        if !in_quotes
            && input
                .get(cursor..cursor + close_bytes.len())
                .is_some_and(|slice| slice == close)
        {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn skip_quoted_literal(bytes: &[u8], open_quote: usize) -> usize {
    let mut cursor = open_quote + 1;
    while cursor < bytes.len() {
        if bytes.get(cursor) == Some(&b'\\') {
            cursor = (cursor + 2).min(bytes.len());
            continue;
        }
        if bytes.get(cursor) == Some(&b'"') {
            return cursor + 1;
        }
        cursor += 1;
    }
    bytes.len()
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn unescape_mermaid_label(raw: &str) -> String {
    let with_quotes = raw.replace("#quot;", "\"");
    let mut out = String::with_capacity(with_quotes.len());
    let mut index = 0usize;
    let bytes = with_quotes.as_bytes();
    while index < bytes.len() {
        let rest = with_quotes.get(index..).unwrap_or_default();
        if let Some(br_len) = match_br_tag(rest) {
            out.push('\n');
            index += br_len;
            continue;
        }
        if let Some(ch) = rest.chars().next() {
            out.push(ch);
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn match_br_tag(rest: &str) -> Option<usize> {
    if !rest.starts_with('<') {
        return None;
    }
    let close = rest.find('>')?;
    let inside = rest.get(1..close)?.trim();
    if inside.eq_ignore_ascii_case("br")
        || inside.eq_ignore_ascii_case("br/")
        || inside.eq_ignore_ascii_case("br /")
    {
        Some(close + 1)
    } else {
        None
    }
}

// --- SVG scanning & geometry helpers ---

#[derive(Clone, Copy, Debug)]
struct Affine2D {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Affine2D {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn translate(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    fn rotate(degrees: f32) -> Self {
        let radians = degrees.to_radians();
        let cos = radians.cos();
        let sin = radians.sin();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    fn multiply(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    fn transform_point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn transform_rect(self, x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
        let corners = [
            self.transform_point(x, y),
            self.transform_point(x + width, y),
            self.transform_point(x, y + height),
            self.transform_point(x + width, y + height),
        ];
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (px, py) in corners {
            min_x = min_x.min(px);
            min_y = min_y.min(py);
            max_x = max_x.max(px);
            max_y = max_y.max(py);
        }
        (
            min_x,
            min_y,
            (max_x - min_x).max(1.0),
            (max_y - min_y).max(1.0),
        )
    }
}

struct ElementFrame {
    tag_name: String,
    transform: Affine2D,
    font_size: f32,
    skip_subtree: bool,
    in_defs: bool,
    is_defs_label_group: bool,
    is_fallback_group: bool,
    is_native_text: bool,
}

struct DefsLabelTemplate {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    text: String,
}

struct DefsGroupCollector {
    id: String,
    rect: Option<(f32, f32, f32, f32)>,
    lines: Vec<String>,
    current_line: String,
    in_text_depth: usize,
}

impl DefsGroupCollector {
    fn finish(self) -> Option<(String, DefsLabelTemplate)> {
        let (x, y, width, height) = self.rect?;
        let text = self.lines.join("\n");
        if text.trim().is_empty()
            || !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
        Some((
            self.id,
            DefsLabelTemplate {
                x,
                y,
                width,
                height,
                text,
            },
        ))
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    if ned.is_empty() || hay.len() < ned.len() {
        return None;
    }
    for index in 0..=(hay.len() - ned.len()) {
        if haystack
            .get(index..index + ned.len())
            .is_some_and(|slice| slice.eq_ignore_ascii_case(needle))
        {
            return Some(index);
        }
    }
    None
}

#[derive(Default)]
struct FallbackCollector {
    rect: Option<(f32, f32, f32, f32)>,
    lines: Vec<String>,
    current_line: String,
    in_text_depth: usize,
}

impl FallbackCollector {
    fn finish(self) -> Option<MermaidSvgLabel> {
        let (x, y, width, height) = self.rect?;
        let text = self.lines.join("\n");
        if text.trim().is_empty()
            || !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
        Some(MermaidSvgLabel {
            text,
            occurrence: 0,
            x,
            y,
            width,
            height,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum TextAnchor {
    Start,
    Middle,
    End,
}

impl TextAnchor {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "middle" => Self::Middle,
            "end" => Self::End,
            _ => Self::Start,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum DominantBaseline {
    Alphabetic,
    Central,
    Hanging,
}

impl DominantBaseline {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "central" | "middle" => Self::Central,
            "hanging" | "text-before-edge" | "before-edge" => Self::Hanging,
            _ => Self::Alphabetic,
        }
    }
}

struct NativeTextCollector {
    x: f32,
    y: f32,
    font_size: f32,
    anchor: TextAnchor,
    baseline: DominantBaseline,
    transform: Affine2D,
    lines: Vec<String>,
    current_line: String,
    last_tspan_y: Option<f32>,
}

impl NativeTextCollector {
    fn new(
        x: f32,
        y: f32,
        font_size: f32,
        anchor: TextAnchor,
        baseline: DominantBaseline,
        transform: Affine2D,
    ) -> Self {
        Self {
            x,
            y,
            font_size: font_size.clamp(6.0, 96.0),
            anchor,
            baseline,
            transform,
            lines: Vec::new(),
            current_line: String::new(),
            last_tspan_y: None,
        }
    }

    fn start_tspan(&mut self, x: Option<f32>, y: Option<f32>, dy: Option<f32>, self_closing: bool) {
        let starts_new_line = match (y, dy) {
            (Some(new_y), _)
                if self
                    .last_tspan_y
                    .is_some_and(|prev| (new_y - prev).abs() > 1.0) =>
            {
                true
            }
            (_, Some(delta_y)) if delta_y.abs() > 0.5 && !self.current_line.trim().is_empty() => {
                true
            }
            _ => false,
        };
        if starts_new_line {
            let line = std::mem::take(&mut self.current_line);
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                self.lines.push(trimmed.to_owned());
            }
        }
        if let Some(new_x) = x
            && self.lines.is_empty()
            && self.current_line.is_empty()
        {
            self.x = new_x;
        }
        if let Some(new_y) = y {
            if self.lines.is_empty() && self.current_line.is_empty() && self.last_tspan_y.is_none()
            {
                self.y = new_y;
            }
            self.last_tspan_y = Some(new_y);
        }
        if self_closing {
            self.finish_tspan();
        }
    }

    fn push_chunk(&mut self, chunk: &str) {
        self.current_line.push_str(chunk);
    }

    fn finish_tspan(&mut self) {}

    fn finish(mut self) -> Option<MermaidSvgLabel> {
        let trailing = self.current_line.trim();
        if !trailing.is_empty() {
            self.lines.push(trailing.to_owned());
        }
        if self.lines.is_empty() {
            return None;
        }
        let max_chars = self
            .lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let line_height = self.font_size * 1.25;
        let width = (max_chars * self.font_size * 0.60).max(self.font_size);
        let height = (self.lines.len() as f32 * line_height).max(line_height);
        let local_x = match self.anchor {
            TextAnchor::Start => self.x,
            TextAnchor::Middle => self.x - width * 0.5,
            TextAnchor::End => self.x - width,
        };
        let local_y = match self.baseline {
            DominantBaseline::Alphabetic => self.y - self.font_size * 0.85,
            DominantBaseline::Central => self.y - line_height * 0.5,
            DominantBaseline::Hanging => self.y,
        };
        let (x, y, rect_w, rect_h) = self
            .transform
            .transform_rect(local_x, local_y, width, height);
        if !x.is_finite()
            || !y.is_finite()
            || !rect_w.is_finite()
            || !rect_h.is_finite()
            || rect_w <= 0.0
            || rect_h <= 0.0
        {
            return None;
        }
        Some(MermaidSvgLabel {
            text: self.lines.join("\n"),
            occurrence: 0,
            x,
            y,
            width: rect_w,
            height: rect_h,
        })
    }
}

fn pop_matching_frame(stack: &mut Vec<ElementFrame>, closing_name: &str) -> Option<ElementFrame> {
    let position = stack
        .iter()
        .rposition(|frame| frame.tag_name.eq_ignore_ascii_case(closing_name))?;
    let frame = stack.remove(position);
    stack.truncate(position);
    Some(frame)
}

fn parse_element_transform(header: &str, tag_name: &str, root_svg_seen: bool) -> Affine2D {
    let mut matrix = Affine2D::IDENTITY;

    if root_svg_seen && tag_name.eq_ignore_ascii_case("svg") {
        let x = extract_xml_attribute(header, "x")
            .and_then(parse_svg_number)
            .unwrap_or(0.0);
        let y = extract_xml_attribute(header, "y")
            .and_then(parse_svg_number)
            .unwrap_or(0.0);
        if x != 0.0 || y != 0.0 {
            matrix = matrix.multiply(Affine2D::translate(x, y));
        }
    }

    // CSS `transform` overrides the SVG `transform` attribute rather than composing with it.
    let style_transform = extract_css_property(header, "transform")
        .filter(|value| !value.trim().eq_ignore_ascii_case("none"));
    let transform_source = style_transform
        .as_deref()
        .or_else(|| extract_xml_attribute(header, "transform"));

    if let Some(raw_transform) = transform_source {
        matrix = matrix.multiply(parse_svg_transform(raw_transform));
    }

    matrix
}

fn parse_svg_transform(raw: &str) -> Affine2D {
    let mut result = Affine2D::IDENTITY;
    let mut cursor = 0usize;
    let bytes = raw.as_bytes();
    while cursor < bytes.len() {
        let rest = raw.get(cursor..).unwrap_or_default();
        let Some(open_paren) = rest.find('(') else {
            break;
        };
        let Some(close_paren) = rest.get(open_paren + 1..).and_then(|slice| slice.find(')')) else {
            break;
        };
        let fn_name = rest
            .get(..open_paren)
            .unwrap_or_default()
            .trim()
            .trim_start_matches(',');
        let args_str = rest
            .get(open_paren + 1..open_paren + 1 + close_paren)
            .unwrap_or_default();
        let args: Vec<f32> = args_str
            .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
            .filter(|part| !part.is_empty())
            .filter_map(parse_svg_number)
            .collect();

        let next = match fn_name.to_ascii_lowercase().as_str() {
            "translate" => match args.as_slice() {
                [tx] => Some(Affine2D::translate(*tx, 0.0)),
                [tx, ty, ..] => Some(Affine2D::translate(*tx, *ty)),
                _ => None,
            },
            "translatex" => args.first().map(|tx| Affine2D::translate(*tx, 0.0)),
            "translatey" => args.first().map(|ty| Affine2D::translate(0.0, *ty)),
            "scale" => match args.as_slice() {
                [s] => Some(Affine2D::scale(*s, *s)),
                [sx, sy, ..] => Some(Affine2D::scale(*sx, *sy)),
                _ => None,
            },
            "rotate" => match args.as_slice() {
                [deg] => Some(Affine2D::rotate(*deg)),
                [deg, cx, cy, ..] => Some(
                    Affine2D::translate(*cx, *cy)
                        .multiply(Affine2D::rotate(*deg))
                        .multiply(Affine2D::translate(-*cx, -*cy)),
                ),
                _ => None,
            },
            "matrix" => match args.as_slice() {
                [a, b, c, d, e, f, ..] => Some(Affine2D {
                    a: *a,
                    b: *b,
                    c: *c,
                    d: *d,
                    e: *e,
                    f: *f,
                }),
                _ => None,
            },
            _ => None,
        };
        if let Some(local) = next {
            result = result.multiply(local);
        }
        cursor += open_paren + 1 + close_paren + 1;
    }
    result
}

fn parse_element_font_size(header: &str) -> Option<f32> {
    extract_style_or_attr(header, "font-size").and_then(|value| parse_svg_length_px(&value))
}

fn extract_style_or_attr(header: &str, name: &str) -> Option<String> {
    if let Some(style_value) = extract_css_property(header, name) {
        return Some(style_value);
    }
    extract_xml_attribute(header, name).map(str::to_owned)
}

fn extract_css_property(header: &str, property: &str) -> Option<String> {
    let style = extract_xml_attribute(header, "style")?;
    for declaration in style.split(';') {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(property) {
            let cleaned = value
                .trim()
                .strip_suffix("!important")
                .unwrap_or(value)
                .trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_owned());
            }
        }
    }
    None
}

fn find_tag_end(svg: &str, lt_index: usize) -> Option<usize> {
    let bytes = svg.as_bytes();
    let mut index = lt_index + 1;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = *bytes.get(index)?;
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index),
            None => {}
        }
        index += 1;
    }
    None
}

fn parse_tag_name(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let end = trimmed
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '/' || ch == '>')
        .unwrap_or(trimmed.len());
    trimmed.get(..end).unwrap_or_default()
}

fn find_svg_open_tag(svg: &str) -> Option<usize> {
    let bytes = svg.as_bytes();
    let mut index = 0usize;
    while index + 4 <= bytes.len() {
        if bytes.get(index) == Some(&b'<')
            && svg
                .get(index + 1..index + 4)
                .is_some_and(|tag| tag.eq_ignore_ascii_case("svg"))
            && bytes
                .get(index + 4)
                .is_some_and(|next| next.is_ascii_whitespace() || *next == b'>')
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn extract_xml_attribute<'a>(tag_header: &'a str, attribute: &str) -> Option<&'a str> {
    let bytes = tag_header.as_bytes();
    let attr_len = attribute.len();
    let mut index = 0usize;
    while index + attr_len < bytes.len() {
        let preceded_by_space =
            index == 0 || bytes.get(index - 1).is_some_and(u8::is_ascii_whitespace);
        if preceded_by_space
            && tag_header
                .get(index..index + attr_len)
                .is_some_and(|slice| slice.eq_ignore_ascii_case(attribute))
        {
            let mut cursor = index + attr_len;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                    cursor += 1;
                }
                let quote = *bytes.get(cursor)?;
                if quote == b'"' || quote == b'\'' {
                    let value_start = cursor + 1;
                    let rest = tag_header.get(value_start..)?;
                    let value_end = rest.find(quote as char)?;
                    return rest.get(..value_end);
                }
            }
        }
        index += 1;
    }
    None
}

fn parse_view_box_rect(raw: &str) -> Option<SvgViewBox> {
    let mut parts = raw
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|part| !part.is_empty());
    let min_x: f32 = parts.next()?.parse().ok()?;
    let min_y: f32 = parts.next()?.parse().ok()?;
    let width: f32 = parts.next()?.parse().ok()?;
    let height: f32 = parts.next()?.parse().ok()?;
    if min_x.is_finite()
        && min_y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
    {
        Some(SvgViewBox {
            x: min_x,
            y: min_y,
            width,
            height,
        })
    } else {
        None
    }
}

fn parse_svg_number(raw: &str) -> Option<f32> {
    let trimmed = raw
        .trim()
        .strip_suffix("px")
        .or_else(|| raw.trim().strip_suffix("em"))
        .unwrap_or(raw.trim());
    let parsed: f32 = trimmed.trim().parse().ok()?;
    parsed.is_finite().then_some(parsed)
}

fn parse_svg_dy(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if let Some(em) = trimmed.strip_suffix("em") {
        let value: f32 = em.trim().parse().ok()?;
        return value.is_finite().then_some(value * 16.0);
    }
    parse_svg_number(trimmed)
}

fn parse_svg_length_px(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if trimmed.ends_with('%') {
        return None;
    }
    let (number, factor) = if let Some(value) = trimmed.strip_suffix("px") {
        (value, 1.0f32)
    } else if let Some(value) = trimmed.strip_suffix("pt") {
        (value, 4.0 / 3.0)
    } else {
        (trimmed, 1.0f32)
    };
    let parsed: f32 = number.trim().parse().ok()?;
    let scaled = parsed * factor;
    if scaled.is_finite() && scaled > 0.0 {
        Some(scaled)
    } else {
        None
    }
}

fn decode_xml_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while let Some(rel_amp) = raw.get(cursor..).and_then(|slice| slice.find('&')) {
        let amp = cursor + rel_amp;
        if let Some(prefix) = raw.get(cursor..amp) {
            output.push_str(prefix);
        }
        let rest = raw.get(amp + 1..).unwrap_or_default();
        if let Some(semi) = rest.find(';') {
            let entity = rest.get(..semi).unwrap_or_default();
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                numeric if numeric.starts_with("#x") || numeric.starts_with("#X") => {
                    u32::from_str_radix(numeric.get(2..).unwrap_or_default(), 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                numeric if numeric.starts_with('#') => numeric
                    .get(1..)
                    .unwrap_or_default()
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = decoded {
                output.push(ch);
                cursor = amp + 1 + semi + 1;
                continue;
            }
        }
        output.push('&');
        cursor = amp + 1;
    }
    if let Some(tail) = raw.get(cursor..) {
        output.push_str(tail);
    }
    output
}
