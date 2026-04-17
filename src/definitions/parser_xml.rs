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

use crate::definitions::types::{DefinitionEntry, DefinitionKind};
use thiserror::Error;

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

/// Check if a file extension is an XML extension we support for on-demand parsing.
pub(crate) fn is_xml_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "xml" | "config" | "csproj" | "props" | "targets" | "manifestxml" | "resx"
    )
}

/// Parse an XML file on-demand and return structured definitions.
///
/// Returns a list of `XmlDefinition` entries representing all XML elements
/// in the file, with parent relationships, signatures, and text content.
///
/// This function does NOT populate the DefinitionIndex — it returns
/// standalone results for immediate use by the handler.
pub(crate) fn parse_xml_on_demand(
    source: &str,
    file_path: &str,
) -> Result<Vec<XmlDefinition>, XmlParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_xml::LANGUAGE_XML.into())
        .map_err(|e| XmlParseError::GrammarLoad(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or(XmlParseError::TreeSitterReturnedNone)?;

    let source_bytes = source.as_bytes();
    let mut defs = Vec::new();

    walk_xml_node(
        tree.root_node(),
        source_bytes,
        file_path,
        None,   // parent_name
        None,   // parent_index
        &[],    // ancestry for signature
        &mut defs,
    );

    // Second pass: determine which elements have child elements
    let parent_indices: Vec<Option<usize>> = defs.iter().map(|d| d.parent_index).collect();
    for pi in &parent_indices {
        if let Some(idx) = pi {
            if *idx < defs.len() {
                defs[*idx].has_child_elements = true;
            }
        }
    }

    Ok(defs)
}

/// Recursively walk the XML AST tree and collect element definitions.
fn walk_xml_node(
    node: tree_sitter::Node,
    source: &[u8],
    file_path: &str,
    parent_name: Option<&str>,
    parent_index: Option<usize>,
    ancestry: &[String],
    defs: &mut Vec<XmlDefinition>,
) {
    let kind = node.kind();

    match kind {
        "element" => {
            // Extract element name from STag or EmptyElemTag child
            if let Some(element_name) = extract_element_name(node, source) {
                let line_start = node.start_position().row as u32 + 1;
                let line_end = node.end_position().row as u32 + 1;

                // Build signature: ancestry path
                let mut sig_parts = ancestry.to_vec();
                let sig_name = build_element_signature_name(node, source, &element_name);
                sig_parts.push(sig_name);
                let signature = sig_parts.join(" > ");

                // Extract XML attributes
                let attributes = extract_xml_attributes(node, source);

                // Extract text content (for leaf elements)
                let text_content = extract_text_content(node, source);

                let def_index = defs.len();
                defs.push(XmlDefinition {
                    entry: DefinitionEntry {
                        file_id: 0, // Not used for on-demand
                        name: element_name.clone(),
                        kind: DefinitionKind::XmlElement,
                        line_start,
                        line_end,
                        parent: parent_name.map(|s| s.to_string()),
                        signature: Some(signature),
                        modifiers: Vec::new(),
                        attributes,
                        base_types: Vec::new(),
                    },
                    text_content,
                    has_child_elements: false, // Will be updated in second pass
                    parent_index,
                });

                // Recurse into children with updated ancestry
                let mut child_ancestry = ancestry.to_vec();
                child_ancestry.push(element_name.clone());

                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk_xml_node(
                        child,
                        source,
                        file_path,
                        Some(&element_name),
                        Some(def_index),
                        &child_ancestry,
                        defs,
                    );
                }
            }
        }
        // EmptyElemTag is always wrapped by an `element` node in tree-sitter-xml,
        // so it's already handled by the `element` branch above. Skip to avoid duplicates.
        _ => {
            // Recurse into non-element nodes (document, content, etc.)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_xml_node(
                    child, source, file_path, parent_name, parent_index, ancestry, defs,
                );
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
/// Only returns content for leaf elements (elements with no child elements).
/// Truncates to 200 characters.
fn extract_text_content(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut has_child_elements = false;
    let mut text_parts: Vec<String> = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "content" => {
                let mut content_cursor = child.walk();
                for content_child in child.children(&mut content_cursor) {
                    match content_child.kind() {
                        "element" | "EmptyElemTag" => {
                            has_child_elements = true;
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
            _ => {}
        }
    }

    if has_child_elements || text_parts.is_empty() {
        return None;
    }

    let combined = text_parts.join(" ");
    // Truncate by character count (not bytes) to avoid slicing in the middle of a
    // UTF-8 multi-byte sequence. Using byte index on non-ASCII text (Cyrillic, CJK,
    // emoji) would cause `byte index N is not a char boundary` panic.
    const MAX_CHARS: usize = 200;
    const TRUNCATE_TO: usize = 197;
    if combined.chars().count() > MAX_CHARS {
        let truncated: String = combined.chars().take(TRUNCATE_TO).collect();
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
            if let Some((attr_name, attr_value)) = attr.split_once('=') {
                if attr_name.eq_ignore_ascii_case(key_attr) {
                    return format!("{}[@{}={}]", element_name, attr_name, attr_value);
                }
            }
        }
    }

    element_name.to_string()
}
