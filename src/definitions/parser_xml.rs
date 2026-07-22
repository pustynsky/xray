//! On-demand XML parser using tree-sitter-xml.
//!
//! Unlike other parsers (C#, TypeScript, Rust), XML files are NOT indexed
//! in the DefinitionIndex. This parser is invoked on-demand when
//! `xray_definitions` receives a request for a file with an XML extension.
//!
//! Key design decisions:
//! - Each XML element becomes a `DefinitionEntry` with kind `XmlElement`
//! - `signature` contains an XPath-like ancestry path: `Root > Section > Element`
//! - `attributes` contains XML attribute name=value pairs
//! - `parent` contains the parent element name
//! - Call sites and code stats are always empty (no code in XML)
//!
//! ## Best-effort parsing and known limitations
//!
//! - **Malformed XML**: if tree-sitter fails to recover a proper `STag`/`EmptyElemTag`
//!   for an `element` node, that element itself is skipped (no name to report), but
//!   **its children are still walked**. The parser records a `parse_warnings` entry
//!   for each skipped anchor so callers can surface the fact that results may be
//!   partial (WALKER-2).
//! - **Depth limit**: recursion is capped at `MAX_RECURSION_DEPTH` (1024) to avoid
//!   stack overflow on pathologically nested XML (WALKER-3). When the limit is hit,
//!   a `parse_warnings` entry is added and that subtree is truncated; the rest of
//!   the file continues to parse normally.
//! - **XML entity escapes**: `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and numeric
//!   entity references (`&#xA;`) are **not decoded** in attribute values or text
//!   content. They appear verbatim in the output. This is acceptable for
//!   signature/display use, but means a `name='foo & bar'` search will not match
//!   text that contains `foo &amp; bar` in the source. See `docs/mcp-guide.md`.

use crate::definitions::types::{DefinitionEntry, DefinitionKind};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use thiserror::Error;

/// Maximum depth for the recursive walker. Elements deeper than this are skipped
/// and a warning is recorded in `ParseResult.warnings`. Chosen to be well below
/// Rust's default 8 MB stack (each frame is ~500 B here, so 1024 levels ≈ 0.5 MB).
const MAX_RECURSION_DEPTH: usize = 1024;

/// Character count at which text content is truncated (WALKER-aware, UTF-8 safe).
const TEXT_CONTENT_MAX_CHARS: usize = 200;
const TEXT_CONTENT_TRUNCATE_TO: usize = 197;

const XML_DECLARATION_SCAN_BYTES: usize = 1024;
const MAX_XML_CHARACTER_WARNINGS: usize = 8;
const MAX_XML_STRUCTURE_WARNINGS: usize = 16;

/// Typed errors from the on-demand XML parser.
///
/// Discriminates between infrastructure failures (grammar load — a bug in our
/// build, not the user's input) and user-input failures (tree-sitter parser
/// returned None — possibly malformed XML). The caller can produce different
/// diagnostic messages and route errors accordingly (e.g. log grammar issues,
/// show "file may be malformed" only to the user).
#[derive(Debug, Error)]
pub(crate) enum XmlParseError {
    /// tree-sitter-xml grammar failed to load. This is an internal bug —
    /// the user's input never causes this. Report to developers.
    #[error("Failed to load tree-sitter XML grammar: {0}")]
    GrammarLoad(String),

    /// tree-sitter parser returned None. This usually means the parser's
    /// internal state was corrupt or the input was pathologically malformed.
    /// Unlike `GrammarLoad`, this is visible to the user.
    #[error("tree-sitter XML parser returned None — file may be malformed or empty")]
    TreeSitterReturnedNone,

    /// PARSE-002: source exceeded the per-file size cap before parsing was
    /// attempted. Surfaced so the caller can render a clear warning instead
    /// of a generic "no definitions found".
    #[error("XML source too large: {size} bytes exceeds limit {limit}")]
    SourceTooLarge { size: usize, limit: usize },
}

/// Parsed XML element with additional metadata not in DefinitionEntry.
#[derive(Debug, Clone)]
pub(crate) struct XmlDefinition {
    pub entry: DefinitionEntry,
    /// Text content of leaf elements (no child elements), truncated to 200 chars.
    pub text_content: Option<String>,
    /// Whether this element has child elements (is a "block").
    pub has_child_elements: bool,
    /// Index of the parent in the result Vec, if any.
    pub parent_index: Option<usize>,
}

/// Result of parsing an XML file: the definitions plus any non-fatal warnings
/// emitted by the walker (malformed element anchors, depth-limit truncation).
///
/// Callers that only care about the definitions can keep using
/// `parse_xml_on_demand` directly and treat warnings as optional diagnostics.
#[derive(Debug, Default)]
pub(crate) struct ParseResult {
    pub definitions: Vec<XmlDefinition>,
    pub warnings: Vec<String>,
}

/// Check if a file extension is an XML extension we support for on-demand parsing.
pub(crate) fn is_xml_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        // Generic XML
        "xml"
        // .NET project / MSBuild family
        | "config" | "csproj" | "vbproj" | "fsproj" | "vcxproj"
        | "props" | "targets"
        // NuGet / ClickOnce / VSIX / Service Fabric
        | "nuspec" | "vsixmanifest" | "manifestxml" | "appxmanifest"
        // Localized resources
        | "resx"
    )
}

fn xml_declaration_encoding(source: &str) -> Option<&str> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let bytes = source.as_bytes();
    if !bytes.starts_with(b"<?xml") || !bytes.get(5).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }

    let scan_len = bytes.len().min(XML_DECLARATION_SCAN_BYTES);
    let declaration_end = bytes[..scan_len]
        .windows(2)
        .position(|window| window == b"?>")?;
    let declaration = std::str::from_utf8(&bytes[5..declaration_end]).ok()?;
    let bytes = declaration.as_bytes();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let name_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        {
            cursor += 1;
        }
        let name_end = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let quote = *bytes.get(cursor)?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| *byte != quote) {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return None;
        }
        let value_end = cursor;
        cursor += 1;

        let name = declaration.get(name_start..name_end)?;
        if name.eq_ignore_ascii_case("encoding") {
            return declaration.get(value_start..value_end);
        }
    }
    None
}

fn is_utf8_compatible_declaration(encoding: &str, source: &str) -> bool {
    encoding.eq_ignore_ascii_case("utf-8")
        || encoding.eq_ignore_ascii_case("utf8")
        || encoding.eq_ignore_ascii_case("us-ascii") && source.is_ascii()
}

fn is_valid_xml_1_0_character(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{a}' | '\u{d}')
        || ('\u{20}'..='\u{d7ff}').contains(&character)
        || ('\u{e000}'..='\u{fffd}').contains(&character)
        || ('\u{10000}'..='\u{10ffff}').contains(&character)
}

fn prevalidate_xml_source(source: &str) -> (Vec<String>, bool) {
    let mut warnings = Vec::new();
    if let Some(encoding) = xml_declaration_encoding(source)
        && !is_utf8_compatible_declaration(encoding, source)
    {
        warnings.push(format!(
            "XML declares encoding '{encoding}', but Xray decoded the file as UTF-8; \
             structural results may not match strict XML consumers."
        ));
    }

    let mut invalid_count = 0usize;
    let mut line = 1usize;
    let mut column = 1usize;
    let mut previous_was_cr = false;
    for character in source.chars() {
        if !is_valid_xml_1_0_character(character) {
            invalid_count += 1;
            if invalid_count <= MAX_XML_CHARACTER_WARNINGS {
                warnings.push(format!(
                    "XML 1.0 forbids raw character U+{:04X} at line {line}, column {column}; \
                     recovered results may be invalid.",
                    character as u32
                ));
            }
        }

        match character {
            '\r' => {
                line += 1;
                column = 1;
                previous_was_cr = true;
            }
            '\n' if previous_was_cr => {
                column = 1;
                previous_was_cr = false;
            }
            '\n' => {
                line += 1;
                column = 1;
                previous_was_cr = false;
            }
            _ => {
                column += 1;
                previous_was_cr = false;
            }
        }
    }
    if invalid_count > MAX_XML_CHARACTER_WARNINGS {
        warnings.push(format!(
            "XML contains {} additional forbidden XML 1.0 characters.",
            invalid_count - MAX_XML_CHARACTER_WARNINGS
        ));
    }
    let strict_error = append_xml_structure_warnings(source, &mut warnings);
    (warnings, strict_error)
}

fn append_xml_structure_warnings(source: &str, warnings: &mut Vec<String>) -> bool {
    let mut reader = NsReader::from_str(source);
    let warning_limit = warnings.len().saturating_add(MAX_XML_STRUCTURE_WARNINGS);
    let mut previous_position = 0;
    let mut line = 1;
    let mut strict_error = false;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(error) => {
                strict_error = true;
                if warnings.len() < warning_limit {
                    let error_position = usize::try_from(reader.error_position())
                        .unwrap_or(source.len())
                        .min(source.len());
                    let error_line = source.as_bytes()[..error_position]
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count()
                        + 1;
                    warnings.push(format!(
                        "XML namespace/well-formedness validation failed at line {error_line}: {error}."
                    ));
                }
                break;
            }
        };
        let position = usize::try_from(reader.buffer_position())
            .unwrap_or(source.len())
            .min(source.len());
        line += source.as_bytes()[previous_position..position]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        previous_position = position;

        match event {
            Event::Start(start) | Event::Empty(start) => {
                validate_xml_start_event(&reader, &start, line, warnings, warning_limit);
                if warnings.len() >= warning_limit {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    strict_error
}

fn validate_xml_start_event(
    reader: &NsReader<&[u8]>,
    start: &BytesStart<'_>,
    line: usize,
    warnings: &mut Vec<String>,
    warning_limit: usize,
) {
    if let ResolveResult::Unknown(prefix) = reader.resolver().resolve_element(start.name()).0
        && warnings.len() < warning_limit
    {
        warnings.push(format!(
            "XML element uses undeclared prefix '{}' at line {line}; recovered results may be invalid.",
            String::from_utf8_lossy(&prefix)
        ));
    }

    let mut raw_names = std::collections::HashSet::new();
    let mut expanded_names = std::collections::HashSet::new();
    let mut attributes = start.attributes();
    let _ = attributes.with_checks(false);
    for attribute in attributes {
        if warnings.len() >= warning_limit {
            break;
        }
        let attribute = match attribute {
            Ok(attribute) => attribute,
            Err(error) => {
                warnings.push(format!(
                    "XML has invalid or duplicate attribute at line {line}: {error}."
                ));
                continue;
            }
        };
        let raw_name = attribute.key.as_ref();
        if !raw_names.insert(raw_name.to_vec()) {
            warnings.push(format!(
                "XML has duplicate attribute '{}' at line {line}.",
                String::from_utf8_lossy(raw_name)
            ));
            continue;
        }
        if raw_name == b"xmlns" || raw_name.starts_with(b"xmlns:") {
            continue;
        }

        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = match namespace {
            ResolveResult::Unknown(prefix) => {
                warnings.push(format!(
                    "XML attribute uses undeclared prefix '{}' at line {line}; recovered results may be invalid.",
                    String::from_utf8_lossy(&prefix)
                ));
                continue;
            }
            ResolveResult::Bound(namespace) => namespace.as_ref().to_vec(),
            ResolveResult::Unbound => Vec::new(),
        };
        let expanded_name = (namespace.clone(), local_name.as_ref().to_vec());
        if !expanded_names.insert(expanded_name) {
            warnings.push(format!(
                "XML has duplicate attribute '{{{}}}{}' at line {line}.",
                String::from_utf8_lossy(&namespace),
                String::from_utf8_lossy(local_name.as_ref())
            ));
        }
    }
}

/// Parse an XML file on-demand and return structured definitions.
///
/// Returns a list of `XmlDefinition` entries representing all XML elements
/// in the file, with parent relationships, signatures, and text content.
///
/// This function does NOT populate the DefinitionIndex — it returns
/// standalone results for immediate use by the handler.
///
/// Non-fatal problems (malformed element anchors, depth-limit truncation) are
/// recorded as warnings and returned via [`parse_xml_on_demand_with_warnings`].
/// This thin wrapper returns only the definitions, preserving the original API
/// used by ~20 unit tests in `definitions_tests_xml.rs`. Production handler code
/// goes through `parse_xml_on_demand_with_warnings` directly to surface warnings
/// in the JSON response.
#[allow(dead_code)] // public-ish API used by tests; production path uses _with_warnings
pub(crate) fn parse_xml_on_demand(
    source: &str,
    file_path: &str,
) -> Result<Vec<XmlDefinition>, XmlParseError> {
    parse_xml_on_demand_with_warnings(source, file_path).map(|r| r.definitions)
}

/// Parse an XML file on-demand and return both definitions and warnings.
///
/// Prefer this over [`parse_xml_on_demand`] when the caller wants to surface
/// "parse succeeded but with caveats" diagnostics to the user (e.g. malformed
/// XML leading to partial results, depth-limit truncation).
pub(crate) fn parse_xml_on_demand_with_warnings(
    source: &str,
    _file_path: &str,
) -> Result<ParseResult, XmlParseError> {
    use super::tree_sitter_utils::{MAX_PARSE_SOURCE_BYTES, PARSE_TIMEOUT_MICROS};

    // PARSE-002: skip oversized sources before tree-sitter allocates ~10× RAM.
    if source.len() > MAX_PARSE_SOURCE_BYTES {
        tracing::warn!(
            target: "xray::parse",
            file = %_file_path,
            size = source.len(),
            limit = MAX_PARSE_SOURCE_BYTES,
            "skipping oversized XML source"
        );
        return Err(XmlParseError::SourceTooLarge {
            size: source.len(),
            limit: MAX_PARSE_SOURCE_BYTES,
        });
    }

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_xml::LANGUAGE_XML.into())
        .map_err(|e| XmlParseError::GrammarLoad(e.to_string()))?;
    // PARSE-001: bound parse wall-clock so a single pathological file cannot
    // pin a worker thread indefinitely.
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);

    let tree = parser
        .parse(source, None)
        .ok_or(XmlParseError::TreeSitterReturnedNone)?;

    let source_bytes = source.as_bytes();
    let root = tree.root_node();
    let (warnings, strict_error) = prevalidate_xml_source(source);
    let mut ctx = WalkCtx {
        defs: Vec::new(),
        ancestry: Vec::new(),
        warnings,
    };
    if root.has_error() && !strict_error {
        ctx.warnings
            .push("XML contains syntax errors; recovered results may be incomplete.".to_string());
    }

    walk_xml_node(root, source_bytes, None, 0, &mut ctx);

    // Mark parents that actually have element children. Single-pass O(N) —
    // indices are distinct (no self-parenting possible in a tree) so we can
    // read `parent_index` and write `has_child_elements` without a split borrow
    // by taking the value out first (MINOR-2: drops the extra `Vec<Option<usize>>`
    // clone the previous implementation used).
    for i in 0..ctx.defs.len() {
        if let Some(p) = ctx.defs[i].parent_index
            && p < ctx.defs.len() {
                ctx.defs[p].has_child_elements = true;
            }
    }

    Ok(ParseResult {
        definitions: ctx.defs,
        warnings: ctx.warnings,
    })
}

/// Mutable walker context: accumulates results + the live ancestry stack.
///
/// `ancestry` is a persistent stack: each element pushes its own name before
/// recursing into children and pops it afterwards (WALKER-1). This replaces
/// the previous O(depth²) cloning scheme, where every child recursion cloned
/// the full `Vec<String>` of ancestors.
struct WalkCtx {
    defs: Vec<XmlDefinition>,
    ancestry: Vec<String>,
    warnings: Vec<String>,
}

/// Recursively walk the XML AST tree and collect element definitions.
///
/// `parent_index` threads the index of the enclosing element in `ctx.defs`.
/// The parent *name* is read on demand from `ctx.ancestry.last()` — no need
/// to pass a separate `parent_name` argument (WALKER-4: eliminates a redundant
/// borrow that tied the walker to a specific call shape).
fn walk_xml_node(
    node: tree_sitter::Node,
    source: &[u8],
    parent_index: Option<usize>,
    depth: usize,
    ctx: &mut WalkCtx,
) {
    enum WalkAction<'tree> {
        Visit {
            node: tree_sitter::Node<'tree>,
            parent_index: Option<usize>,
            depth: usize,
        },
        LeaveElement,
    }

    let mut pending = vec![WalkAction::Visit {
        node,
        parent_index,
        depth,
    }];

    while let Some(action) = pending.pop() {
        let (node, parent_index, depth) = match action {
            WalkAction::Visit {
                node,
                parent_index,
                depth,
            } => (node, parent_index, depth),
            WalkAction::LeaveElement => {
                ctx.ancestry.pop();
                continue;
            }
        };

        if depth > MAX_RECURSION_DEPTH {
            ctx.warnings.push(format!(
                "XML nesting exceeded {} levels; subtree at line {} truncated.",
                MAX_RECURSION_DEPTH,
                node.start_position().row + 1
            ));
            continue;
        }

        match node.kind() {
            "element" => {
                if let Some(element_name) = extract_element_name(node, source) {
                    let line_start = node.start_position().row as u32 + 1;
                    let line_end = node.end_position().row as u32 + 1;

                    let sig_name =
                        build_element_signature_name(node, source, &element_name);
                    let mut sig_parts: Vec<String> = ctx.ancestry.clone();
                    sig_parts.push(sig_name);
                    let signature = sig_parts.join(" > ");

                    let attributes = extract_xml_attributes(node, source);
                    let text_content = extract_text_content(node, source);
                    let parent_name = ctx.ancestry.last().cloned();

                    let def_index = ctx.defs.len();
                    ctx.defs.push(XmlDefinition {
                        entry: DefinitionEntry {
                            file_id: 0,
                            name: element_name.clone(),
                            kind: DefinitionKind::XmlElement,
                            line_start,
                            line_end,
                            parent: parent_name,
                            signature: Some(signature),
                            modifiers: Vec::new(),
                            attributes,
                            base_types: Vec::new(),
                        },
                        text_content,
                        has_child_elements: false,
                        parent_index,
                    });

                    ctx.ancestry.push(element_name);
                    pending.push(WalkAction::LeaveElement);

                    let mut cursor = node.walk();
                    let children: Vec<_> = node.children(&mut cursor).collect();
                    for child in children.into_iter().rev() {
                        pending.push(WalkAction::Visit {
                            node: child,
                            parent_index: Some(def_index),
                            depth: depth + 1,
                        });
                    }
                } else {
                    ctx.warnings.push(format!(
                        "Could not extract element name at line {}; children still walked.",
                        node.start_position().row + 1
                    ));
                    let mut cursor = node.walk();
                    let children: Vec<_> = node.children(&mut cursor).collect();
                    for child in children.into_iter().rev() {
                        pending.push(WalkAction::Visit {
                            node: child,
                            parent_index,
                            depth: depth + 1,
                        });
                    }
                }
            }
            _ => {
                let mut cursor = node.walk();
                let children: Vec<_> = node.children(&mut cursor).collect();
                for child in children.into_iter().rev() {
                    pending.push(WalkAction::Visit {
                        node: child,
                        parent_index,
                        depth: depth + 1,
                    });
                }
            }
        }
    }
}

/// Extract element name from an `element` node.
/// Looks for STag > Name or EmptyElemTag > Name.
fn extract_element_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "STag" | "EmptyElemTag" => {
                return extract_name_from_node(child, source);
            }
            _ => {}
        }
    }
    None
}

/// Extract a Name from a tag node (STag, ETag, EmptyElemTag).
fn extract_name_from_node(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "Name" {
            return child
                .utf8_text(source)
                .ok()
                .map(|s| s.to_string());
        }
    }
    None
}

/// Extract XML attributes from an element node as "name=value" strings.
fn extract_xml_attributes(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut attrs = Vec::new();
    collect_attributes_recursive(node, source, &mut attrs);
    attrs
}

/// Recursively collect attributes from a node and its immediate children.
fn collect_attributes_recursive(
    node: tree_sitter::Node,
    source: &[u8],
    attrs: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "Attribute" => {
                if let Some(attr_str) = format_attribute(child, source) {
                    attrs.push(attr_str);
                }
            }
            "STag" | "EmptyElemTag" => {
                // Look for attributes inside the opening tag
                collect_attributes_recursive(child, source, attrs);
            }
            _ => {}
        }
    }
}

/// Format a single Attribute node as "name=value".
///
/// NOTE: XML entity escapes in the value (`&amp;`, `&lt;`, `&quot;`, numeric
/// `&#xA;` refs, etc.) are **not decoded** — they are surfaced verbatim. This
/// matches tree-sitter's raw text view and is documented in the module header.
fn format_attribute(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut name = None;
    let mut value = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "Name" => {
                name = child.utf8_text(source).ok().map(|s| s.to_string());
            }
            "AttValue" => {
                // AttValue includes the quotes — strip them
                let raw = child.utf8_text(source).ok().unwrap_or_default();
                let stripped = raw
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(raw);
                value = Some(stripped.to_string());
            }
            _ => {}
        }
    }
    match (name, value) {
        (Some(n), Some(v)) => Some(format!("{}={}", n, v)),
        (Some(n), None) => Some(n),
        _ => None,
    }
}

/// Extract text content from an element node.
///
/// Only returns content for leaf elements (elements with no child elements).
/// Truncates to `TEXT_CONTENT_MAX_CHARS` characters (character-aware — safe for
/// UTF-8 input, see regression tests `test_text_content_truncation_utf8_*`).
///
/// WALKER-7: aborts as soon as the first element/EmptyElemTag child is found —
/// we know a block element cannot have a text representation, so collecting
/// `CharData` or CDATA from other siblings would be wasted work. For large XML
/// blocks with many text/comment/PI siblings this avoids a full content sweep.
///
/// NOTE: XML entity escapes in CharData (`&amp;`, `&lt;`, numeric refs) are
/// **not decoded**. They appear verbatim in the returned string.
fn extract_text_content(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut text_content = String::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "content" {
            continue;
        }
        let mut content_cursor = child.walk();
        for content_child in child.children(&mut content_cursor) {
            match content_child.kind() {
                "element" | "EmptyElemTag" => {
                    // WALKER-7 early-exit: block element — no text content to
                    // report, abandon collection immediately.
                    return None;
                }
                "CharData" | "EntityRef" | "CharRef" => {
                    if let Ok(segment) = content_child.utf8_text(source) {
                        text_content.push_str(segment);
                    }
                }
                "CDSect" => {
                    let mut cdata_cursor = content_child.walk();
                    for cdata_child in content_child.children(&mut cdata_cursor) {
                        if cdata_child.kind() == "CData"
                            && let Ok(segment) = cdata_child.utf8_text(source) {
                                text_content.push_str(segment);
                            }
                    }
                }
                _ => {}
            }
        }
    }

    let combined = text_content.trim();
    if combined.is_empty() {
        return None;
    }
    // Truncate by character count (not bytes) to avoid slicing in the middle of a
    // UTF-8 multi-byte sequence. Using byte index on non-ASCII text (Cyrillic, CJK,
    // emoji) would cause `byte index N is not a char boundary` panic.
    if combined.chars().count() > TEXT_CONTENT_MAX_CHARS {
        let truncated: String = combined.chars().take(TEXT_CONTENT_TRUNCATE_TO).collect();
        Some(format!("{}...", truncated))
    } else {
        Some(combined.to_string())
    }
}

/// Build a signature name for an element, including key attributes.
/// For example: `add[@key=DbConnection]` or `SearchService`.
fn build_element_signature_name(
    node: tree_sitter::Node,
    source: &[u8],
    element_name: &str,
) -> String {
    // Look for "key", "name", or "id" attributes to include in signature
    let key_attrs = ["key", "name", "id"];
    let attrs = extract_xml_attributes(node, source);

    for key_attr in &key_attrs {
        for attr in &attrs {
            if let Some((attr_name, attr_value)) = attr.split_once('=')
                && attr_name.eq_ignore_ascii_case(key_attr) {
                    return format!("{}[@{}={}]", element_name, attr_name, attr_value);
                }
        }
    }

    element_name.to_string()
}

