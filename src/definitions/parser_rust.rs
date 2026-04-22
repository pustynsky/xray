//! Rust AST parser using tree-sitter: extracts definitions, call sites, and code stats.

use std::collections::HashMap;

use super::types::*;
use super::tree_sitter_utils::{node_text, find_child_by_kind, find_child_by_field, walk_code_stats, warn_ast_depth_exceeded, MAX_AST_RECURSION_DEPTH, MAX_PARSE_SOURCE_BYTES, PARSE_TIMEOUT_MICROS, RUST_CODE_STATS_CONFIG};

// ─── Main entry point ─────────────────────────────────

pub(crate) fn parse_rust_definitions(
    parser: &mut tree_sitter::Parser,
    source: &str,
    file_id: u32,
) -> ParseResult {
    // PARSE-002: skip oversized sources before tree-sitter allocates ~10× RAM.
    if source.len() > MAX_PARSE_SOURCE_BYTES {
        tracing::warn!(
            target: "xray::parse",
            file_id = file_id,
            size = source.len(),
            limit = MAX_PARSE_SOURCE_BYTES,
            "skipping oversized Rust source"
        );
        return (Vec::new(), Vec::new(), Vec::new());
    }
    // PARSE-001: bound parse wall-clock so a single pathological file cannot
    // pin a worker thread indefinitely.
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => {
            tracing::warn!(
                target: "xray::parse",
                file_id = file_id,
                "tree-sitter Rust parse returned None (timeout or grammar error)"
            );
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };

    let mut defs = Vec::new();
    let source_bytes = source.as_bytes();
    let mut method_nodes: Vec<(usize, tree_sitter::Node)> = Vec::new();
    walk_rust_node(tree.root_node(), source_bytes, file_id, None, &mut defs, &mut method_nodes, 0);

    // Build per-struct field type maps from collected defs
    let mut struct_field_types: HashMap<String, HashMap<String, String>> = HashMap::new();
    for def in &defs {
        if let Some(ref parent) = def.parent
            && def.kind == DefinitionKind::Field
                && let Some(ref sig) = def.signature
                    && let Some((name, type_name)) = parse_rust_field_type(sig) {
                        struct_field_types
                            .entry(parent.clone())
                            .or_default()
                            .insert(name, type_name);
                    }
    }

    // Extract call sites from pre-collected method nodes
    let mut call_sites: Vec<(usize, Vec<CallSite>)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let def = &defs[def_local_idx];
        let parent_name = def.parent.as_deref().unwrap_or("");
        let field_types = struct_field_types.get(parent_name)
            .cloned()
            .unwrap_or_default();

        let calls = extract_rust_call_sites(method_node, source_bytes, parent_name, &field_types);
        if !calls.is_empty() {
            call_sites.push((def_local_idx, calls));
        }
    }

    // Compute code stats
    let call_count_map: HashMap<usize, u16> = call_sites.iter()
        .map(|(idx, calls)| (*idx, calls.len() as u16))
        .collect();

    let mut code_stats_entries: Vec<(usize, CodeStats)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let mut stats = compute_code_stats_rust(method_node, source_bytes);
        stats.call_count = call_count_map.get(&def_local_idx).copied().unwrap_or(0);
        code_stats_entries.push((def_local_idx, stats));
    }

    (defs, call_sites, code_stats_entries)
}

// ─── AST walking ────────────────────────────────────────────────────

fn walk_rust_node<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    file_id: u32,
    parent_name: Option<&str>,
    defs: &mut Vec<DefinitionEntry>,
    method_nodes: &mut Vec<(usize, tree_sitter::Node<'a>)>,
    depth: usize,
) {
    // MINOR-27: hard cap recursion to avoid SIGABRT on adversarial /
    // auto-generated code with pathologically deep AST. In normal Rust
    // code this cap is never hit (typical depth < 30).
    if depth > MAX_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded("rust", node);
        return;
    }
    let kind = node.kind();

    match kind {
        "struct_item" => {
            if let Some(def) = extract_rust_struct_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Extract fields from field_declaration_list
                if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
                    extract_rust_struct_fields(body, source, file_id, &name, defs);
                }
                return;
            }
        }
        "enum_item" => {
            if let Some(def) = extract_rust_enum_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Extract enum variants
                if let Some(body) = find_child_by_kind(node, "enum_variant_list") {
                    extract_rust_enum_variants(body, source, file_id, &name, defs);
                }
                return;
            }
        }
        "trait_item" => {
            if let Some(def) = extract_rust_trait_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Walk trait body for method signatures and default methods
                if let Some(body) = find_child_by_kind(node, "declaration_list") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            match child.kind() {
                                // Trait method with body (default implementation)
                                "function_item" => {
                                    if let Some(mut def) = extract_rust_function_def(child, source, file_id, Some(&name)) {
                                        def.kind = DefinitionKind::Method;
                                        let idx = defs.len();
                                        defs.push(def);
                                        method_nodes.push((idx, child));
                                    }
                                }
                                // Trait method without body (signature only)
                                "function_signature_item" => {
                                    if let Some(mut def) = extract_rust_function_def(child, source, file_id, Some(&name)) {
                                        def.kind = DefinitionKind::Method;
                                        defs.push(def);
                                        // No method_nodes entry — no body to extract calls from
                                    }
                                }
                                _ => {
                                    walk_rust_node(child, source, file_id, Some(&name), defs, method_nodes, depth + 1);
                                }
                            }
                        }
                    }
                }
                return;
            }
        }
        "impl_item" => {
            // Extract the struct name and optional trait name from impl block
            let (impl_struct_name, trait_name) = extract_impl_names(node, source);
            if let Some(ref struct_name) = impl_struct_name {
                // If this is a trait impl, register base_types on the struct
                if let Some(ref trait_n) = trait_name {
                    // Find or create a synthetic entry? No — we just set base_types
                    // on methods inside the impl block via walk. The struct itself
                    // already has its definition from struct_item.
                    // We'll pass trait_name info through and handle in method extraction.
                    let _ = trait_n; // used below when iterating children
                }

                // Walk impl body — methods inside get parent = struct_name
                if let Some(body) = find_child_by_kind(node, "declaration_list") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            if child.kind() == "function_item" {
                                if let Some(mut def) = extract_rust_function_def(child, source, file_id, Some(struct_name)) {
                                    // Determine kind: Constructor if name is "new" or "default"
                                    let fn_name = def.name.as_str();
                                    if fn_name == "new" || fn_name == "default" {
                                        def.kind = DefinitionKind::Constructor;
                                    } else {
                                        def.kind = DefinitionKind::Method;
                                    }
                                    // Add trait as base_type if this is a trait impl
                                    if let Some(ref tn) = trait_name {
                                        def.base_types.push(tn.clone());
                                    }
                                    let idx = defs.len();
                                    defs.push(def);
                                    method_nodes.push((idx, child));
                                }
                            } else {
                                walk_rust_node(child, source, file_id, Some(struct_name), defs, method_nodes, depth + 1);
                            }
                        }
                    }
                }
            }
            return;
        }
        "function_item" => {
            // Top-level function (not inside impl — those are handled above)
            if let Some(def) = extract_rust_function_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                method_nodes.push((idx, node));
                return;
            }
        }
        "const_item" => {
            if let Some(def) = extract_rust_const_static_def(node, source, file_id, parent_name, false) {
                defs.push(def);
                return;
            }
        }
        "static_item" => {
            if let Some(def) = extract_rust_const_static_def(node, source, file_id, parent_name, true) {
                defs.push(def);
                return;
            }
        }
        "type_item" => {
            if let Some(def) = extract_rust_type_alias_def(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        _ => {}
    }

    // Default: recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_rust_node(child, source, file_id, parent_name, defs, method_nodes, depth + 1);
        }
    }
}

// ─── Modifier and attribute extraction ──────────────────────────────

fn extract_rust_modifiers(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut modifiers = Vec::new();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "visibility_modifier" => {
                    modifiers.push(node_text(child, source).to_string());
                }
                "mutable_specifier" => {
                    modifiers.push("mut".to_string());
                }
                _ => {
                    let text = node_text(child, source);
                    match text {
                        "async" | "unsafe" | "const" | "static" | "pub"
                            if !modifiers.iter().any(|m| m == text) =>
                        {
                            modifiers.push(text.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    modifiers
}

fn extract_rust_attributes(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut attributes = Vec::new();
    // Look for attribute_item nodes that are siblings preceding this node
    // In tree-sitter-rust, attributes are children of the parent, not of the item itself
    // But for items inside declaration_list, attributes ARE children of the item
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "attribute_item" {
                let text = node_text(child, source).trim();
                // Strip #[ and ]
                let inner = text.strip_prefix("#[")
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(text);
                if !inner.is_empty() {
                    attributes.push(inner.to_string());
                }
            }
    }

    // Also check preceding siblings (attributes appear before the item at same level)
    if let Some(parent) = node.parent() {
        let node_id = node.id();
        for i in 0..parent.child_count() {
            if let Some(child) = parent.child(i) {
                if child.id() == node_id {
                    break; // stop when we reach the node itself
                }
                if child.kind() == "attribute_item" {
                    // Check if this attribute is immediately before our node
                    // (no other named node between them)
                    let next_named = find_next_named_sibling(parent, i + 1);
                    if next_named.is_some_and(|n| n.id() == node_id) {
                        let text = node_text(child, source).trim();
                        let inner = text.strip_prefix("#[")
                            .and_then(|s| s.strip_suffix(']'))
                            .unwrap_or(text);
                        if !inner.is_empty() && !attributes.contains(&inner.to_string()) {
                            attributes.push(inner.to_string());
                        }
                    }
                }
            }
        }
    }

    attributes
}

fn find_next_named_sibling(parent: tree_sitter::Node, start_idx: usize) -> Option<tree_sitter::Node> {
    for i in start_idx..parent.child_count() {
        if let Some(child) = parent.child(i)
            && child.is_named() && child.kind() != "attribute_item" && child.kind() != "line_comment" && child.kind() != "block_comment" {
                return Some(child);
            }
    }
    None
}

// ─── impl block name extraction ─────────────────────────────────────

/// Extract (struct_name, trait_name) from an impl_item.
/// For `impl Foo { ... }` → (Some("Foo"), None)
/// For `impl Display for Foo { ... }` → (Some("Foo"), Some("Display"))
fn extract_impl_names(node: tree_sitter::Node, source: &[u8]) -> (Option<String>, Option<String>) {
    // tree-sitter-rust impl_item fields:
    //   type: the type being implemented (Foo)
    //   trait: the trait (if trait impl)
    let type_node = find_child_by_field(node, "type");
    let trait_node = find_child_by_field(node, "trait");

    let struct_name = type_node.map(|n| {
        let text = node_text(n, source).trim();
        // Strip generic parameters: Foo<T> → Foo
        text.split('<').next().unwrap_or(text).to_string()
    });

    let trait_name = trait_node.map(|n| {
        let text = node_text(n, source).trim();
        text.split('<').next().unwrap_or(text).to_string()
    });

    (struct_name, trait_name)
}

// ─── Definition extraction ──────────────────────────────────────────

fn extract_rust_struct_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);
    let sig = build_rust_signature_before_body(node, source);

    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Struct,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_rust_struct_fields(
    field_list: tree_sitter::Node, source: &[u8], file_id: u32,
    parent_name: &str, defs: &mut Vec<DefinitionEntry>,
) {
    for i in 0..field_list.child_count() {
        if let Some(child) = field_list.child(i)
            && child.kind() == "field_declaration" {
                let name_node = find_child_by_field(child, "name");
                if let Some(name_n) = name_node {
                    let name = node_text(name_n, source).to_string();
                    let modifiers = extract_rust_modifiers(child, source);
                    let attributes = extract_rust_attributes(child, source);
                    let sig = node_text(child, source).split_whitespace()
                        .collect::<Vec<_>>().join(" ");
                    defs.push(DefinitionEntry {
                        file_id, name, kind: DefinitionKind::Field,
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        parent: Some(parent_name.to_string()),
                        signature: Some(sig), modifiers, attributes,
                        base_types: Vec::new(),
                    });
                }
            }
    }
}

fn extract_rust_enum_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);
    let sig = build_rust_signature_before_body(node, source);

    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Enum,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_rust_enum_variants(
    variant_list: tree_sitter::Node, source: &[u8], file_id: u32,
    parent_name: &str, defs: &mut Vec<DefinitionEntry>,
) {
    for i in 0..variant_list.child_count() {
        if let Some(child) = variant_list.child(i)
            && child.kind() == "enum_variant" {
                let name_node = find_child_by_field(child, "name");
                if let Some(name_n) = name_node {
                    let name = node_text(name_n, source).to_string();
                    let attributes = extract_rust_attributes(child, source);
                    defs.push(DefinitionEntry {
                        file_id, name, kind: DefinitionKind::EnumMember,
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        parent: Some(parent_name.to_string()),
                        signature: None, modifiers: Vec::new(), attributes,
                        base_types: Vec::new(),
                    });
                }
            }
    }
}

fn extract_rust_trait_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);
    let sig = build_rust_signature_before_body(node, source);

    // Extract supertraits as base_types
    let mut base_types = Vec::new();
    if let Some(bounds) = find_child_by_kind(node, "trait_bounds") {
        for i in 0..bounds.child_count() {
            if let Some(child) = bounds.child(i)
                && child.is_named() {
                    let bt = node_text(child, source).trim();
                    let bt_base = bt.split('<').next().unwrap_or(bt);
                    if !bt_base.is_empty() {
                        base_types.push(bt_base.to_string());
                    }
                }
        }
    }

    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Interface,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types,
    })
}

fn extract_rust_function_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);
    let sig = build_rust_function_signature(node, source);

    // Default kind is Function; caller (impl_item handler) may override to Method/Constructor
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Function,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_rust_const_static_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
    _is_static: bool,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);

    // Build signature: everything up to the value (= ...)
    let sig = {
        let start = node.start_byte();
        let mut end = node.end_byte();
        // Find '=' to truncate the value part
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && node_text(child, source) == "=" {
                    end = child.start_byte();
                    break;
                }
        }
        // PARSE-007: use lossy decoding so a multi-byte codepoint that straddles
        // the [start, end) byte range yields U+FFFD instead of an empty signature.
        let text = String::from_utf8_lossy(&source[start..end]);
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Variable,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_rust_type_alias_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_rust_modifiers(node, source);
    let attributes = extract_rust_attributes(node, source);
    let sig = node_text(node, source).split_whitespace()
        .collect::<Vec<_>>().join(" ");

    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::TypeAlias,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

// ─── Signature building ─────────────────────────────────────────────

fn build_rust_signature_before_body(node: tree_sitter::Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let mut end = node.end_byte();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "field_declaration_list" | "enum_variant_list" | "declaration_list" | "{" => {
                    end = child.start_byte();
                    break;
                }
                _ => {}
            }
        }
    }
    // PARSE-007: lossy decode keeps multi-byte identifiers (Cyrillic / CJK)
    // legible instead of silently substituting an empty string.
    let text = String::from_utf8_lossy(&source[start..end]);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build_rust_function_signature(node: tree_sitter::Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let mut end = node.end_byte();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && (child.kind() == "block" || child.kind() == ";") {
                end = child.start_byte();
                break;
            }
    }
    // PARSE-007: lossy decode prevents multi-byte identifier truncation
    // from collapsing the signature to an empty string.
    let text = String::from_utf8_lossy(&source[start..end]);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ─── Field type parsing ─────────────────────────────────────────────

/// Parse a Rust field signature like "pub name: String" into (field_name, base_type).
fn parse_rust_field_type(sig: &str) -> Option<(String, String)> {
    let colon_pos = sig.find(':')?;
    let before_colon = sig[..colon_pos].trim();
    let after_colon = sig[colon_pos + 1..].trim();

    // Field name is the last word before the colon (may have `pub` prefix)
    let name = before_colon.split_whitespace().last()?.to_string();
    // Base type: strip generics
    let base_type = after_colon.split('<').next().unwrap_or(after_colon)
        .split('(').next().unwrap_or(after_colon)
        .trim().to_string();

    if !name.is_empty() && !base_type.is_empty() {
        Some((name, base_type))
    } else {
        None
    }
}

// ─── Call site extraction ───────────────────────────────────────────

fn extract_rust_call_sites(
    method_node: tree_sitter::Node,
    source: &[u8],
    parent_name: &str,
    field_types: &HashMap<String, String>,
) -> Vec<CallSite> {
    let mut calls = Vec::new();

    let body = find_child_by_kind(method_node, "block");
    if let Some(body_node) = body {
        walk_rust_for_calls(body_node, source, parent_name, field_types, &mut calls);
    }

    calls.sort_by(|a, b| a.line.cmp(&b.line)
        .then_with(|| a.method_name.cmp(&b.method_name))
        .then_with(|| a.receiver_type.cmp(&b.receiver_type)));
    calls.dedup_by(|a, b| a.line == b.line && a.method_name == b.method_name && a.receiver_type == b.receiver_type);

    calls
}

fn walk_rust_for_calls(
    node: tree_sitter::Node,
    source: &[u8],
    parent_name: &str,
    field_types: &HashMap<String, String>,
    calls: &mut Vec<CallSite>,
) {
    match node.kind() {
        "call_expression" => {
            if let Some(call) = extract_rust_call(node, source, parent_name, field_types) {
                calls.push(call);
            }
            // Recurse into all children for chained/nested calls
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_rust_for_calls(child, source, parent_name, field_types, calls);
                }
            }
            return;
        }
        // Skip macro invocations
        "macro_invocation" => return,
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_rust_for_calls(child, source, parent_name, field_types, calls);
        }
    }
}

fn extract_rust_call(
    node: tree_sitter::Node,
    source: &[u8],
    parent_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<CallSite> {
    // call_expression: child(0) = function expression, child(1) = arguments
    let func = node.child(0)?;
    let line = node.start_position().row as u32 + 1;

    match func.kind() {
        // Simple function call: my_function(args)
        "identifier" => {
            let method_name = node_text(func, source).to_string();
            Some(CallSite { method_name, receiver_type: None, line, receiver_is_generic: false })
        }
        // Method call: receiver.method(args) — tree-sitter-rust uses field_expression
        "field_expression" => {
            extract_rust_field_expression_call(func, source, parent_name, field_types, line)
        }
        // Static/path call: Type::method(args) or module::function(args)
        "scoped_identifier" => {
            extract_rust_scoped_call(func, source, line)
        }
        // Generic function call: function::<T>(args)
        "generic_function" => {
            // generic_function has child(0) = function identifier/scoped_identifier
            let inner_func = func.child(0)?;
            match inner_func.kind() {
                "identifier" => {
                    let method_name = node_text(inner_func, source).to_string();
                    Some(CallSite { method_name, receiver_type: None, line, receiver_is_generic: false })
                }
                "scoped_identifier" => {
                    extract_rust_scoped_call(inner_func, source, line)
                }
                "field_expression" => {
                    extract_rust_field_expression_call(inner_func, source, parent_name, field_types, line)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_rust_field_expression_call(
    node: tree_sitter::Node,
    source: &[u8],
    parent_name: &str,
    field_types: &HashMap<String, String>,
    line: u32,
) -> Option<CallSite> {
    // field_expression: value.field
    let value_node = find_child_by_field(node, "value").or_else(|| node.child(0))?;
    let field_node = find_child_by_field(node, "field").or_else(|| {
        // Last named child is typically the field
        let count = node.child_count();
        if count > 0 { node.child(count - 1) } else { None }
    })?;

    let method_name = node_text(field_node, source).to_string();
    let receiver_type = resolve_rust_receiver(value_node, source, parent_name, field_types);

    Some(CallSite { method_name, receiver_type, line, receiver_is_generic: false })
}

fn extract_rust_scoped_call(
    node: tree_sitter::Node,
    source: &[u8],
    line: u32,
) -> Option<CallSite> {
    // scoped_identifier: path::name, e.g., HashMap::new
    let name_node = find_child_by_field(node, "name")?;
    let method_name = node_text(name_node, source).to_string();

    let path_node = find_child_by_field(node, "path")?;
    let path_text = node_text(path_node, source).trim();
    // Get the last segment of the path as the receiver type
    let receiver = path_text.rsplit("::").next().unwrap_or(path_text);
    let receiver_base = receiver.split('<').next().unwrap_or(receiver).trim();

    let receiver_type = if !receiver_base.is_empty() {
        Some(receiver_base.to_string())
    } else {
        None
    };

    Some(CallSite { method_name, receiver_type, line, receiver_is_generic: false })
}

fn resolve_rust_receiver(
    receiver: tree_sitter::Node,
    source: &[u8],
    parent_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<String> {
    let text = node_text(receiver, source).trim();

    match receiver.kind() {
        "self" | "identifier" if text == "self" => {
            if parent_name.is_empty() { None } else { Some(parent_name.to_string()) }
        }
        "identifier" => {
            // Try to resolve via field types
            if let Some(type_name) = field_types.get(text) {
                Some(type_name.clone())
            } else {
                // Preserve receiver name regardless of case
                Some(text.to_string())
            }
        }
        "field_expression" => {
            // self.field.method() — extract the field name, look up type
            let value = find_child_by_field(receiver, "value").or_else(|| receiver.child(0));
            let field = find_child_by_field(receiver, "field");

            if let (Some(val), Some(fld)) = (value, field) {
                let val_text = node_text(val, source).trim();
                let fld_text = node_text(fld, source).trim();

                if val_text == "self" || val.kind() == "self" {
                    // self.field → look up field type
                    if let Some(type_name) = field_types.get(fld_text) {
                        return Some(type_name.clone());
                    }
                    return Some(fld_text.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

// ─── Code stats computation ─────────────────────────────────────────

fn compute_code_stats_rust(method_node: tree_sitter::Node, source: &[u8]) -> CodeStats {
    let mut stats = CodeStats {
        cyclomatic_complexity: 1, // base complexity
        param_count: count_rust_parameters(method_node, source),
        ..Default::default()
    };

    // Walk body using unified data-driven walker
    if let Some(body) = find_child_by_kind(method_node, "block") {
        walk_code_stats(body, source, 0, 0, &mut stats, &RUST_CODE_STATS_CONFIG);
    }

    stats
}

fn count_rust_parameters(method_node: tree_sitter::Node, source: &[u8]) -> u8 {
    let params = match find_child_by_field(method_node, "parameters")
        .or_else(|| find_child_by_kind(method_node, "parameters"))
    {
        Some(p) => p,
        None => return 0,
    };

    let mut count = 0u8;
    for i in 0..params.child_count() {
        if let Some(child) = params.child(i) {
            match child.kind() {
                "parameter" => {
                    count += 1;
                }
                "self_parameter" => {
                    // self, &self, &mut self — do NOT count
                }
                _ => {
                    // Check for `self` text in case tree-sitter uses different node kind
                    let text = node_text(child, source).trim();
                    if child.is_named() && text != "self" && text != "&self" && text != "&mut self" && text != "mut self" {
                        // Only count if it looks like a parameter (has a colon for type annotation)
                        if text.contains(':') {
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    count
}

// walk_code_stats_rust removed — replaced by unified walk_code_stats() in tree_sitter_utils.rs
// with RUST_CODE_STATS_CONFIG.