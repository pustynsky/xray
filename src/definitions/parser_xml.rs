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
use thiserror::Error;

/// Maximum depth for the recursive walker. Elements deeper than this are skipped
/// and a warning is recorded in `ParseResult.warnings`. Chosen to be well below
/// Rust's default 8 MB stack (each frame is ~500 B here, so 1024 levels ≈ 0.5 MB).
const MAX_RECURSION_DEPTH: usize = 1024;

/// Character count at which text content is truncated (WALKER-aware, UTF-8 safe).
const TEXT_CONTENT_MAX_CHARS: usize = 200;
const TEXT_CONTENT_TRUNCATE_TO: usize = 197;

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
    let mut ctx = WalkCtx {
        defs: Vec::new(),
        ancestry: Vec::new(),
        warnings: Vec::new(),
    };

    walk_xml_node(tree.root_node(), source_bytes, None, 0, &mut ctx);

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
    if depth > MAX_RECURSION_DEPTH {
        // Depth-limit tripwire (WALKER-3). We stop descending here; any elements
        // deeper than this are silently dropped, but we record a warning so the
        // handler can surface "results may be incomplete".
        ctx.warnings.push(format!(
            "XML nesting exceeded {} levels; subtree at line {} truncated.",
            MAX_RECURSION_DEPTH,
            node.start_position().row + 1
        ));
        return;
    }

    let kind = node.kind();

    match kind {
        "element" => {
            // Extract element name from STag or EmptyElemTag child
            if let Some(element_name) = extract_element_name(node, source) {
                let line_start = node.start_position().row as u32 + 1;
                let line_end = node.end_position().row as u32 + 1;

                // Build signature: ancestry path. We clone the live stack once
                // here (only when pushing a def), not per-level.
                let sig_name = build_element_signature_name(node, source, &element_name);
                let mut sig_parts: Vec<String> = ctx.ancestry.clone();
                sig_parts.push(sig_name);
                let signature = sig_parts.join(" > ");

                // Extract XML attributes
                let attributes = extract_xml_attributes(node, source);

                // Extract text content (for leaf elements). Early-exits internally
                // on the first element/EmptyElemTag child (WALKER-7), so block
                // elements do not pay for a full CharData sweep.
                let text_content = extract_text_content(node, source);

                // Parent name is the current top of the ancestry stack — WALKER-4
                // replaced the separate `parent_name: Option<&str>` argument with
                // this lookup for a single source of truth.
                let parent_name = ctx.ancestry.last().cloned();

                let def_index = ctx.defs.len();
                ctx.defs.push(XmlDefinition {
                    entry: DefinitionEntry {
                        file_id: 0, // Not used for on-demand
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
                    has_child_elements: false, // Will be updated after walk
                    parent_index,
                });

                // Persistent-stack recursion (WALKER-1): push + pop instead of
                // cloning the full ancestry Vec on every level.
                ctx.ancestry.push(element_name);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk_xml_node(child, source, Some(def_index), depth + 1, ctx);
                }
                ctx.ancestry.pop();
            } else {
                // WALKER-2: tree-sitter did not recover a usable STag/EmptyElemTag
                // for this `element` node (malformed XML — e.g. unterminated tag,
                // junk before opening angle bracket). We record a warning, skip
                // adding a definition, but still descend so nested well-formed
                // elements inside are not silently lost.
                ctx.warnings.push(format!(
                    "Could not extract element name at line {}; children still walked.",
                    node.start_position().row + 1
                ));
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk_xml_node(child, source, parent_index, depth + 1, ctx);
                }
            }
        }
        // EmptyElemTag is always wrapped by an `element` node in tree-sitter-xml,
        // so it's already handled by the `element` branch above. Skip to avoid duplicates.
        _ => {
            // Non-element nodes (document, prolog, content wrappers, misc) are
            // transparent for parent tracking — their element children inherit
            // the current `parent_index`/`ancestry` as if the wrapper weren't there.
            // Depth counter still increments to keep the MAX_RECURSION_DEPTH budget
            // tight against adversarial inputs (WALKER-5: comment for the reader).
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_xml_node(child, source, parent_index, depth + 1, ctx);
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
/// `CharData` from other siblings would be wasted work. For large XML blocks
/// with many CharData/comment/PI siblings this avoids a full content sweep.
///
/// NOTE: XML entity escapes in CharData (`&amp;`, `&lt;`, numeric refs) are
/// **not decoded**. They appear verbatim in the returned string.
fn extract_text_content(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut text_parts: Vec<String> = Vec::new();

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
                "CharData" => {
                    if let Ok(text) = content_child.utf8_text(source) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            text_parts.push(trimmed.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if text_parts.is_empty() {
        return None;
    }

    let combined = text_parts.join(" ");
    // Truncate by character count (not bytes) to avoid slicing in the middle of a
    // UTF-8 multi-byte sequence. Using byte index on non-ASCII text (Cyrillic, CJK,
    // emoji) would cause `byte index N is not a char boundary` panic.
    if combined.chars().count() > TEXT_CONTENT_MAX_CHARS {
        let truncated: String = combined.chars().take(TEXT_CONTENT_TRUNCATE_TO).collect();
        Some(format!("{}...", truncated))
    } else {
        Some(combined)
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

