//! TypeScript AST parser using tree-sitter: extracts definitions and call sites.

use std::collections::HashMap;

use super::types::*;
use super::tree_sitter_utils::{find_child_by_kind, find_descendant_by_kind, find_child_by_field, count_named_children, walk_code_stats, TYPESCRIPT_CODE_STATS_CONFIG};

// ─── Main entry point ───────────────────────────────────────────────

pub(crate) fn parse_typescript_definitions(
    parser: &mut tree_sitter::Parser,
    source: &str,
    file_id: u32,
) -> ParseResult {
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => {
            eprintln!("[def-index] WARNING: tree-sitter TS parse returned None for file_id={}", file_id);
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };

    let mut defs = Vec::new();
    let mut method_nodes: Vec<(usize, tree_sitter::Node)> = Vec::new();
    walk_typescript_node_collecting(tree.root_node(), source, file_id, None, &mut defs, &mut method_nodes);

    // Build per-class field type maps from the collected defs
    let mut class_field_types: HashMap<String, HashMap<String, String>> = HashMap::new();

    for def in &defs {
        if let Some(ref parent) = def.parent
            && def.kind == DefinitionKind::Field
                && let Some(ref sig) = def.signature
                    && let Some((name, type_name)) = parse_ts_field_type(sig) {
                        class_field_types
                            .entry(parent.clone())
                            .or_default()
                            .insert(name, type_name);
                    }
    }

    // Extract constructor parameter types as field types (DI pattern)
    for def in &defs {
        if def.kind == DefinitionKind::Constructor
            && let Some(ref parent) = def.parent
                && let Some(ref sig) = def.signature {
                    let param_types = extract_ts_constructor_param_types(sig);
                    let field_map = class_field_types.entry(parent.clone()).or_default();
                    for (param_name, param_type) in param_types {
                        field_map.entry(param_name).or_insert(param_type);
                    }
                }
    }

    // Extract Angular inject() patterns as field types
    extract_ts_inject_types(tree.root_node(), source, &mut class_field_types);

    // Extract call sites from pre-collected method nodes
    let mut call_sites: Vec<(usize, Vec<CallSite>)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let def = &defs[def_local_idx];
        let parent_name = def.parent.as_deref().unwrap_or("");
        let field_types = class_field_types.get(parent_name)
            .cloned()
            .unwrap_or_default();

        let calls = extract_ts_call_sites(method_node, source, parent_name, &field_types);
        if !calls.is_empty() {
            call_sites.push((def_local_idx, calls));
        }
    }

    // Compute code stats for pre-collected method/constructor/function nodes
    let call_count_map: HashMap<usize, u16> = call_sites.iter()
        .map(|(idx, calls)| (*idx, calls.len() as u16))
        .collect();

    let mut code_stats_entries: Vec<(usize, CodeStats)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let mut stats = compute_code_stats_typescript(method_node, source);
        stats.call_count = call_count_map.get(&def_local_idx).copied().unwrap_or(0);
        code_stats_entries.push((def_local_idx, stats));
    }

    (defs, call_sites, code_stats_entries)
}

// ─── AST walking ────────────────────────────────────────────────────

fn walk_typescript_node_collecting<'a>(
    node: tree_sitter::Node<'a>,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
    defs: &mut Vec<DefinitionEntry>,
    method_nodes: &mut Vec<(usize, tree_sitter::Node<'a>)>,
) {
    let kind = node.kind();

    match kind {
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(def) = extract_ts_class_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Walk into class body
                if let Some(body) = find_child_by_kind(node, "class_body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            walk_typescript_node_collecting(child, source, file_id, Some(&name), defs, method_nodes);
                        }
                    }
                }
                return;
            }
        }
        "interface_declaration" => {
            if let Some(def) = extract_ts_interface_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Walk into interface body for property signatures
                if let Some(body) = find_child_by_kind(node, "object_type")
                    .or_else(|| find_child_by_kind(node, "interface_body"))
                {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            walk_typescript_node_collecting(child, source, file_id, Some(&name), defs, method_nodes);
                        }
                    }
                }
                return;
            }
        }
        "enum_declaration" => {
            if let Some(def) = extract_ts_enum_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                // Walk into enum body for members
                if let Some(body) = find_child_by_kind(node, "enum_body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            walk_typescript_node_collecting(child, source, file_id, Some(&name), defs, method_nodes);
                        }
                    }
                }
                return;
            }
        }
        "function_declaration" => {
            if let Some(def) = extract_ts_function_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                method_nodes.push((idx, node));
                return;
            }
        }
        "method_definition" => {
            if let Some(def) = extract_ts_method_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                method_nodes.push((idx, node));
                return;
            }
        }
        "abstract_method_signature" => {
            if let Some(def) = extract_ts_abstract_method_sig(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "method_signature" => {
            if let Some(def) = extract_ts_method_signature(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "public_field_definition" => {
            if let Some(def) = extract_ts_field_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                // Collect arrow function fields for call-site extraction
                if has_arrow_function_value(node) {
                    method_nodes.push((idx, node));
                }
                return;
            }
        }
        "property_signature" => {
            if let Some(def) = extract_ts_property_signature(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "type_alias_declaration" => {
            if let Some(def) = extract_ts_type_alias_def(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "lexical_declaration" => {
            // Only extract exported variable declarations
            if is_exported(node) {
                extract_ts_variable_defs(node, source, file_id, parent_name, defs);
                return;
            }
        }
        "enum_member" | "enum_assignment" => {
            if let Some(def) = extract_ts_enum_member(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        // In tree-sitter-typescript, enum members can also be plain property_identifier
        // nodes inside enum_body (without an enum_member wrapper)
        "property_identifier" if is_inside_enum_body(node) => {
            let name = node_text(node, source).to_string();
            if !name.is_empty() {
                defs.push(DefinitionEntry {
                    file_id,
                    name,
                    kind: DefinitionKind::EnumMember,
                    line_start: node.start_position().row as u32 + 1,
                    line_end: node.end_position().row as u32 + 1,
                    parent: parent_name.map(|s| s.to_string()),
                    signature: None,
                    modifiers: Vec::new(),
                    attributes: Vec::new(),
                    base_types: Vec::new(),
                });
                return;
            }
        }
        // For export_statement, walk into the child declaration
        "export_statement" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_typescript_node_collecting(child, source, file_id, parent_name, defs, method_nodes);
                }
            }
            return;
        }
        _ => {}
    }

    // Default: recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_typescript_node_collecting(child, source, file_id, parent_name, defs, method_nodes);
        }
    }
}

// ─── Helper utilities ───────────────────────────────────────────────

/// TypeScript-specific wrapper for `node_text` that accepts `&str` source.
/// Delegates to the shared `tree_sitter_utils::node_text` with `source.as_bytes()`.
/// This avoids changing 50+ call sites that pass `source: &str`.
fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    super::tree_sitter_utils::node_text(node, source.as_bytes())
}

/// Check if a node is exported (its parent is an export_statement).
fn is_exported(node: tree_sitter::Node) -> bool {
    if let Some(parent) = node.parent() {
        return parent.kind() == "export_statement";
    }
    false
}

/// Extract modifiers from a TypeScript node.
/// Handles: accessibility_modifier (public/private/protected), static, async,
/// abstract, readonly, export, override.
fn extract_ts_modifiers(node: tree_sitter::Node, source: &str) -> Vec<String> {
    let mut modifiers = Vec::new();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "accessibility_modifier" => {
                    modifiers.push(node_text(child, source).to_string());
                }
                "static" | "async" | "abstract" | "readonly" | "override" | "declare" | "const" => {
                    modifiers.push(node_text(child, source).to_string());
                }
                _ => {}
            }
        }
    }
    // Check if exported
    if is_exported(node) {
        modifiers.push("export".to_string());
    }
    modifiers
}

/// Extract decorators from a TypeScript node (equivalent to C# attributes).
/// Also checks the parent `export_statement` for decorators, because tree-sitter-typescript
/// places decorators as siblings of the class_declaration inside export_statement:
///   export_statement → [decorator, class_declaration]
fn extract_ts_decorators(node: tree_sitter::Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    // Check direct children of this node
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "decorator" {
                let text = node_text(child, source);
                let trimmed = text.strip_prefix('@').unwrap_or(text).to_string();
                decorators.push(trimmed);
            }
    }
    // If no decorators found and parent is export_statement, check parent's children
    // (tree-sitter-typescript places decorators as siblings inside export_statement)
    if decorators.is_empty()
        && let Some(parent) = node.parent()
            && parent.kind() == "export_statement" {
                for i in 0..parent.child_count() {
                    if let Some(child) = parent.child(i)
                        && child.kind() == "decorator" {
                            let text = node_text(child, source);
                            let trimmed = text.strip_prefix('@').unwrap_or(text).to_string();
                            decorators.push(trimmed);
                        }
                }
            }
    decorators
}

/// Extract Angular @Component metadata from decorator text.
/// Input — decorator text without `@`:
///   "Component({selector: 'dashboard-embed', templateUrl: './file.html'})"
/// Returns (selector, templateUrl) if found.
pub(crate) fn extract_component_metadata(decorator_text: &str) -> Option<(String, Option<String>)> {
    if !decorator_text.starts_with("Component(") {
        return None;
    }
    let selector = extract_decorator_string_property(decorator_text, "selector")?;
    let template_url = extract_decorator_string_property(decorator_text, "templateUrl");
    Some((selector, template_url))
}

/// Extract a string property value from decorator text.
/// Looks for: propertyName: 'value' or propertyName: "value"
fn extract_decorator_string_property(text: &str, property: &str) -> Option<String> {
    let search = format!("{}:", property);
    let pos = text.find(&search)?;
    let after_colon = &text[pos + search.len()..];
    let trimmed = after_colon.trim_start();
    let quote_char = trimmed.chars().next()?;
    if quote_char != '\'' && quote_char != '"' { return None; }
    let inner = &trimmed[1..];
    let end = inner.find(quote_char)?;
    let value = &inner[..end];
    if value.is_empty() { return None; }
    Some(value.to_string())
}

/// Extract base types / heritage (extends/implements) from a class or interface.
fn extract_ts_heritage(node: tree_sitter::Node, source: &str) -> Vec<String> {
    let mut base_types = Vec::new();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "class_heritage" | "extends_clause" | "implements_clause"
                | "extends_type_clause" => {
                    // Walk the clause children to find type identifiers
                    for j in 0..child.child_count() {
                        if let Some(type_node) = child.child(j) {
                            match type_node.kind() {
                                // In class_heritage, there may be nested extends_clause/implements_clause
                                "extends_clause" | "implements_clause" => {
                                    for k in 0..type_node.child_count() {
                                        if let Some(t) = type_node.child(k)
                                            && t.is_named() && t.kind() != "extends" && t.kind() != "implements" {
                                                base_types.push(node_text(t, source).to_string());
                                            }
                                    }
                                }
                                _ if type_node.is_named()
                                    && type_node.kind() != "extends"
                                    && type_node.kind() != "implements" =>
                                {
                                    base_types.push(node_text(type_node, source).to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    base_types
}

/// Extract type annotation string from a node (looks for type_annotation child).
fn extract_type_annotation(node: tree_sitter::Node, source: &str) -> Option<String> {
    find_child_by_kind(node, "type_annotation").map(|ta| {
        // type_annotation is ": Type", we want the Type part
        let text = node_text(ta, source).trim();
        // Strip leading ':'
        text.strip_prefix(':').unwrap_or(text).trim().to_string()
    })
}

/// Extract formal parameters text from a function/method node.
fn extract_params_text(node: tree_sitter::Node, source: &str) -> Option<String> {
    find_child_by_kind(node, "formal_parameters").map(|params| {
        node_text(params, source).to_string()
    })
}

/// Build a signature for a function/method-like declaration.
fn build_function_signature(
    name: &str,
    params: Option<&str>,
    return_type: Option<&str>,
    prefix_modifiers: &[String],
) -> String {
    let mut sig = String::new();
    for m in prefix_modifiers {
        if matches!(m.as_str(), "async" | "static" | "abstract" | "export") {
            sig.push_str(m);
            sig.push(' ');
        }
    }
    sig.push_str(name);
    if let Some(p) = params {
        sig.push_str(p);
    } else {
        sig.push_str("()");
    }
    if let Some(rt) = return_type {
        sig.push_str(": ");
        sig.push_str(rt);
    }
    sig
}

// ─── Definition extraction helpers ──────────────────────────────────

fn extract_ts_class_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let mut modifiers = extract_ts_modifiers(node, source);
    let decorators = extract_ts_decorators(node, source);
    let base_types = extract_ts_heritage(node, source);

    // Add "abstract" for abstract_class_declaration if not already present
    if node.kind() == "abstract_class_declaration" && !modifiers.contains(&"abstract".to_string()) {
        modifiers.push("abstract".to_string());
    }

    // Build signature: everything up to the class body
    let sig = build_type_signature(node, source);

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Class,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: decorators,
        base_types,
    })
}

fn extract_ts_interface_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);
    let base_types = extract_ts_heritage(node, source);
    let sig = build_type_signature(node, source);

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Interface,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types,
    })
}

fn extract_ts_enum_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);
    let sig = build_type_signature(node, source);

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Enum,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

fn extract_ts_function_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);
    let decorators = extract_ts_decorators(node, source);
    let params = extract_params_text(node, source);
    let return_type = extract_type_annotation(node, source);
    let sig = build_function_signature(
        &name,
        params.as_deref(),
        return_type.as_deref(),
        &modifiers,
    );

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Function,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: decorators,
        base_types: Vec::new(),
    })
}

fn extract_ts_method_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();

    // Detect constructor
    let is_constructor = name == "constructor";
    let kind = if is_constructor {
        DefinitionKind::Constructor
    } else {
        DefinitionKind::Method
    };

    let modifiers = extract_ts_modifiers(node, source);
    let decorators = extract_ts_decorators(node, source);
    let params = extract_params_text(node, source);
    let return_type = extract_type_annotation(node, source);
    let sig = build_function_signature(
        &name,
        params.as_deref(),
        return_type.as_deref(),
        &modifiers,
    );

    Some(DefinitionEntry {
        file_id,
        name,
        kind,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: decorators,
        base_types: Vec::new(),
    })
}

fn extract_ts_field_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")
        .or_else(|| find_child_by_kind(node, "property_identifier"))?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);
    let decorators = extract_ts_decorators(node, source);
    let type_ann = extract_type_annotation(node, source);
    let sig = if let Some(ref t) = type_ann {
        format!("{}: {}", name, t)
    } else {
        name.clone()
    };

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Field,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: decorators,
        base_types: Vec::new(),
    })
}

fn extract_ts_property_signature(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")
        .or_else(|| find_child_by_kind(node, "property_identifier"))?;
    let name = node_text(name_node, source).to_string();
    let mut modifiers = Vec::new();
    // Check for readonly
    if find_child_by_kind(node, "readonly").is_some() {
        modifiers.push("readonly".to_string());
    }
    let type_ann = extract_type_annotation(node, source);
    let sig = if let Some(ref t) = type_ann {
        format!("{}: {}", name, t)
    } else {
        name.clone()
    };

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Property,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

fn extract_ts_type_alias_def(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);

    // Build signature from the full type alias text (excluding body/semicolon)
    let sig = {
        let text = node_text(node, source);
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::TypeAlias,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

fn extract_ts_variable_defs(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
    defs: &mut Vec<DefinitionEntry>,
) {
    // lexical_declaration contains "const"/"let" keyword and variable_declarator(s)
    let mut decl_keyword = String::new();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && (child.kind() == "const" || child.kind() == "let" || child.kind() == "var") {
                decl_keyword = node_text(child, source).to_string();
            }
    }

    let mut modifiers = vec![];
    if !decl_keyword.is_empty() {
        modifiers.push(decl_keyword.clone());
    }
    if is_exported(node) {
        modifiers.push("export".to_string());
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_declarator"
                && let Some(name_node) = find_child_by_field(child, "name") {
                    let name = node_text(name_node, source).to_string();
                    let type_ann = extract_type_annotation(child, source);
                    let sig = if let Some(ref t) = type_ann {
                        format!("{} {}: {}", decl_keyword, name, t)
                    } else {
                        format!("{} {}", decl_keyword, name)
                    };

                    defs.push(DefinitionEntry {
                        file_id,
                        name,
                        kind: DefinitionKind::Variable,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        parent: parent_name.map(|s| s.to_string()),
                        signature: Some(sig.trim().to_string()),
                        modifiers: modifiers.clone(),
                        attributes: Vec::new(),
                        base_types: Vec::new(),
                    });
                }
    }
}

fn extract_ts_enum_member(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")
        .or_else(|| find_child_by_kind(node, "property_identifier"))?;
    let name = node_text(name_node, source).to_string();

    // Check for initializer
    let sig = {
        let text = node_text(node, source).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    };

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::EnumMember,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: sig,
        modifiers: Vec::new(),
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

/// Build a type signature from everything before the body (class_body, object_type, enum_body).
fn build_type_signature(node: tree_sitter::Node, source: &str) -> String {
    let start = node.start_byte();
    let mut end = node.end_byte();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "class_body" | "object_type" | "interface_body" | "enum_body" | "{" => {
                    end = child.start_byte();
                    break;
                }
                _ => {}
            }
        }
    }
    let text = &source[start..end];
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Check if a node is directly inside an enum_body (its parent is enum_body).
fn is_inside_enum_body(node: tree_sitter::Node) -> bool {
    if let Some(parent) = node.parent() {
        return parent.kind() == "enum_body";
    }
    false
}

/// Check if a public_field_definition has an arrow_function as its value.
fn has_arrow_function_value(node: tree_sitter::Node) -> bool {
    if let Some(value) = find_child_by_field(node, "value") {
        return value.kind() == "arrow_function";
    }
    false
}

/// Extract an abstract method signature (e.g., `abstract handle(): void;`).
fn extract_ts_abstract_method_sig(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_kind(node, "property_identifier")?;
    let name = node_text(name_node, source).to_string();
    let mut modifiers = extract_ts_modifiers(node, source);
    if !modifiers.contains(&"abstract".to_string()) {
        modifiers.push("abstract".to_string());
    }
    let params = extract_params_text(node, source);
    let return_type = extract_type_annotation(node, source);
    let sig = build_function_signature(&name, params.as_deref(), return_type.as_deref(), &modifiers);

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Method,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

/// Extract a method signature from an interface body (e.g., `process(order: Order): Promise<void>;`).
fn extract_ts_method_signature(
    node: tree_sitter::Node,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_kind(node, "property_identifier")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_ts_modifiers(node, source);
    let params = extract_params_text(node, source);
    let return_type = extract_type_annotation(node, source);
    let sig = build_function_signature(&name, params.as_deref(), return_type.as_deref(), &modifiers);

    Some(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::Property,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig),
        modifiers,
        attributes: Vec::new(),
        base_types: Vec::new(),
    })
}

// ─── Angular inject() extraction ────────────────────────────────────

/// Extract field types from Angular `inject()` patterns in class bodies.
///
/// Supports two patterns:
/// - **Field initializer**: `private zone = inject(NgZone);`
/// - **Constructor assignment**: `this.store = inject(Store);` inside constructor body
///
/// Handles generic type arguments: `inject(Store<AppState>)` → extracts `"Store"`.
fn extract_ts_inject_types(
    node: tree_sitter::Node,
    source: &str,
    class_field_types: &mut HashMap<String, HashMap<String, String>>,
) {
    let kind = node.kind();
    match kind {
        "class_declaration" | "abstract_class_declaration" => {
            let class_name = find_child_by_field(node, "name")
                .map(|n| node_text(n, source).to_string());
            if let (Some(class_name), Some(body)) = (class_name, find_child_by_kind(node, "class_body")) {
                extract_inject_from_class_body(body, source, &class_name, class_field_types);
            }
            // Don't recurse further for nested classes — they'll be handled by their own match
        }
        _ => {}
    }

    // Recurse into children to find all class declarations
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_ts_inject_types(child, source, class_field_types);
        }
    }
}

/// Walk a class body looking for inject() patterns.
fn extract_inject_from_class_body(
    body: tree_sitter::Node,
    source: &str,
    class_name: &str,
    class_field_types: &mut HashMap<String, HashMap<String, String>>,
) {
    for i in 0..body.child_count() {
        if let Some(child) = body.child(i) {
            match child.kind() {
                // Pattern 1: Field initializer — `private zone = inject(NgZone);`
                "public_field_definition" => {
                    if let Some((field_name, type_name)) = extract_inject_from_field(child, source) {
                        class_field_types
                            .entry(class_name.to_string())
                            .or_default()
                            .insert(field_name, type_name);
                    }
                }
                // Pattern 2: Constructor assignment — `this.store = inject(Store);`
                "method_definition" => {
                    let is_constructor = find_child_by_field(child, "name")
                        .map(|n| node_text(n, source) == "constructor")
                        .unwrap_or(false);
                    if is_constructor
                        && let Some(stmt_block) = find_child_by_kind(child, "statement_block") {
                            extract_inject_from_statement_block(stmt_block, source, class_name, class_field_types);
                        }
                }
                _ => {}
            }
        }
    }
}

/// Extract inject() from a field initializer: `private zone = inject(NgZone);`
fn extract_inject_from_field(node: tree_sitter::Node, source: &str) -> Option<(String, String)> {
    // Get field name
    let name_node = find_child_by_field(node, "name")
        .or_else(|| find_child_by_kind(node, "property_identifier"))?;
    let field_name = node_text(name_node, source).to_string();

    // Get the value (initializer)
    let value_node = find_child_by_field(node, "value")?;

    // Check if it's a call_expression with function name "inject"
    extract_inject_class_name(value_node, source)
        .map(|type_name| (field_name, type_name))
}

/// Extract inject() assignments from a statement block (constructor body).
/// Looks for: `this.fieldName = inject(ClassName);`
fn extract_inject_from_statement_block(
    block: tree_sitter::Node,
    source: &str,
    class_name: &str,
    class_field_types: &mut HashMap<String, HashMap<String, String>>,
) {
    for i in 0..block.child_count() {
        if let Some(child) = block.child(i) {
            // expression_statement → assignment_expression
            if child.kind() == "expression_statement" {
                for j in 0..child.child_count() {
                    if let Some(expr) = child.child(j)
                        && expr.kind() == "assignment_expression"
                            && let Some((field_name, type_name)) = extract_inject_from_assignment(expr, source) {
                                class_field_types
                                    .entry(class_name.to_string())
                                    .or_default()
                                    .insert(field_name, type_name);
                            }
                }
            }
        }
    }
}

/// Extract inject() from an assignment expression: `this.store = inject(Store)`
fn extract_inject_from_assignment(node: tree_sitter::Node, source: &str) -> Option<(String, String)> {
    // Left side should be member_expression: this.fieldName
    let left = find_child_by_field(node, "left")?;
    if left.kind() != "member_expression" {
        return None;
    }
    let obj = find_child_by_field(left, "object")?;
    if node_text(obj, source).trim() != "this" && obj.kind() != "this" {
        return None;
    }
    let prop = find_child_by_field(left, "property")?;
    let field_name = node_text(prop, source).to_string();

    // Right side should be inject(ClassName)
    let right = find_child_by_field(node, "right")?;
    let type_name = extract_inject_class_name(right, source)?;

    Some((field_name, type_name))
}

/// Check if a node is a call_expression to `inject(ClassName)` and extract the class name.
/// Handles generic type params: `inject(Store<AppState>)` → `"Store"`.
fn extract_inject_class_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }

    // Check function name is "inject"
    let func = find_child_by_field(node, "function").or_else(|| node.child(0))?;
    if node_text(func, source).trim() != "inject" {
        return None;
    }

    // Get arguments
    let args = find_child_by_kind(node, "arguments")?;

    // Find the first real argument (skip parentheses and commas)
    for k in 0..args.child_count() {
        if let Some(arg) = args.child(k)
            && arg.is_named() {
                let arg_text = node_text(arg, source).trim().to_string();
                // Strip generic type params: Store<AppState> → Store
                let base_name = arg_text
                    .split('<')
                    .next()
                    .unwrap_or(&arg_text)
                    .trim()
                    .to_string();
                if !base_name.is_empty() {
                    return Some(base_name);
                }
            }
    }
    None
}

// ─── Call-site extraction ───────────────────────────────────────────

/// Parse a TS field signature "name: Type" into (name, base_type).
fn parse_ts_field_type(sig: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = sig.splitn(2, ':').collect();
    if parts.len() == 2 {
        let name = parts[0].trim().to_string();
        let type_str = parts[1].trim();
        let base_type = type_str
            .split('<')
            .next()
            .unwrap_or(type_str)
            .trim()
            .to_string();
        if !name.is_empty() && !base_type.is_empty() {
            return Some((name, base_type));
        }
    }
    None
}

/// Extract parameter names and types from a TS constructor signature.
/// TS format: `constructor(private userService: UserService, logger: Logger)`
fn extract_ts_constructor_param_types(sig: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let start = match sig.find('(') {
        Some(i) => i + 1,
        None => return result,
    };
    let end = match sig.rfind(')') {
        Some(i) => i,
        None => return result,
    };
    if start >= end {
        return result;
    }

    let params_str = &sig[start..end];
    for param in params_str.split(',') {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        // TS params: "private readonly name: Type" or "name: Type"
        let parts: Vec<&str> = param.splitn(2, ':').collect();
        if parts.len() == 2 {
            let name_part = parts[0].trim();
            let type_part = parts[1].trim();
            // Last word of name_part is the param name (skip modifiers)
            let name = name_part
                .split_whitespace()
                .last()
                .unwrap_or("")
                .to_string();
            let base_type = type_part
                .split('<')
                .next()
                .unwrap_or(type_part)
                .trim()
                .to_string();
            if !name.is_empty() && !base_type.is_empty() {
                result.push((name, base_type));
            }
        }
    }
    result
}

/// Extracts type annotations from local variable declarations in a method body.
/// Handles two patterns:
/// 1. Explicit type annotation: const x: Foo = ...
/// 2. Constructor inference: const x = new Foo(...)
///    Returns a map of variable_name -> base_type.
fn extract_ts_local_var_types(
    body_node: tree_sitter::Node,
    source: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    collect_ts_local_var_types(body_node, source, &mut vars);
    vars
}

fn collect_ts_local_var_types(
    node: tree_sitter::Node,
    source: &str,
    vars: &mut HashMap<String, String>,
) {
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && child.kind() == "variable_declarator" {
                        extract_ts_var_declarator_type(child, source, vars);
                    }
            }
        }
        // Don't recurse into nested functions/classes/arrow functions
        "function_declaration" | "arrow_function" | "class_declaration"
        | "method_definition" => return,
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_ts_local_var_types(child, source, vars);
        }
    }
}

fn extract_ts_var_declarator_type(
    node: tree_sitter::Node,
    source: &str,
    vars: &mut HashMap<String, String>,
) {
    // Get variable name
    let name_node = match node.child_by_field_name("name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(name_node, source).trim().to_string();
    if name.is_empty() { return; }

    // Path 1: explicit type annotation — const x: Foo = ...
    if let Some(type_node) = find_child_by_kind(node, "type_annotation") {
        let type_text = node_text(type_node, source).trim();
        let type_str = type_text.strip_prefix(':').unwrap_or(type_text).trim();
        let base_type = type_str
            .split('<')
            .next()
            .unwrap_or(type_str)
            .trim()
            .to_string();
        if !base_type.is_empty() && base_type.chars().next().is_some_and(|c| c.is_uppercase()) {
            vars.insert(name, base_type);
            return;
        }
    }

    // Path 2: infer from new expression — const x = new Foo(...)
    if let Some(value_node) = node.child_by_field_name("value")
        && let Some(new_type) = extract_type_from_new_expr(value_node, source) {
            vars.insert(name, new_type);
        }
}

/// Extracts the constructor name from a `new_expression` node or its wrapper.
/// Handles: new Foo(), new Foo<T>(), new ns.Foo()
/// Returns the simple class name (last segment, without generics).
fn extract_type_from_new_expr(
    node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    let new_expr = if node.kind() == "new_expression" {
        Some(node)
    } else {
        find_descendant_by_kind(node, "new_expression")
    };

    let new_expr = new_expr?;
    // In tree-sitter-typescript, new_expression children:
    // child(0) = "new" keyword, child(1) = constructor identifier/member_expression
    let constructor_node = new_expr.child(1)?;
    let text = node_text(constructor_node, source).trim().to_string();

    // Handle ns.Foo → take "Foo" (last segment)
    let simple_name = text.rsplit('.').next().unwrap_or(&text);
    // Strip generics: Foo<T> → Foo
    let base = simple_name
        .split('<')
        .next()
        .unwrap_or(simple_name)
        .trim()
        .to_string();

    if !base.is_empty() && base.chars().next().is_some_and(|c| c.is_uppercase()) {
        Some(base)
    } else {
        None
    }
}

/// Extract call sites from a method/function body node.
fn extract_ts_call_sites(
    method_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
) -> Vec<CallSite> {
    let mut calls = Vec::new();

    // Find the body (statement_block for methods/functions, or walk the whole node)
    let body = find_child_by_kind(method_node, "statement_block")
        .or_else(|| find_child_by_kind(method_node, "arrow_function"))
        .unwrap_or(method_node);

    // Extract local variable types and merge with field types
    let local_vars = extract_ts_local_var_types(body, source);
    let mut combined_types = field_types.clone();
    for (name, type_name) in local_vars {
        combined_types.entry(name).or_insert(type_name);
    }

    walk_ts_for_invocations(body, source, class_name, &combined_types, &mut calls);

    calls.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.method_name.cmp(&b.method_name))
            .then_with(|| a.receiver_type.cmp(&b.receiver_type))
    });
    calls.dedup_by(|a, b| {
        a.line == b.line && a.method_name == b.method_name && a.receiver_type == b.receiver_type
    });

    calls
}

/// Recursively walk AST looking for call_expression and new_expression nodes.
fn walk_ts_for_invocations(
    node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
    calls: &mut Vec<CallSite>,
) {
    match node.kind() {
        "call_expression" => {
            if let Some(call) = extract_ts_call(node, source, class_name, field_types) {
                calls.push(call);
            }
            // Recurse into ALL children — not just arguments.
            // The function child (first child, typically member_expression)
            // may contain nested call_expressions for chained calls like:
            //   service.method1().method2().then(...)
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_ts_for_invocations(child, source, class_name, field_types, calls);
                }
            }
            return;
        }
        "new_expression" => {
            if let Some(call) = extract_ts_new_expression(node, source) {
                calls.push(call);
            }
            // Same fix: recurse into all children to capture nested calls
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_ts_for_invocations(child, source, class_name, field_types, calls);
                }
            }
            return;
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_ts_for_invocations(child, source, class_name, field_types, calls);
        }
    }
}

/// Extract a call site from a call_expression node.
fn extract_ts_call(
    node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<CallSite> {
    let func_node = find_child_by_field(node, "function").or_else(|| node.child(0))?;
    let line = node.start_position().row as u32 + 1;

    match func_node.kind() {
        "identifier" => {
            let method_name = node_text(func_node, source).to_string();
            Some(CallSite {
                method_name,
                receiver_type: None,
                line,
                receiver_is_generic: false,
            })
        }
        "member_expression" => {
            extract_ts_member_call(func_node, source, class_name, field_types, line)
        }
        _ => None,
    }
}

/// Extract a call site from a member_expression (e.g., `this.method()`, `service.method()`).
fn extract_ts_member_call(
    member_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
    line: u32,
) -> Option<CallSite> {
    let property_node = find_child_by_field(member_node, "property")?;
    let method_name = node_text(property_node, source).to_string();

    let object_node =
        find_child_by_field(member_node, "object").or_else(|| member_node.child(0))?;
    let receiver_type = resolve_ts_receiver_type(object_node, source, class_name, field_types);

    Some(CallSite {
        method_name,
        receiver_type,
        line,
        receiver_is_generic: false,
    })
}

/// Resolve the type of a receiver expression.
fn resolve_ts_receiver_type(
    object_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<String> {
    let text = node_text(object_node, source).trim();

    match object_node.kind() {
        "this" => {
            if class_name.is_empty() {
                None
            } else {
                Some(class_name.to_string())
            }
        }
        "identifier" => {
            if let Some(type_name) = field_types.get(text) {
                Some(type_name.clone())
            } else {
                // Preserve receiver name regardless of case (e.g., "dbSession", "UserService")
                Some(text.to_string())
            }
        }
        "member_expression" => {
            // Handle this.service.method() — object is this.service
            let inner_object = find_child_by_field(object_node, "object")?;
            let inner_property = find_child_by_field(object_node, "property")?;
            let inner_obj_text = node_text(inner_object, source).trim();

            if inner_obj_text == "this" || inner_object.kind() == "this" {
                let prop_name = node_text(inner_property, source);
                field_types.get(prop_name).cloned()
            } else {
                None
            }
        }
        _ => {
            if text == "this" {
                if class_name.is_empty() {
                    None
                } else {
                    Some(class_name.to_string())
                }
            } else {
                None
            }
        }
    }
}

// ─── Code stats computation ─────────────────────────────────────────

fn compute_code_stats_typescript(
    method_node: tree_sitter::Node,
    _source: &str,
) -> CodeStats {
    let mut stats = CodeStats {
        cyclomatic_complexity: 1, // base complexity
        param_count: count_parameters_typescript(method_node),
        ..Default::default()
    };

    // Find body node — statement_block for methods/functions, or arrow body
    let body = find_child_by_kind(method_node, "statement_block")
        .or_else(|| {
            // For arrow functions assigned to fields: public_field_definition -> value -> arrow_function -> body
            find_child_by_field(method_node, "value")
                .and_then(|v| if v.kind() == "arrow_function" {
                    find_child_by_kind(v, "statement_block")
                        .or(Some(v)) // expression body arrow
                } else {
                    None
                })
        });

    if let Some(body_node) = body {
        walk_code_stats(body_node, &[], 0, &mut stats, &TYPESCRIPT_CODE_STATS_CONFIG);
    }

    // callCount is filled separately from call_sites after invocations walk
    stats
}

pub(crate) fn count_parameters_typescript(method_node: tree_sitter::Node) -> u8 {
    // Direct formal_parameters child
    find_child_by_kind(method_node, "formal_parameters")
        .or_else(|| {
            // For arrow function fields: public_field_definition -> value -> arrow_function -> formal_parameters
            find_child_by_field(method_node, "value")
                .filter(|v| v.kind() == "arrow_function")
                .and_then(|v| find_child_by_kind(v, "formal_parameters"))
        })
        .map(count_named_children)
        .unwrap_or(0)
}

// walk_code_stats_typescript removed — replaced by unified walk_code_stats() in tree_sitter_utils.rs
// with TYPESCRIPT_CODE_STATS_CONFIG.

/// Extract a call site from a new_expression node (e.g., `new SomeClass()`).
fn extract_ts_new_expression(node: tree_sitter::Node, source: &str) -> Option<CallSite> {
    // new_expression: find the constructor identifier
    let type_node = find_child_by_field(node, "constructor").or_else(|| {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && child.kind() == "identifier" {
                    return Some(child);
                }
        }
        None
    })?;

    let type_text = node_text(type_node, source);
    // Check for generics BEFORE stripping: new Map<K,V>() → is_generic = true
    // Also check the full new_expression text for type_arguments child node
    let is_generic = type_text.contains('<') || {
        // tree-sitter may separate generics into a type_arguments child node
        let mut found = false;
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && child.kind() == "type_arguments" {
                    found = true;
                    break;
                }
        }
        found
    };
    let type_name = type_text.split('<').next().unwrap_or(type_text).trim();

    if type_name.is_empty() {
        return None;
    }

    Some(CallSite {
        method_name: type_name.to_string(),
        receiver_type: Some(type_name.to_string()),
        line: node.start_position().row as u32 + 1,
        receiver_is_generic: is_generic,
    })
}