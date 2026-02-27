//! C# AST parser using tree-sitter: extracts definitions and call sites.

use std::collections::HashMap;

use super::types::*;
use super::tree_sitter_utils::{node_text, find_child_by_kind, find_descendant_by_kind, find_child_by_field, walk_code_stats, CSHARP_CODE_STATS_CONFIG};

// ─── Main entry point ───────────────────────────────────────────────

pub(crate) fn parse_csharp_definitions(
    parser: &mut tree_sitter::Parser,
    source: &str,
    file_id: u32,
) -> (Vec<DefinitionEntry>, Vec<(usize, Vec<CallSite>)>, Vec<(usize, CodeStats)>, HashMap<String, Vec<String>>) {
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => {
            eprintln!("[def-index] WARNING: tree-sitter C# parse returned None for file_id={}", file_id);
            return (Vec::new(), Vec::new(), Vec::new(), HashMap::new());
        }
    };

    let mut defs = Vec::new();
    let source_bytes = source.as_bytes();
    let mut method_nodes: Vec<(usize, tree_sitter::Node)> = Vec::new();
    walk_csharp_node_collecting(tree.root_node(), source_bytes, file_id, None, &mut defs, &mut method_nodes);

    // Build per-class field type maps and method return type maps from the collected defs
    let mut class_field_types: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut class_base_types: HashMap<String, Vec<String>> = HashMap::new();
    let mut class_method_return_types: HashMap<String, HashMap<String, String>> = HashMap::new();

    for def in &defs {
        if let Some(ref parent) = def.parent {
            match def.kind {
                DefinitionKind::Field | DefinitionKind::Property => {
                    if let Some(ref sig) = def.signature {
                        if let Some((type_name, _field_name)) = parse_field_signature(sig) {
                            class_field_types
                                .entry(parent.clone())
                                .or_default()
                                .insert(def.name.clone(), type_name);
                        }
                    }
                }
                DefinitionKind::Method => {
                    if let Some(ref sig) = def.signature {
                        if let Some(return_type) = parse_return_type_from_signature(sig) {
                            class_method_return_types
                                .entry(parent.clone())
                                .or_default()
                                .insert(def.name.clone(), return_type);
                        }
                    }
                }
                DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record => {
                    if !def.base_types.is_empty() {
                        class_base_types.insert(def.name.clone(), def.base_types.clone());
                    }
                }
                _ => {}
            }
        }
        if def.parent.is_none() && matches!(def.kind, DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record) {
            if !def.base_types.is_empty() {
                class_base_types.insert(def.name.clone(), def.base_types.clone());
            }
        }
    }

    // Extract constructor parameter types as field types (DI pattern).
    // Two strategies:
    //   1. Convention-based: map _paramName and bare paramName to the param type.
    //   2. Assignment-based: parse constructor body for `field = paramName` assignments
    //      and map the assigned field to the param type. This handles ANY naming
    //      convention (m_field, _field, this.field, etc.) without hardcoding prefixes.
    let constructor_param_types: HashMap<String, HashMap<String, String>> = {
        let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();
        for def in &defs {
            if def.kind == DefinitionKind::Constructor {
                if let Some(ref parent) = def.parent {
                    if let Some(ref sig) = def.signature {
                        let param_types = extract_constructor_param_types(sig);
                        let field_map = result.entry(parent.clone()).or_default();
                        for (param_name, param_type) in param_types {
                            field_map.insert(param_name, param_type);
                        }
                    }
                }
            }
        }
        result
    };

    // Strategy 1: Convention-based — add _paramName and bare paramName mappings
    for (class_name, param_map) in &constructor_param_types {
        let field_map = class_field_types.entry(class_name.clone()).or_default();
        for (param_name, param_type) in param_map {
            let underscore_name = format!("_{}", param_name);
            if !field_map.contains_key(&underscore_name) {
                field_map.insert(underscore_name, param_type.clone());
            }
            if !field_map.contains_key(param_name) {
                field_map.insert(param_name.clone(), param_type.clone());
            }
        }
    }

    // Strategy 2: Assignment-based — parse constructor bodies for `field = param` patterns
    for &(def_local_idx, ctor_node) in &method_nodes {
        let def = &defs[def_local_idx];
        if def.kind != DefinitionKind::Constructor { continue; }
        let parent_name = match &def.parent {
            Some(p) => p.clone(),
            None => continue,
        };
        let param_map = match constructor_param_types.get(&parent_name) {
            Some(m) => m,
            None => continue,
        };
        if param_map.is_empty() { continue; }

        let body = find_child_by_kind(ctor_node, "block");
        if let Some(body_node) = body {
            let assignments = extract_constructor_field_assignments(body_node, source_bytes, param_map);
            let field_map = class_field_types.entry(parent_name).or_default();
            for (field_name, param_type) in assignments {
                if !field_map.contains_key(&field_name) {
                    field_map.insert(field_name, param_type);
                }
            }
        }
    }

    // Extract call sites from pre-collected method nodes
    let mut call_sites: Vec<(usize, Vec<CallSite>)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let def = &defs[def_local_idx];
        let parent_name = def.parent.as_deref().unwrap_or("");
        let mut field_types = class_field_types.get(parent_name)
            .cloned()
            .unwrap_or_default();

        // If this method is in a nested class, also include outer class's field types.
        // This enables resolving Owner.m_field patterns where m_field is a DI-injected
        // field of the outer (parent) class. Inner class fields take precedence.
        if !parent_name.is_empty() {
            let outer_class_name = defs.iter()
                .find(|d| d.name == parent_name && matches!(d.kind,
                    DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record))
                .and_then(|d| d.parent.as_deref());
            if let Some(outer_name) = outer_class_name {
                if let Some(outer_fields) = class_field_types.get(outer_name) {
                    for (k, v) in outer_fields {
                        field_types.entry(k.clone()).or_insert(v.clone());
                    }
                }
            }
        }

        let base_types = class_base_types.get(parent_name)
            .cloned()
            .unwrap_or_default();

        let method_return_types = class_method_return_types.get(parent_name)
            .cloned()
            .unwrap_or_default();

        let calls = extract_call_sites(method_node, source_bytes, parent_name, &field_types, &base_types, &method_return_types);
        if !calls.is_empty() {
            call_sites.push((def_local_idx, calls));
        }
    }

    // Compute code stats for pre-collected method/constructor/property nodes
    let call_count_map: HashMap<usize, u16> = call_sites.iter()
        .map(|(idx, calls)| (*idx, calls.len() as u16))
        .collect();

    let mut code_stats_entries: Vec<(usize, CodeStats)> = Vec::new();
    for &(def_local_idx, method_node) in &method_nodes {
        let mut stats = compute_code_stats_csharp(method_node, source_bytes);
        stats.call_count = call_count_map.get(&def_local_idx).copied().unwrap_or(0);
        code_stats_entries.push((def_local_idx, stats));
    }

    // Build extension method map: detect static classes with `this` parameter methods
    let extension_methods = build_extension_method_map(&defs);

    (defs, call_sites, code_stats_entries, extension_methods)
}

/// Build a map of extension method names to the static classes that define them.
/// An extension method is a static method in a static class whose first parameter
/// has the `this` modifier (detected via signature pattern `(this `).
fn build_extension_method_map(defs: &[DefinitionEntry]) -> HashMap<String, Vec<String>> {
    use std::collections::HashSet;

    let mut extension_methods: HashMap<String, Vec<String>> = HashMap::new();

    // Step 1: Find all static classes
    let static_classes: HashSet<&str> = defs.iter()
        .filter(|d| matches!(d.kind, DefinitionKind::Class | DefinitionKind::Struct))
        .filter(|d| d.modifiers.iter().any(|m| m == "static"))
        .map(|d| d.name.as_str())
        .collect();

    if static_classes.is_empty() {
        return extension_methods;
    }

    // Step 2: For each method in a static class, check if the signature contains `(this `
    for def in defs {
        if def.kind != DefinitionKind::Method {
            continue;
        }
        let parent = match &def.parent {
            Some(p) => p.as_str(),
            None => continue,
        };
        if !static_classes.contains(parent) {
            continue;
        }
        // Check if the method signature has `(this ` indicating an extension method
        if let Some(ref sig) = def.signature {
            if sig.contains("(this ") {
                extension_methods
                    .entry(def.name.clone())
                    .or_default()
                    .push(parent.to_string());
            }
        }
    }

    extension_methods
}

// ─── Field/Constructor/Method signature parsing ─────────────────────

/// Parse a method signature to extract the return type.
/// Examples:
///   "private Stream GetDataStream()" → Some("Stream")
///   "public async Task<List<User>> GetUsersAsync(string id)" → Some("Task<List<User>>")
///   "public static void Main(string[] args)" → None (void)
///   "override string ToString()" → Some("string")
pub(crate) fn parse_return_type_from_signature(signature: &str) -> Option<String> {
    const MODIFIERS: &[&str] = &[
        "public", "private", "protected", "internal",
        "static", "async", "virtual", "override", "abstract",
        "sealed", "new", "extern", "unsafe", "partial", "readonly",
    ];

    // Find the opening paren — everything before it is modifiers + return_type + method_name
    let paren_pos = signature.find('(')?;
    let before_paren = signature[..paren_pos].trim();

    // Split into tokens, respecting that generic types like Task<List<User>> are a single token
    // We need to handle angle brackets properly
    let tokens = tokenize_signature_before_paren(before_paren);

    if tokens.len() < 2 {
        return None; // Need at least return_type + method_name
    }

    // The last token is the method name. The token before it is the return type.
    // But we need to skip modifiers that appear before the return type.
    // Work backwards: last = method_name, second-to-last = return_type
    // Verify second-to-last is not a modifier (it shouldn't be, but just in case)
    let return_type_idx = tokens.len() - 2;
    let candidate = &tokens[return_type_idx];

    // Check if the candidate is actually a modifier (edge case: shouldn't happen in valid C#)
    if MODIFIERS.contains(&candidate.to_lowercase().as_str()) {
        return None;
    }

    let return_type = candidate.to_string();
    if return_type == "void" {
        return None;
    }

    Some(return_type)
}

/// Tokenize the part of a method signature before the opening paren,
/// keeping generic types like `Task<List<User>>` as single tokens.
fn tokenize_signature_before_paren(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut angle_depth = 0;

    for ch in s.chars() {
        match ch {
            '<' => {
                angle_depth += 1;
                current.push(ch);
            }
            '>' => {
                angle_depth -= 1;
                current.push(ch);
            }
            c if c.is_whitespace() && angle_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parse a field/property signature like "IUserService _userService" into (type, name)
pub(crate) fn parse_field_signature(sig: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = sig.trim().rsplitn(2, char::is_whitespace).collect();
    if parts.len() == 2 {
        let field_name = parts[0].trim().to_string();
        let type_name = parts[1].trim().to_string();
        let base_type = type_name.split('<').next().unwrap_or(&type_name).to_string();
        if !base_type.is_empty() && !field_name.is_empty() {
            return Some((base_type, field_name));
        }
    }
    None
}

/// Extract parameter names and types from a constructor signature.
/// Handles constructor initializers like `: base(logger)` by matching
/// the closing paren that balances the first opening paren.
pub(crate) fn extract_constructor_param_types(sig: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let start = match sig.find('(') {
        Some(i) => i + 1,
        None => return result,
    };
    // Find matching ')' for the first '(' by tracking paren depth.
    // This avoids matching `)` from constructor initializers like `: base(...)`.
    let end = {
        let mut depth = 1;
        let mut pos = None;
        for (i, ch) in sig[start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        pos = Some(start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match pos {
            Some(p) => p,
            None => return result,
        }
    };
    if start >= end { return result; }

    let params_str = &sig[start..end];
    for param in params_str.split(',') {
        let param = param.trim();
        if param.is_empty() { continue; }
        let parts: Vec<&str> = param.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[parts.len() - 1];
            let type_parts: Vec<&&str> = parts[..parts.len() - 1].iter()
                .filter(|p| !matches!(**p, "ref" | "out" | "in" | "params" | "this"))
                .collect();
            if let Some(type_str) = type_parts.last() {
                let base_type = type_str.split('<').next().unwrap_or(type_str);
                result.push((name.to_string(), base_type.to_string()));
            }
        }
    }
    result
}

// ─── Constructor body assignment extraction ─────────────────────────

/// Parse constructor body for field assignment patterns like:
///   `_field = paramName;`
///   `m_field = paramName;`
///   `this.field = paramName;`
///   `this._field = paramName;`
///
/// Returns a list of (field_name, param_type) tuples for any assignment where
/// the right-hand side matches a known constructor parameter name.
/// This handles ANY naming convention without hardcoding prefixes.
fn extract_constructor_field_assignments(
    body_node: tree_sitter::Node,
    source: &[u8],
    param_types: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    collect_constructor_assignments(body_node, source, param_types, &mut result);
    result
}

fn collect_constructor_assignments(
    node: tree_sitter::Node,
    source: &[u8],
    param_types: &HashMap<String, String>,
    result: &mut Vec<(String, String)>,
) {
    if node.kind() == "assignment_expression" || node.kind() == "simple_assignment_expression" {
        // AST: assignment_expression → left = right
        // left is the field (identifier or member_access_expression like this._field)
        // right is the parameter name (identifier)
        let left = node.child(0);
        let right = node.child(2); // child(1) is "="

        if let (Some(left_node), Some(right_node)) = (left, right) {
            // Right side must be a simple identifier matching a constructor param
            if right_node.kind() == "identifier" {
                let param_name = node_text(right_node, source).trim();
                if let Some(param_type) = param_types.get(param_name) {
                    // Extract field name from left side
                    let field_name = match left_node.kind() {
                        "identifier" => {
                            // Direct: _field = param; or m_field = param;
                            Some(node_text(left_node, source).trim().to_string())
                        }
                        "member_access_expression" => {
                            // this._field = param; or this.m_field = param;
                            let expr = find_child_by_field(left_node, "expression")
                                .or_else(|| left_node.child(0));
                            let name = find_child_by_field(left_node, "name");
                            if let (Some(expr_node), Some(name_node)) = (expr, name) {
                                let expr_text = node_text(expr_node, source).trim();
                                if expr_text == "this" || expr_node.kind() == "this_expression" {
                                    Some(node_text(name_node, source).trim().to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    if let Some(name) = field_name {
                        if !name.is_empty() {
                            result.push((name, param_type.clone()));
                        }
                    }
                }
            }
        }
    }

    // Recurse into children, but skip nested lambdas/methods
    match node.kind() {
        "lambda_expression" | "anonymous_method_expression" | "local_function_statement" => return,
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_constructor_assignments(child, source, param_types, result);
        }
    }
}

// ─── Call site extraction ───────────────────────────────────────────

fn extract_call_sites(
    method_node: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
    method_return_types: &HashMap<String, String>,
) -> Vec<CallSite> {
    let mut calls = Vec::new();

    let body = find_child_by_kind(method_node, "block")
        .or_else(|| find_child_by_kind(method_node, "arrow_expression_clause"));

    if let Some(body_node) = body {
        // Extract local variable types and merge with field types
        let local_vars = extract_csharp_local_var_types(body_node, source, method_return_types);
        let mut combined_types = field_types.clone();
        for (name, type_name) in local_vars {
            combined_types.entry(name).or_insert(type_name);
        }

        walk_for_invocations(body_node, source, class_name, &combined_types, base_types, &mut calls);
    }

    calls.sort_by(|a, b| a.line.cmp(&b.line)
        .then_with(|| a.method_name.cmp(&b.method_name))
        .then_with(|| a.receiver_type.cmp(&b.receiver_type)));
    calls.dedup_by(|a, b| a.line == b.line && a.method_name == b.method_name && a.receiver_type == b.receiver_type);

    calls
}

fn walk_for_invocations(
    node: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
    calls: &mut Vec<CallSite>,
) {
    match node.kind() {
        "invocation_expression" => {
            if let Some(call) = extract_invocation(node, source, class_name, field_types, base_types) {
                calls.push(call);
            }
            // Recurse into ALL children — not just argument_list.
            // The expression child (first child, typically member_access_expression)
            // may contain nested invocation_expressions for chained calls like:
            //   a.Method1().Method2().ConfigureAwait(false)
            // AST: invocation(member_access(invocation(member_access(...)), name), args)
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                walk_for_invocations(child, source, class_name, field_types, base_types, calls);
            }
            return;
        }
        "object_creation_expression" => {
            if let Some(call) = extract_object_creation(node, source) {
                calls.push(call);
            }
            // Same fix: recurse into all children to capture nested calls in arguments
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                walk_for_invocations(child, source, class_name, field_types, base_types, calls);
            }
            return;
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        walk_for_invocations(node.child(i).unwrap(), source, class_name, field_types, base_types, calls);
    }
}

fn extract_invocation(
    node: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
) -> Option<CallSite> {
    let expr = node.child(0)?;
    let line = node.start_position().row as u32 + 1;

    match expr.kind() {
        "identifier" => {
            let method_name = node_text(expr, source).to_string();
            Some(CallSite { method_name, receiver_type: None, line, receiver_is_generic: false })
        }
        "member_access_expression" => {
            extract_member_access_call(expr, source, class_name, field_types, base_types, line)
        }
        "conditional_access_expression" => {
            extract_conditional_access_call(expr, source, class_name, field_types, base_types, line)
        }
        "generic_name" => {
            let name_node = find_child_by_field(expr, "name")
                .or_else(|| expr.child(0));
            let method_name = name_node.map(|n| node_text(n, source)).unwrap_or("");
            if !method_name.is_empty() {
                Some(CallSite { method_name: method_name.to_string(), receiver_type: None, line, receiver_is_generic: false })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_member_access_call(
    node: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
    line: u32,
) -> Option<CallSite> {
    let name_node = find_child_by_field(node, "name")?;
    let method_name = extract_method_name_from_name_node(name_node, source);

    let receiver_node = find_child_by_field(node, "expression")
        .or_else(|| node.child(0))?;
    let receiver_type = resolve_receiver_type(receiver_node, source, class_name, field_types, base_types);

    Some(CallSite { method_name, receiver_type, line, receiver_is_generic: false })
}

fn extract_conditional_access_call(
    node: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
    line: u32,
) -> Option<CallSite> {
    let receiver_node = node.child(0)?;

    let mut binding = None;
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "member_binding_expression" {
            binding = Some(child);
            break;
        }
    }

    let binding = binding?;
    let name_node = find_child_by_field(binding, "name")
        .or_else(|| binding.child(binding.child_count().saturating_sub(1)))?;
    let method_name = extract_method_name_from_name_node(name_node, source);

    let receiver_type = resolve_receiver_type(receiver_node, source, class_name, field_types, base_types);

    Some(CallSite { method_name, receiver_type, line, receiver_is_generic: false })
}

/// Extract the method name from a name node, handling `generic_name` by stripping
/// type arguments. For `generic_name` nodes (e.g., `Method<T>`), returns just the
/// identifier (`Method`). For other nodes (e.g., `identifier`), returns the full text.
fn extract_method_name_from_name_node(name_node: tree_sitter::Node, source: &[u8]) -> String {
    if name_node.kind() == "generic_name" {
        // generic_name: child(0) = identifier, child(1) = type_argument_list
        if let Some(id_node) = name_node.child(0) {
            if id_node.kind() == "identifier" {
                return node_text(id_node, source).to_string();
            }
        }
        // Fallback: strip everything from '<' onwards
        let text = node_text(name_node, source);
        text.split('<').next().unwrap_or(text).to_string()
    } else {
        node_text(name_node, source).to_string()
    }
}

fn extract_object_creation(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<CallSite> {
    let type_node = find_child_by_field(node, "type")?;
    let type_text = node_text(type_node, source);
    let is_generic = type_text.contains('<');
    let type_name = type_text.split('<').next().unwrap_or(type_text).trim();

    if type_name.is_empty() { return None; }

    Some(CallSite {
        method_name: type_name.to_string(),
        receiver_type: Some(type_name.to_string()),
        line: node.start_position().row as u32 + 1,
        receiver_is_generic: is_generic,
    })
}

fn resolve_receiver_type(
    receiver: tree_sitter::Node,
    source: &[u8],
    class_name: &str,
    field_types: &HashMap<String, String>,
    base_types: &[String],
) -> Option<String> {
    let text = node_text(receiver, source);
    match receiver.kind() {
        "identifier" => {
            let name = text.trim();
            match name {
                "this" => Some(class_name.to_string()),
                "base" => base_types.first().map(|bt| bt.split('<').next().unwrap_or(bt).to_string()),
                _ => {
                    if let Some(type_name) = field_types.get(name) {
                        Some(type_name.clone())
                    } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                        Some(name.to_string())
                    } else {
                        Some(name.to_string()) // preserve unresolved receiver name (e.g., "dbSession")
                    }
                }
            }
        }
        "this_expression" => Some(class_name.to_string()),
        "base_expression" => base_types.first().map(|bt| bt.split('<').next().unwrap_or(bt).to_string()),
        "member_access_expression" => {
            // Chained property access: _context.RuntimeContext.UtteranceIndexBuilder
            // Extract the LAST member name as the receiver type.
            // This handles patterns like: field.Property.Type.Method()
            // where we want "Type" as the receiver.
            let name_node = find_child_by_field(receiver, "name");
            if let Some(name) = name_node {
                let name_text = node_text(name, source).trim();
                // If the last segment is a known field/property, resolve its type
                if let Some(type_name) = field_types.get(name_text) {
                    return Some(type_name.clone());
                }
                // If starts with uppercase, treat as type name (PascalCase convention)
                if !name_text.is_empty() && name_text.chars().next().is_some_and(|c| c.is_uppercase()) {
                    return Some(name_text.to_string());
                }
            }
            // Fallback: try to resolve the leftmost identifier through field_types
            let expr_node = find_child_by_field(receiver, "expression")
                .or_else(|| receiver.child(0));
            if let Some(expr) = expr_node {
                return resolve_receiver_type(expr, source, class_name, field_types, base_types);
            }
            None
        }
        _ => {
            let trimmed = text.trim();
            if trimmed == "this" {
                Some(class_name.to_string())
            } else if trimmed == "base" {
                base_types.first().map(|bt| bt.split('<').next().unwrap_or(bt).to_string())
            } else {
                None
            }
        }
    }
}

// ─── AST walking ────────────────────────────────────────────────────

/// Walk AST collecting definitions AND method/constructor nodes for call extraction.
fn walk_csharp_node_collecting<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    file_id: u32,
    parent_name: Option<&str>,
    defs: &mut Vec<DefinitionEntry>,
    method_nodes: &mut Vec<(usize, tree_sitter::Node<'a>)>,
) {
    let kind = node.kind();

    match kind {
        "class_declaration" | "interface_declaration" | "struct_declaration"
        | "enum_declaration" | "record_declaration" => {
            if let Some(def) = extract_csharp_type_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                defs.push(def);
                for i in 0..node.child_count() {
                    let child = node.child(i).unwrap();
                    match child.kind() {
                        "declaration_list" | "enum_member_declaration_list" => {
                            walk_csharp_node_collecting(child, source, file_id, Some(&name), defs, method_nodes);
                        }
                        _ => {}
                    }
                }
                return;
            }
        }
        "method_declaration" => {
            if let Some(def) = extract_csharp_method_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                method_nodes.push((idx, node));
                return;
            }
        }
        "constructor_declaration" => {
            if let Some(def) = extract_csharp_constructor_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                method_nodes.push((idx, node));
                return;
            }
        }
        "property_declaration" => {
            if let Some(def) = extract_csharp_property_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                defs.push(def);
                // Expression body properties (e.g. `public string Name => expr;`)
                // have an arrow_expression_clause that may contain call sites.
                if find_child_by_kind(node, "arrow_expression_clause").is_some() {
                    method_nodes.push((idx, node));
                }
                return;
            }
        }
        "field_declaration" => {
            extract_csharp_field_defs(node, source, file_id, parent_name, defs);
            return;
        }
        "delegate_declaration" => {
            if let Some(def) = extract_csharp_delegate_def(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "event_declaration" | "event_field_declaration" => {
            if let Some(def) = extract_csharp_event_def(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        "enum_member_declaration" => {
            if let Some(def) = extract_csharp_enum_member(node, source, file_id, parent_name) {
                defs.push(def);
                return;
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        walk_csharp_node_collecting(node.child(i).unwrap(), source, file_id, parent_name, defs, method_nodes);
    }
}

// ─── Definition extraction helpers ──────────────────────────────────

fn extract_modifiers(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut modifiers = Vec::new();
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "modifier" || child.kind().ends_with("_modifier") {
            modifiers.push(node_text(child, source).to_string());
        }
        match child.kind() {
            "public" | "private" | "protected" | "internal" | "static" | "readonly"
            | "sealed" | "abstract" | "virtual" | "override" | "async" | "partial"
            | "new" | "extern" | "unsafe" | "volatile" | "const" => {
                modifiers.push(node_text(child, source).to_string());
            }
            _ => {}
        }
    }
    modifiers
}

fn extract_attributes(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut attributes = Vec::new();
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "attribute_list" {
            for j in 0..child.child_count() {
                let attr = child.child(j).unwrap();
                if attr.kind() == "attribute" {
                    attributes.push(node_text(attr, source).to_string());
                }
            }
        }
    }
    attributes
}

fn extract_base_types(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut base_types = Vec::new();
    if let Some(base_list) = find_child_by_kind(node, "base_list") {
        for i in 0..base_list.child_count() {
            let child = base_list.child(i).unwrap();
            if child.is_named() {
                base_types.push(node_text(child, source).to_string());
            }
        }
    }
    base_types
}

fn extract_csharp_type_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let kind = match node.kind() {
        "class_declaration" => DefinitionKind::Class,
        "interface_declaration" => DefinitionKind::Interface,
        "struct_declaration" => DefinitionKind::Struct,
        "enum_declaration" => DefinitionKind::Enum,
        "record_declaration" => DefinitionKind::Record,
        _ => return None,
    };
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    let base_types = extract_base_types(node, source);
    let sig = build_type_signature(node, source);
    Some(DefinitionEntry {
        file_id, name, kind,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types,
    })
}

fn build_type_signature(node: tree_sitter::Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let mut end = node.end_byte();
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "declaration_list" || child.kind() == "{" {
            end = child.start_byte();
            break;
        }
    }
    let text = std::str::from_utf8(&source[start..end]).unwrap_or("");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_csharp_method_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    let sig = build_method_signature(node, source);
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Method,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn build_method_signature(node: tree_sitter::Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let mut end = node.end_byte();
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "block" || child.kind() == "arrow_expression_clause" || child.kind() == ";" {
            end = child.start_byte();
            break;
        }
    }
    let text = std::str::from_utf8(&source[start..end]).unwrap_or("");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_csharp_constructor_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    let sig = build_method_signature(node, source);
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Constructor,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_csharp_property_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    let type_node = find_child_by_field(node, "type");
    let type_str = type_node.map(|n| node_text(n, source)).unwrap_or("");
    let sig = format!("{} {}", type_str, name);
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Property,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig.trim().to_string()), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_csharp_field_defs(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
    defs: &mut Vec<DefinitionEntry>,
) {
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    if let Some(var_decl) = find_child_by_kind(node, "variable_declaration") {
        let type_node = find_child_by_field(var_decl, "type");
        let type_str = type_node.map(|n| node_text(n, source)).unwrap_or("");
        for i in 0..var_decl.child_count() {
            let child = var_decl.child(i).unwrap();
            if child.kind() == "variable_declarator"
                && let Some(name_node) = find_child_by_field(child, "name") {
                    let name = node_text(name_node, source).to_string();
                    let sig = format!("{} {}", type_str, name);
                    defs.push(DefinitionEntry {
                        file_id, name, kind: DefinitionKind::Field,
                        line_start: node.start_position().row as u32 + 1,
                        line_end: node.end_position().row as u32 + 1,
                        parent: parent_name.map(|s| s.to_string()),
                        signature: Some(sig.trim().to_string()),
                        modifiers: modifiers.clone(), attributes: attributes.clone(),
                        base_types: Vec::new(),
                    });
                }
        }
    }
}

fn extract_csharp_delegate_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    let sig_text = node_text(node, source);
    let sig = sig_text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Delegate,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: Some(sig), modifiers, attributes, base_types: Vec::new(),
    })
}

fn extract_csharp_event_def(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name = if let Some(name_node) = find_child_by_field(node, "name") {
        node_text(name_node, source).to_string()
    } else {
        let var_decl = find_child_by_kind(node, "variable_declaration");
        if let Some(vd) = var_decl {
            let declarator = find_child_by_kind(vd, "variable_declarator");
            if let Some(d) = declarator {
                if let Some(n) = find_child_by_field(d, "name") {
                    node_text(n, source).to_string()
                } else { return None; }
            } else { return None; }
        } else { return None; }
    };
    let modifiers = extract_modifiers(node, source);
    let attributes = extract_attributes(node, source);
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::Event,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: None, modifiers, attributes, base_types: Vec::new(),
    })
}

// ─── Local variable type extraction ─────────────────────────────────

/// Extracts type annotations from local variable declarations in a C# method body.
/// Handles two patterns:
/// 1. Explicit type: `UserResult result = ...`
/// 2. Constructor inference: `var result = new UserResult(...)`
fn extract_csharp_local_var_types(
    body_node: tree_sitter::Node,
    source: &[u8],
    method_return_types: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    collect_csharp_local_var_types(body_node, source, &mut vars, method_return_types);
    vars
}

fn collect_csharp_local_var_types(
    node: tree_sitter::Node,
    source: &[u8],
    vars: &mut HashMap<String, String>,
    method_return_types: &HashMap<String, String>,
) {
    match node.kind() {
        "local_declaration_statement" => {
            if let Some(var_decl) = find_child_by_kind(node, "variable_declaration") {
                extract_csharp_var_declaration_types(var_decl, source, vars, method_return_types);
            }
        }
        // Pattern matching: if (obj is TypeName varName) { ... }
        // Also handles switch case patterns: case TypeName varName:
        // AST: declaration_pattern → [identifier(type), identifier(name)]
        "declaration_pattern" => {
            let type_node = node.child(0);
            let name_node = node.child(1);
            if let (Some(t), Some(n)) = (type_node, name_node) {
                let type_name = node_text(t, source).trim().to_string();
                let var_name = node_text(n, source).trim().to_string();
                if !type_name.is_empty() && !var_name.is_empty()
                    && type_name.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    vars.insert(var_name, type_name);
                }
            }
        }
        // Don't recurse into nested methods/lambdas
        "local_function_statement" | "lambda_expression"
        | "anonymous_method_expression" => return,
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_csharp_local_var_types(child, source, vars, method_return_types);
        }
    }
}

fn extract_csharp_var_declaration_types(
    var_decl: tree_sitter::Node,
    source: &[u8],
    vars: &mut HashMap<String, String>,
    method_return_types: &HashMap<String, String>,
) {
    // Get the type node — first child of variable_declaration
    let type_node = match var_decl.child(0) {
        Some(n) => n,
        None => return,
    };
    let type_text = node_text(type_node, source).trim().to_string();

    let is_var_or_dynamic = type_text == "var" || type_text == "dynamic";

    // For explicit types (not var/dynamic), extract the base type
    let explicit_base_type = if !is_var_or_dynamic {
        let base = type_text.split('<').next().unwrap_or(&type_text).trim().to_string();
        if !base.is_empty() && base.chars().next().is_some_and(|c| c.is_uppercase()) {
            Some(base)
        } else {
            None
        }
    } else {
        None
    };

    // Iterate over variable_declarator children
    for i in 0..var_decl.child_count() {
        if let Some(child) = var_decl.child(i) {
            if child.kind() == "variable_declarator" {
                // Get variable name — try field "name" first, then first child
                let name_node = find_child_by_field(child, "name")
                    .or_else(|| child.child(0));
                if let Some(name_n) = name_node {
                    if name_n.kind() == "identifier" {
                        let name = node_text(name_n, source).trim().to_string();
                        if name.is_empty() { continue; }

                        if let Some(ref base_type) = explicit_base_type {
                            // Path 1: explicit type
                            vars.insert(name, base_type.clone());
                        } else if is_var_or_dynamic {
                            // Path 2a: try to infer from new expression
                            // Try equals_value_clause first, then direct child (tree-sitter C# 0.23
                            // puts object_creation_expression as a direct child of variable_declarator)
                            let mut inferred_type = find_child_by_kind(child, "equals_value_clause")
                                .and_then(|eq| extract_csharp_type_from_new_expr(eq, source))
                                .or_else(|| extract_csharp_type_from_new_expr(child, source));

                            // Path 2b: var x = (TypeName)expr → extract type from cast_expression
                            // AST: cast_expression -> child(0)="(", child(1)=type, child(2)=")", child(3)=expr
                            if inferred_type.is_none() {
                                inferred_type = find_descendant_by_kind(child, "cast_expression")
                                    .and_then(|cast| cast.child(1))
                                    .map(|type_node| node_text(type_node, source).trim().to_string())
                                    .filter(|t| !t.is_empty() && t.chars().next().is_some_and(|c| c.is_uppercase()));
                            }

                            // Path 2c: var x = expr as TypeName
                            // AST: as_expression -> child(0)=expr, child(1)="as", child(2)=type
                            if inferred_type.is_none() {
                                inferred_type = find_descendant_by_kind(child, "as_expression")
                                    .and_then(|as_expr| as_expr.child(2))
                                    .map(|type_node| node_text(type_node, source).trim().to_string())
                                    .filter(|t| !t.is_empty() && t.chars().next().is_some_and(|c| c.is_uppercase()));
                            }

                            // Path 2d: var x = MethodCall() or var x = this.MethodCall()
                            // Path 2d+: var x = await MethodCall() → unwrap Task<T> to T
                            // Look up the return type of the method in the current class
                            if inferred_type.is_none() {
                                let has_await = find_descendant_by_kind(child, "await_expression").is_some();
                                inferred_type = find_descendant_by_kind(child, "invocation_expression")
                                    .and_then(|inv| extract_simple_method_name_from_invocation(inv, source))
                                    .and_then(|method_name| method_return_types.get(&method_name))
                                    .map(|return_type| {
                                        if has_await {
                                            unwrap_task_type(return_type)
                                        } else {
                                            return_type.clone()
                                        }
                                    })
                                    .filter(|t| !t.is_empty() && t.chars().next().is_some_and(|c| c.is_uppercase()));
                            }

                            if let Some(t) = inferred_type {
                                vars.insert(name, t);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extracts the type name from a C# object creation expression.
/// Handles: `new Foo()`, `new Foo<T>()`, `new ns.Foo(args)`
fn extract_csharp_type_from_new_expr(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<String> {
    // Look for object_creation_expression (C# equivalent of TS new_expression)
    let new_expr = if node.kind() == "object_creation_expression" {
        Some(node)
    } else {
        find_descendant_by_kind(node, "object_creation_expression")
    };

    let new_expr = new_expr?;

    // In C# tree-sitter, object_creation_expression: child(0) = "new", child(1) = type
    let type_node = new_expr.child(1)?;
    let text = node_text(type_node, source).trim().to_string();

    // Strip namespaces (ns.Foo → Foo) and generics (Foo<T> → Foo)
    let simple_name = text.rsplit('.').next().unwrap_or(&text);
    let base = simple_name.split('<').next().unwrap_or(simple_name).trim().to_string();

    if !base.is_empty() && base.chars().next().is_some_and(|c| c.is_uppercase()) {
        Some(base)
    } else {
        None
    }
}

/// Extract the method name from an invocation_expression, but only for simple
/// calls (same-class methods). Returns None for cross-class calls via fields.
///
/// Supported patterns:
///   - `GetDataStream()`         → Some("GetDataStream")
///   - `this.GetDataStream()`    → Some("GetDataStream")
///   - `_service.GetData()`      → None (cross-class, field receiver)
///   - `SomeClass.StaticMethod()`→ None (cross-class, static)
///   - `a.b.c.Method()`         → None (cross-class, chained)
fn extract_simple_method_name_from_invocation(
    invocation: tree_sitter::Node,
    source: &[u8],
) -> Option<String> {
    let expr = invocation.child(0)?;

    match expr.kind() {
        // Simple call: GetDataStream()
        "identifier" => {
            let name = node_text(expr, source).trim();
            if !name.is_empty() {
                Some(name.to_string())
            } else {
                None
            }
        }
        // Bare generic call: GetDataStream<T>()
        "generic_name" => {
            let name = extract_method_name_from_name_node(expr, source);
            if !name.is_empty() {
                Some(name)
            } else {
                None
            }
        }
        // Member access: this.GetDataStream() or _field.Method()
        "member_access_expression" => {
            let receiver_node = find_child_by_field(expr, "expression")
                .or_else(|| expr.child(0))?;
            // Only resolve if receiver is `this`
            let receiver_text = node_text(receiver_node, source).trim();
            if receiver_text == "this" || receiver_node.kind() == "this_expression" {
                let name_node = find_child_by_field(expr, "name")?;
                let method_name = extract_method_name_from_name_node(name_node, source);
                if !method_name.is_empty() {
                    Some(method_name)
                } else {
                    None
                }
            } else {
                // Cross-class call (_field.Method(), ClassName.Method()) — skip
                None
            }
        }
        _ => None,
    }
}

// ─── Task<T> unwrapping for await expressions ───────────────────────

/// Unwraps `Task<T>` and `ValueTask<T>` to their inner type `T`.
/// - `"Task<HttpResponseMessage>"` → `"HttpResponseMessage"`
/// - `"ValueTask<Stream>"` → `"Stream"`
/// - `"Task<List<User>>"` → `"List<User>"` (nested generics preserved)
/// - `"Task"` (no generic) → `"Task"` (unchanged, no inner type)
/// - `"Stream"` (not a Task) → `"Stream"` (unchanged)
/// - `"Task<>"` (edge case) → `"Task<>"` (unchanged)
pub(crate) fn unwrap_task_type(type_name: &str) -> String {
    let prefix_len = if type_name.starts_with("Task<") {
        5
    } else if type_name.starts_with("ValueTask<") {
        10
    } else {
        return type_name.to_string(); // not a Task type, return as-is
    };

    // Must end with '>'
    if !type_name.ends_with('>') {
        return type_name.to_string();
    }

    // Extract inner type: Task<HttpResponseMessage> → HttpResponseMessage
    let inner = &type_name[prefix_len..type_name.len() - 1];
    if inner.is_empty() {
        return type_name.to_string(); // Task<> edge case
    }
    inner.to_string()
}

// ─── Code stats computation ─────────────────────────────────────────

fn compute_code_stats_csharp(
    method_node: tree_sitter::Node,
    _source: &[u8],
) -> CodeStats {
    let mut stats = CodeStats::default();
    stats.cyclomatic_complexity = 1; // base complexity

    // paramCount from parameter_list (not from body walk)
    stats.param_count = count_parameters_csharp(method_node);

    // Find body node
    let body = find_child_by_kind(method_node, "block")
        .or_else(|| find_child_by_kind(method_node, "arrow_expression_clause"));

    if let Some(body_node) = body {
        walk_code_stats(body_node, &[], 0, &mut stats, &CSHARP_CODE_STATS_CONFIG);
    }

    // callCount is filled separately from method_calls after invocations walk
    stats
}

pub(crate) fn count_parameters_csharp(method_node: tree_sitter::Node) -> u8 {
    find_child_by_kind(method_node, "parameter_list")
        .map(|params| {
            (0..params.child_count())
                .filter(|&i| params.child(i).map(|c| c.is_named()).unwrap_or(false))
                .count() as u8
        })
        .unwrap_or(0)
}

// walk_code_stats_csharp removed — replaced by unified walk_code_stats() in tree_sitter_utils.rs
// with CSHARP_CODE_STATS_CONFIG.

fn extract_csharp_enum_member(
    node: tree_sitter::Node, source: &[u8], file_id: u32, parent_name: Option<&str>,
) -> Option<DefinitionEntry> {
    let name_node = find_child_by_field(node, "name")?;
    let name = node_text(name_node, source).to_string();
    Some(DefinitionEntry {
        file_id, name, kind: DefinitionKind::EnumMember,
        line_start: node.start_position().row as u32 + 1,
        line_end: node.end_position().row as u32 + 1,
        parent: parent_name.map(|s| s.to_string()),
        signature: None, modifiers: Vec::new(), attributes: Vec::new(), base_types: Vec::new(),
    })
}