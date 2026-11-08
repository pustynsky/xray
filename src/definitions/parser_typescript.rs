//! TypeScript AST parser using tree-sitter: extracts definitions and call sites.

use std::collections::HashMap;

use super::types::*;
use super::tree_sitter_utils::{find_child_by_kind, find_descendant_by_kind, find_child_by_field, count_named_children, walk_code_stats, warn_ast_depth_exceeded_at, MAX_PARSE_SOURCE_BYTES, PARSE_TIMEOUT_MICROS, TYPESCRIPT_CODE_STATS_CONFIG};

const MAX_TYPESCRIPT_AST_RECURSION_DEPTH: usize = 256;

// ─── Main entry point ───────────────────────────────────────────────

pub(crate) fn parse_typescript_definitions_with_components(
    parser: &mut tree_sitter::Parser,
    source: &str,
    file_id: u32,
) -> TypeScriptParseResult {
    parse_typescript_definitions_impl(parser, source, file_id)
}

fn parse_typescript_definitions_impl(
    parser: &mut tree_sitter::Parser,
    source: &str,
    file_id: u32,
) -> TypeScriptParseResult {
    // PARSE-002: skip oversized sources before tree-sitter allocates ~10× RAM.
    if source.len() > MAX_PARSE_SOURCE_BYTES {
        tracing::warn!(
            target: "xray::parse",
            file_id = file_id,
            size = source.len(),
            limit = MAX_PARSE_SOURCE_BYTES,
            "skipping oversized TypeScript source"
        );
        return ((Vec::new(), Vec::new(), Vec::new()), Vec::new());
    }
    // PARSE-001: bound parse wall-clock so a single pathological file cannot
    // pin a worker thread indefinitely.
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);
    // Interrupted parses retain state; this call starts an independent document.
    parser.reset();
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => {
            tracing::warn!(
                target: "xray::parse",
                file_id = file_id,
                "tree-sitter TS parse returned None (timeout or grammar error)"
            );
            return ((Vec::new(), Vec::new(), Vec::new()), Vec::new());
        }
    };

    let mut defs = Vec::new();
    let mut method_nodes: Vec<(usize, tree_sitter::Node)> = Vec::new();
    let mut angular_components = Vec::new();
    walk_typescript_node_collecting(
        tree.root_node(),
        source,
        file_id,
        None,
        (&mut defs, &mut method_nodes),
        Some(&mut angular_components),
        0,
    );

    let lexical_model = build_ts_lexical_model(tree.root_node(), source);
    let mut definition_by_callable_node: HashMap<usize, usize> = method_nodes
        .iter()
        .map(|(definition_index, node)| (node.id(), *definition_index))
        .collect();
    for callable in &lexical_model.callable_definitions {
        let existing_definition = definition_by_callable_node
            .get(&callable.node.id())
            .copied();
        let definition_index = existing_definition.unwrap_or_else(|| {
            let definition_index = defs.len();
            defs.push(DefinitionEntry {
                file_id,
                name: callable.name.clone(),
                kind: DefinitionKind::Function,
                line_start: callable.line_start,
                line_end: callable.line_end,
                parent: callable.parent.clone(),
                signature: Some(ts_callable_signature(callable, source)),
                modifiers: if callable.is_local {
                    vec!["local".to_string()]
                } else {
                    Vec::new()
                },
                attributes: Vec::new(),
                base_types: Vec::new(),
            });
            definition_index
        });
        if existing_definition.is_none() {
            method_nodes.push((definition_index, callable.node));
            definition_by_callable_node.insert(callable.node.id(), definition_index);
        }
    }

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

        let calls = extract_ts_call_sites(
            method_node,
            source,
            parent_name,
            &field_types,
            &lexical_model,
        );
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

    ((defs, call_sites, code_stats_entries), angular_components)
}

fn extract_angular_component_record(
    class_node: tree_sitter::Node,
    source: &str,
) -> Option<AngularComponentRecord> {
    let mut direct_cursor = class_node.walk();
    for decorator in class_node
        .children(&mut direct_cursor)
        .filter(|child| child.kind() == "decorator")
    {
        if let Some(component) = parse_angular_component_decorator(decorator, source) {
            return Some(component);
        }
    }
    let parent = class_node.parent()?;
    if parent.kind() != "export_statement" {
        return None;
    }
    let mut parent_cursor = parent.walk();
    parent
        .children(&mut parent_cursor)
        .filter(|child| child.kind() == "decorator")
        .find_map(|decorator| parse_angular_component_decorator(decorator, source))
}

fn parse_angular_component_decorator(
    decorator: tree_sitter::Node,
    source: &str,
) -> Option<AngularComponentRecord> {
    let call = find_child_by_kind(decorator, "call_expression")
        .or_else(|| find_descendant_by_kind(decorator, "call_expression"))?;
    let function = find_child_by_field(call, "function")?;
    if node_text(function, source) != "Component" {
        return None;
    }
    let arguments = find_child_by_field(call, "arguments")
        .or_else(|| find_child_by_kind(call, "arguments"))?;
    let mut argument_cursor = arguments.walk();
    let argument_nodes: Vec<_> = arguments.named_children(&mut argument_cursor).collect();
    if argument_nodes.is_empty() {
        return Some(AngularComponentRecord {
            selector: StaticValue::Missing,
            template: AngularTemplateSource::Missing,
        });
    }
    if argument_nodes.len() != 1 || argument_nodes[0].kind() != "object" {
        let reason = "component metadata is not a static object".to_string();
        return Some(AngularComponentRecord {
            selector: StaticValue::Dynamic {
                reason: reason.clone(),
            },
            template: AngularTemplateSource::Dynamic { reason },
        });
    }
    let object = argument_nodes[0];

    let mut selector = None;
    let mut template_url = None;
    let mut template = None;
    let mut unknown_metadata = false;
    let mut property_cursor = object.walk();
    for property in object.named_children(&mut property_cursor) {
        if property.kind() == "spread_element" {
            unknown_metadata = true;
            continue;
        }
        if matches!(property.kind(), "shorthand_property_identifier_pattern" | "shorthand_property_identifier") {
            let key = node_text(property, source);
            let dynamic = StaticValue::Dynamic {
                reason: format!("shorthand {key} property is dynamic"),
            };
            match key {
                "selector" => selector = Some(dynamic),
                "templateUrl" => template_url = Some(dynamic),
                "template" => template = Some(dynamic),
                _ => {}
            }
            continue;
        }
        if property.kind() != "pair" {
            continue;
        }
        let Some(key_node) = find_child_by_field(property, "key") else {
            unknown_metadata = true;
            continue;
        };
        let Some(value) = find_child_by_field(property, "value") else {
            unknown_metadata = true;
            continue;
        };
        let Some(key) = angular_property_name(key_node, source) else {
            unknown_metadata = true;
            continue;
        };
        let slot = match key.as_str() {
            "selector" => &mut selector,
            "templateUrl" => &mut template_url,
            "template" => &mut template,
            _ => continue,
        };
        if slot.is_some() {
            *slot = Some(StaticValue::Dynamic {
                reason: format!("duplicate {key} property"),
            });
        } else {
            *slot = Some(angular_static_string(value, source));
        }
    }

    if unknown_metadata {
        let reason = "component metadata contains dynamic properties".to_string();
        return Some(AngularComponentRecord {
            selector: StaticValue::Dynamic {
                reason: reason.clone(),
            },
            template: AngularTemplateSource::Dynamic { reason },
        });
    }

    let template = match (template, template_url) {
        (Some(_), Some(_)) => AngularTemplateSource::Dynamic {
            reason: "component declares both template and templateUrl".to_string(),
        },
        (Some(StaticValue::Static(content)), None) => AngularTemplateSource::Inline { content },
        (Some(StaticValue::Dynamic { reason }), None) => AngularTemplateSource::Dynamic { reason },
        (Some(StaticValue::Missing), None) => AngularTemplateSource::Missing,
        (None, Some(StaticValue::Static(relative_path))) => {
            AngularTemplateSource::External { relative_path }
        }
        (None, Some(StaticValue::Dynamic { reason })) => AngularTemplateSource::Dynamic { reason },
        (None, Some(StaticValue::Missing)) | (None, None) => AngularTemplateSource::Missing,
    };

    Some(AngularComponentRecord {
        selector: selector.unwrap_or(StaticValue::Missing),
        template,
    })
}

fn angular_property_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    match node.kind() {
        "property_identifier" | "identifier" => Some(node_text(node, source).to_string()),
        "string" => decode_ts_string_literal(node_text(node, source)),
        _ => None,
    }
}

fn angular_static_string(node: tree_sitter::Node, source: &str) -> StaticValue<String> {
    match node.kind() {
        "string" => decode_ts_string_literal(node_text(node, source))
            .map(StaticValue::Static)
            .unwrap_or_else(|| StaticValue::Dynamic {
                reason: "invalid string literal".to_string(),
            }),
        "template_string"
            if find_descendant_by_kind(node, "template_substitution").is_none() =>
        {
            decode_ts_string_literal(node_text(node, source))
                .map(StaticValue::Static)
                .unwrap_or_else(|| StaticValue::Dynamic {
                    reason: "invalid template literal".to_string(),
                })
        }
        "template_string" => StaticValue::Dynamic {
            reason: "template interpolation is dynamic".to_string(),
        },
        _ => StaticValue::Dynamic {
            reason: format!("{} is not a static string", node.kind()),
        },
    }
}

pub(crate) fn decode_ts_string_literal(text: &str) -> Option<String> {
    fn parse_fixed_hex(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..digits {
            value = value.checked_mul(16)? + chars.next()?.to_digit(16)?;
        }
        Some(value)
    }

    fn push_scalar(decoded: &mut String, value: u32) -> Option<()> {
        decoded.push(char::from_u32(value)?);
        Some(())
    }

    if text.len() < 2 {
        return None;
    }
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"' | b'`') || text.as_bytes().last().copied() != Some(quote) {
        return None;
    }
    let mut decoded = String::new();
    let mut chars = text[1..text.len() - 1].chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            'b' => decoded.push('\u{0008}'),
            'f' => decoded.push('\u{000c}'),
            'v' => decoded.push('\u{000b}'),
            '0' if !chars
                .as_str()
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit()) =>
            {
                decoded.push('\0');
            }
            '\\' => decoded.push('\\'),
            '\'' => decoded.push('\''),
            '"' => decoded.push('"'),
            '`' => decoded.push('`'),
            'x' => push_scalar(&mut decoded, parse_fixed_hex(&mut chars, 2)?)?,
            'u' => {
                let value = if chars.as_str().starts_with('{') {
                    chars.next();
                    let mut value = 0u32;
                    let mut digits = 0usize;
                    loop {
                        let character = chars.next()?;
                        if character == '}' {
                            break;
                        }
                        if digits == 6 {
                            return None;
                        }
                        value = value.checked_mul(16)? + character.to_digit(16)?;
                        digits += 1;
                    }
                    if digits == 0 {
                        return None;
                    }
                    value
                } else {
                    parse_fixed_hex(&mut chars, 4)?
                };
                push_scalar(&mut decoded, value)?;
            }
            '\n' => {}
            '\r' => {
                if chars.as_str().starts_with('\n') {
                    chars.next();
                }
            }
            _ => return None,
        }
    }
    Some(decoded)
}

// ─── AST walking ────────────────────────────────────────────────────

fn walk_typescript_node_collecting<'a>(
    node: tree_sitter::Node<'a>,
    source: &str,
    file_id: u32,
    parent_name: Option<&str>,
    outputs: (
        &mut Vec<DefinitionEntry>,
        &mut Vec<(usize, tree_sitter::Node<'a>)>,
    ),
    mut angular_components: Option<&mut Vec<ParsedAngularComponentRecord>>,
    depth: usize,
) {
    let (defs, method_nodes) = outputs;
    // MINOR-27: hard cap recursion. Normal TS code is well under 50 levels.
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        return;
    }
    let kind = node.kind();

    match kind {
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(def) = extract_ts_class_def(node, source, file_id, parent_name) {
                let name = def.name.clone();
                let local_def_index = defs.len();
                defs.push(def);
                if let Some(angular_components) = angular_components.as_deref_mut()
                    && let Some(component) = extract_angular_component_record(node, source)
                {
                    let template_children = match &component.template {
                        AngularTemplateSource::Inline { content } => {
                            super::extract_custom_elements(content)
                        }
                        _ => Vec::new(),
                    };
                    angular_components.push(ParsedAngularComponentRecord {
                        local_def_index,
                        component,
                        template_children,
                    });
                }
                // Walk into class body
                if let Some(body) = find_child_by_kind(node, "class_body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            walk_typescript_node_collecting(
                                child,
                                source,
                                file_id,
                                Some(&name),
                                (&mut *defs, &mut *method_nodes),
                                angular_components.as_deref_mut(),
                                depth + 1,
                            );
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
                            walk_typescript_node_collecting(
                                child,
                                source,
                                file_id,
                                Some(&name),
                                (&mut *defs, &mut *method_nodes),
                                angular_components.as_deref_mut(),
                                depth + 1,
                            );
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
                            walk_typescript_node_collecting(
                                child,
                                source,
                                file_id,
                                Some(&name),
                                (&mut *defs, &mut *method_nodes),
                                angular_components.as_deref_mut(),
                                depth + 1,
                            );
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
            if let Some(mut def) = extract_ts_field_def(node, source, file_id, parent_name) {
                let idx = defs.len();
                // Collect arrow function fields for call-site extraction and root election.
                if has_arrow_function_value(node) {
                    def.modifiers.push("callable".to_string());
                    method_nodes.push((idx, node));
                }
                defs.push(def);
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
        // Only extract exported variable declarations
        "lexical_declaration" if is_exported(node) => {
            let first_definition = defs.len();
            extract_ts_variable_defs(node, source, file_id, parent_name, defs);
            collect_ts_exported_callable_nodes(
                node,
                source,
                first_definition,
                defs,
                method_nodes,
            );
            return;
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
                    walk_typescript_node_collecting(
                        child,
                        source,
                        file_id,
                        parent_name,
                        (&mut *defs, &mut *method_nodes),
                        angular_components.as_deref_mut(),
                        depth + 1,
                    );
                }
            }
            return;
        }
        _ => {}
    }

    // Default: recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_typescript_node_collecting(
                child,
                source,
                file_id,
                parent_name,
                (&mut *defs, &mut *method_nodes),
                angular_components.as_deref_mut(),
                depth + 1,
            );
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
    let mut modifiers = extract_ts_modifiers(node, source);
    if parent_name.is_none() && !is_exported(node) {
        modifiers.push("local".to_string());
    }
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
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        parent: parent_name.map(|s| s.to_string()),
                        signature: Some(sig.trim().to_string()),
                        modifiers: modifiers.clone(),
                        attributes: Vec::new(),
                        base_types: Vec::new(),
                    });
                }
    }
}

fn collect_ts_exported_callable_nodes<'tree>(
    declaration: tree_sitter::Node<'tree>,
    source: &str,
    first_definition: usize,
    defs: &mut [DefinitionEntry],
    method_nodes: &mut Vec<(usize, tree_sitter::Node<'tree>)>,
) {
    for index in 0..declaration.child_count() {
        let Some(declarator) = declaration
            .child(index)
            .filter(|child| child.kind() == "variable_declarator")
        else {
            continue;
        };
        let Some(name_node) = find_child_by_field(declarator, "name") else {
            continue;
        };
        let Some(value) = find_child_by_field(declarator, "value")
            .filter(|value| matches!(value.kind(), "arrow_function" | "function_expression"))
        else {
            continue;
        };
        let name = node_text(name_node, source);
        if let Some(definition_index) = defs[first_definition..]
            .iter()
            .position(|definition| definition.name == name)
            .map(|offset| first_definition + offset)
        {
            if !defs[definition_index]
                .modifiers
                .iter()
                .any(|modifier| modifier == "callable")
            {
                defs[definition_index].modifiers.push("callable".to_string());
            }
            method_nodes.push((definition_index, value));
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

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_local_var_types(child, source, vars);
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TsBareCallBindingKind {
    SameFile { definition_line: u32 },
    LocalCallable { definition_line: u32 },
    DynamicCallableParameter,
    UnresolvedLocal,
    Imported,
}

#[derive(Clone, Debug)]
struct TsLexicalBinding {
    kind: TsBareCallBindingKind,
    reassigned_at: Vec<usize>,
}

#[derive(Default)]
struct TsLexicalScope {
    parent: Option<usize>,
    bindings: HashMap<String, TsLexicalBinding>,
    function_scope: bool,
}

struct TsCallableDefinition<'tree> {
    name: String,
    parent: Option<String>,
    node: tree_sitter::Node<'tree>,
    line_start: u32,
    line_end: u32,
    is_local: bool,
}

fn ts_callable_signature(callable: &TsCallableDefinition<'_>, source: &str) -> String {
    let params = extract_params_text(callable.node, source).or_else(|| {
        find_child_by_field(callable.node, "parameter")
            .map(|parameter| format!("({})", node_text(parameter, source)))
    });
    let return_type = extract_type_annotation(callable.node, source);
    build_function_signature(
        &callable.name,
        params.as_deref(),
        return_type.as_deref(),
        &[],
    )
}

struct TsLexicalModel<'tree> {
    scopes: Vec<TsLexicalScope>,
    scope_by_node: HashMap<usize, usize>,
    callable_definitions: Vec<TsCallableDefinition<'tree>>,
    named_callable_nodes: std::collections::HashSet<usize>,
    analysis_incomplete: bool,
}

impl<'tree> TsLexicalModel<'tree> {
    fn new(root: tree_sitter::Node<'tree>) -> Self {
        let mut scope_by_node = HashMap::new();
        scope_by_node.insert(root.id(), 0);
        Self {
            scopes: vec![TsLexicalScope {
                parent: None,
                bindings: HashMap::new(),
                function_scope: true,
            }],
            scope_by_node,
            callable_definitions: Vec::new(),
            named_callable_nodes: std::collections::HashSet::new(),
            analysis_incomplete: false,
        }
    }

    fn add_scope(
        &mut self,
        parent: usize,
        node: tree_sitter::Node<'tree>,
        function_scope: bool,
    ) -> usize {
        let scope = self.scopes.len();
        self.scopes.push(TsLexicalScope {
            parent: Some(parent),
            bindings: HashMap::new(),
            function_scope,
        });
        self.scope_by_node.insert(node.id(), scope);
        scope
    }

    fn insert_binding(&mut self, scope: usize, name: String, kind: TsBareCallBindingKind) {
        if let Some(binding) = self.scopes[scope].bindings.get_mut(&name) {
            binding.kind = TsBareCallBindingKind::UnresolvedLocal;
            return;
        }
        self.scopes[scope].bindings.insert(
            name,
            TsLexicalBinding {
                kind,
                reassigned_at: Vec::new(),
            },
        );
    }

    fn binding_scope(&self, mut scope: usize, name: &str) -> Option<usize> {
        loop {
            if self.scopes[scope].bindings.contains_key(name) {
                return Some(scope);
            }
            scope = self.scopes[scope].parent?;
        }
    }

    fn call_crosses_function_scope(&self, mut scope: usize, binding_scope: usize) -> bool {
        while scope != binding_scope {
            if self.scopes[scope].function_scope {
                return true;
            }
            let Some(parent) = self.scopes[scope].parent else {
                return false;
            };
            scope = parent;
        }
        false
    }

    fn nearest_function_scope(&self, mut scope: usize) -> usize {
        loop {
            if self.scopes[scope].function_scope {
                return scope;
            }
            scope = self.scopes[scope].parent.unwrap_or(0);
        }
    }

    fn classify_bare_call(&self, scope: usize, name: &str, call_start: usize) -> CallSiteKind {
        if self.analysis_incomplete {
            return CallSiteKind::TypeScriptAnalysisIncomplete;
        }
        let Some(binding_scope) = self.binding_scope(scope, name) else {
            return CallSiteKind::TypeScriptUnknownGlobal;
        };
        let binding = &self.scopes[binding_scope].bindings[name];
        let may_run_after_reassignment = self.call_crosses_function_scope(scope, binding_scope)
            && !binding.reassigned_at.is_empty();
        if may_run_after_reassignment
            || binding
                .reassigned_at
                .iter()
                .any(|&assignment_start| assignment_start <= call_start)
        {
            return CallSiteKind::TypeScriptUnresolvedLocal;
        }
        match binding.kind {
            TsBareCallBindingKind::SameFile { definition_line } => {
                CallSiteKind::TypeScriptSameFile { definition_line }
            }
            TsBareCallBindingKind::LocalCallable { definition_line } => {
                CallSiteKind::TypeScriptLocalCallable { definition_line }
            }
            TsBareCallBindingKind::DynamicCallableParameter => {
                CallSiteKind::TypeScriptDynamicCallableParameter
            }
            TsBareCallBindingKind::UnresolvedLocal => CallSiteKind::TypeScriptUnresolvedLocal,
            TsBareCallBindingKind::Imported => CallSiteKind::TypeScriptImported,
        }
    }

    fn scope_for_node(&self, node: tree_sitter::Node, fallback: usize) -> usize {
        self.scope_by_node.get(&node.id()).copied().unwrap_or(fallback)
    }
}

fn ts_binding_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(node_text(node, source).to_string());
    }
    find_child_by_field(node, "pattern")
        .or_else(|| find_child_by_field(node, "name"))
        .or_else(|| find_child_by_field(node, "left"))
        .and_then(|child| ts_binding_name(child, source))
}

fn collect_ts_binding_names(
    node: tree_sitter::Node,
    source: &str,
    names: &mut Vec<String>,
    depth: usize,
    analysis_incomplete: &mut bool,
) {
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        *analysis_incomplete = true;
        return;
    }
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            names.push(node_text(node, source).to_string());
            return;
        }
        "pair_pattern" => {
            if let Some(value) = find_child_by_field(node, "value") {
                collect_ts_binding_names(
                    value,
                    source,
                    names,
                    depth + 1,
                    analysis_incomplete,
                );
            }
            return;
        }
        "member_expression" | "subscript_expression" => return,
        _ => {}
    }

    if let Some(pattern) = find_child_by_field(node, "pattern")
        .or_else(|| find_child_by_field(node, "name"))
        .or_else(|| find_child_by_field(node, "left"))
        .or_else(|| find_child_by_field(node, "argument"))
    {
        collect_ts_binding_names(
            pattern,
            source,
            names,
            depth + 1,
            analysis_incomplete,
        );
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_ts_binding_names(
            child,
            source,
            names,
            depth + 1,
            analysis_incomplete,
        );
    }
}

fn ts_binding_names(
    node: tree_sitter::Node,
    source: &str,
    model: &mut TsLexicalModel<'_>,
) -> Vec<String> {
    let mut names = Vec::new();
    collect_ts_binding_names(
        node,
        source,
        &mut names,
        0,
        &mut model.analysis_incomplete,
    );
    names.sort();
    names.dedup();
    names
}

fn ts_reassignment_may_repeat(node: tree_sitter::Node) -> bool {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            "for_statement" | "for_in_statement" | "for_of_statement"
            | "while_statement" | "do_statement" => return true,
            "function_declaration" | "method_definition" | "function_expression"
            | "arrow_function" => return false,
            _ => ancestor = parent.parent(),
        }
    }
    false
}

fn register_ts_parameters(
    callable: tree_sitter::Node,
    source: &str,
    scope: usize,
    model: &mut TsLexicalModel,
) {
    if let Some(parameters) = find_child_by_kind(callable, "formal_parameters") {
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            for name in ts_binding_names(parameter, source, model) {
                model.insert_binding(scope, name, TsBareCallBindingKind::DynamicCallableParameter);
            }
        }
    } else if callable.kind() == "arrow_function"
        && let Some(parameter) = find_child_by_field(callable, "parameter")
    {
        for name in ts_binding_names(parameter, source, model) {
            model.insert_binding(scope, name, TsBareCallBindingKind::DynamicCallableParameter);
        }
    }
}

fn collect_ts_import_names(
    node: tree_sitter::Node,
    source: &str,
    names: &mut Vec<String>,
    depth: usize,
    analysis_incomplete: &mut bool,
) {
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        *analysis_incomplete = true;
        return;
    }
    match node.kind() {
        "import_specifier" => {
            if let Some(local) = find_child_by_field(node, "alias")
                .or_else(|| find_child_by_field(node, "name"))
                && let Some(name) = ts_binding_name(local, source)
            {
                names.push(name);
            }
            return;
        }
        "namespace_import" => {
            let mut cursor = node.walk();
            if let Some(identifier) = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "identifier")
            {
                names.push(node_text(identifier, source).to_string());
            }
            return;
        }
        "identifier" if node.parent().is_some_and(|parent| parent.kind() == "import_clause") => {
            names.push(node_text(node, source).to_string());
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_import_names(
            child,
            source,
            names,
            depth + 1,
            analysis_incomplete,
        );
    }
}

fn collect_ts_lexical_scopes<'tree>(
    node: tree_sitter::Node<'tree>,
    source: &str,
    scope: usize,
    class_name: Option<&str>,
    model: &mut TsLexicalModel<'tree>,
    depth: usize,
) {
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        model.analysis_incomplete = true;
        return;
    }
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            let nested_class_name = find_child_by_field(node, "name")
                .map(|name| node_text(name, source).to_string());
            let class_name = nested_class_name.as_deref().or(class_name);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_ts_lexical_scopes(
                    child,
                    source,
                    scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "function_declaration" => {
            let name = find_child_by_field(node, "name")
                .map(|name| node_text(name, source).to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                let definition_line = node.start_position().row as u32 + 1;
                let kind = if scope == 0 {
                    TsBareCallBindingKind::SameFile { definition_line }
                } else {
                    TsBareCallBindingKind::LocalCallable { definition_line }
                };
                model.insert_binding(scope, name.clone(), kind);
                model.named_callable_nodes.insert(node.id());
                if scope != 0 {
                    model.callable_definitions.push(TsCallableDefinition {
                        name: name.clone(),
                        parent: class_name.map(str::to_string),
                        node,
                        line_start: definition_line,
                        line_end: node.end_position().row as u32 + 1,
                        is_local: true,
                    });
                }
            }
            let function_scope = model.add_scope(scope, node, true);
            register_ts_parameters(node, source, function_scope, model);
            if let Some(body) = find_child_by_kind(node, "statement_block") {
                collect_ts_lexical_scopes(
                    body,
                    source,
                    function_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "method_definition" | "function_expression" | "arrow_function" => {
            let callable_scope = model.add_scope(scope, node, true);
            register_ts_parameters(node, source, callable_scope, model);
            if let Some(body) = find_child_by_kind(node, "statement_block")
                .or_else(|| find_child_by_field(node, "body"))
            {
                collect_ts_lexical_scopes(
                    body,
                    source,
                    callable_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "for_in_statement" | "for_of_statement" => {
            let loop_scope = model.add_scope(scope, node, false);
            let left = find_child_by_field(node, "left");
            if let Some(left) = left {
                for name in ts_binding_names(left, source, model) {
                    model.insert_binding(loop_scope, name, TsBareCallBindingKind::UnresolvedLocal);
                }
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if left.is_some_and(|left| left.id() == child.id()) {
                    continue;
                }
                collect_ts_lexical_scopes(
                    child,
                    source,
                    loop_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "for_statement" => {
            let loop_scope = model.add_scope(scope, node, false);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_ts_lexical_scopes(
                    child,
                    source,
                    loop_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }

        "statement_block" => {
            let block_scope = model.add_scope(scope, node, false);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_ts_lexical_scopes(
                    child,
                    source,
                    block_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "catch_clause" => {
            let catch_scope = model.add_scope(scope, node, false);
            if let Some(parameter) = find_child_by_field(node, "parameter") {
                for name in ts_binding_names(parameter, source, model) {
                    model.insert_binding(catch_scope, name, TsBareCallBindingKind::UnresolvedLocal);
                }
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_ts_lexical_scopes(
                    child,
                    source,
                    catch_scope,
                    class_name,
                    model,
                    depth + 1,
                );
            }
            return;
        }
        "import_statement" => {
            let mut names = Vec::new();
            collect_ts_import_names(
                node,
                source,
                &mut names,
                0,
                &mut model.analysis_incomplete,
            );
            for name in names {
                model.insert_binding(scope, name, TsBareCallBindingKind::Imported);
            }
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut keyword_cursor = node.walk();
            let declaration_scope = if node
                .children(&mut keyword_cursor)
                .any(|child| child.kind() == "var")
            {
                model.nearest_function_scope(scope)
            } else {
                scope
            };
            let mut declarator_cursor = node.walk();
            for declarator in node
                .children(&mut declarator_cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                let Some(name_node) = find_child_by_field(declarator, "name") else {
                    continue;
                };
                let names = ts_binding_names(name_node, source, model);
                if names.is_empty() {
                    continue;
                }
                let value = find_child_by_field(declarator, "value");
                let callable = name_node.kind() == "identifier"
                    && names.len() == 1
                    && value.is_some_and(|value| {
                        matches!(value.kind(), "arrow_function" | "function_expression")
                    });
                if callable {
                    let name = names[0].clone();
                    let definition_line = declarator.start_position().row as u32 + 1;
                    let kind = if declaration_scope == 0 {
                        TsBareCallBindingKind::SameFile { definition_line }
                    } else {
                        TsBareCallBindingKind::LocalCallable { definition_line }
                    };
                    model.insert_binding(declaration_scope, name.clone(), kind);
                    if let Some(value) = value {
                        model.named_callable_nodes.insert(value.id());
                        model.callable_definitions.push(TsCallableDefinition {
                            name: name.clone(),
                            parent: class_name.map(str::to_string),
                            node: value,
                            line_start: definition_line,
                            line_end: value.end_position().row as u32 + 1,
                            is_local: declaration_scope != 0 || !is_exported(node),
                        });
                        collect_ts_lexical_scopes(
                            value,
                            source,
                            declaration_scope,
                            class_name,
                            model,
                            depth + 1,
                        );
                    }
                } else {
                    for name in names {
                        model.insert_binding(
                            declaration_scope,
                            name,
                            TsBareCallBindingKind::UnresolvedLocal,
                        );
                    }
                    if let Some(value) = value {
                        collect_ts_lexical_scopes(
                            value,
                            source,
                            declaration_scope,
                            class_name,
                            model,
                            depth + 1,
                        );
                    }
                }
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_lexical_scopes(
            child,
            source,
            scope,
            class_name,
            model,
            depth + 1,
        );
    }
}

fn collect_ts_reassignments(
    node: tree_sitter::Node,
    source: &str,
    scope: usize,
    model: &mut TsLexicalModel,
    depth: usize,
) {
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        model.analysis_incomplete = true;
        return;
    }
    let scope = model.scope_for_node(node, scope);
    let reassigned = match node.kind() {
        "assignment_expression" | "augmented_assignment_expression" => {
            find_child_by_field(node, "left")
        }
        "update_expression" => {
            find_child_by_field(node, "argument").or_else(|| node.named_child(0))
        }
        _ => None,
    };
    if let Some(reassigned) = reassigned {
        let assignment_start = if ts_reassignment_may_repeat(node) {
            0
        } else {
            node.start_byte()
        };
        for name in ts_binding_names(reassigned, source, model) {
            if let Some(binding_scope) = model.binding_scope(scope, &name) {
                model.scopes[binding_scope].bindings.get_mut(&name)
                    .expect("binding scope must contain binding")
                    .reassigned_at
                    .push(assignment_start);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_reassignments(child, source, scope, model, depth + 1);
    }
}

fn build_ts_lexical_model<'tree>(
    root: tree_sitter::Node<'tree>,
    source: &str,
) -> TsLexicalModel<'tree> {
    let mut model = TsLexicalModel::new(root);
    collect_ts_lexical_scopes(root, source, 0, None, &mut model, 0);
    collect_ts_reassignments(root, source, 0, &mut model, 0);
    model
}


fn extract_ts_call_sites(
    method_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
    lexical_model: &TsLexicalModel,
) -> Vec<CallSite> {
    let mut calls = Vec::new();
    if lexical_model.analysis_incomplete {
        calls.push(CallSite {
            method_name: "<ast-depth-limit>".to_string(),
            receiver_type: None,
            line: method_node.start_position().row as u32 + 1,
            call_kind: CallSiteKind::TypeScriptAnalysisIncomplete,
            receiver_is_generic: false,
        });
    }

    // Find the body (statement_block for methods/functions, or walk the whole node)
    let body = find_child_by_kind(method_node, "statement_block")
        .or_else(|| find_child_by_kind(method_node, "arrow_function"))
        .unwrap_or(method_node);

    let mut combined_types = field_types.clone();
    for (name, type_name) in extract_ts_parameter_types(method_node, source) {
        combined_types.insert(name, type_name);
    }

    let local_vars = extract_ts_local_var_types(body, source);
    for (name, type_name) in local_vars {
        combined_types.entry(name).or_insert(type_name);
    }

    let callable_root = find_child_by_field(method_node, "value")
        .filter(|value| matches!(value.kind(), "arrow_function" | "function_expression"))
        .unwrap_or(method_node);
    let callable_scope = lexical_model.scope_for_node(callable_root, 0);
    let walk_context = TsInvocationWalkContext {
        source,
        class_name,
        field_types: &combined_types,
        lexical_model,
        root_callable_node: callable_root.id(),
    };
    walk_ts_for_invocations(body, callable_scope, &walk_context, &mut calls, 0);

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

fn extract_ts_parameter_types(
    method_node: tree_sitter::Node,
    source: &str,
) -> HashMap<String, String> {
    let mut parameter_types = HashMap::new();
    let Some(parameters) = find_child_by_field(method_node, "parameters")
        .or_else(|| find_child_by_kind(method_node, "formal_parameters"))
    else {
        return parameter_types;
    };

    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(parameter.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        let Some(pattern) = find_child_by_field(parameter, "pattern")
            .or_else(|| find_child_by_field(parameter, "name"))
        else {
            continue;
        };
        if pattern.kind() != "identifier" {
            continue;
        }
        let Some(type_node) = find_child_by_field(parameter, "type") else {
            continue;
        };
        let name = node_text(pattern, source).trim();
        if !name.is_empty()
            && let Some(type_name) = extract_ts_declared_receiver_type(type_node, source)
        {
            parameter_types.insert(name.to_string(), type_name);
        }
    }

    parameter_types
}

fn extract_ts_declared_receiver_type(
    type_node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    match type_node.kind() {
        "type_annotation" | "parenthesized_type" => type_node
            .named_child(0)
            .and_then(|child| extract_ts_declared_receiver_type(child, source)),
        "type_identifier" => normalize_ts_receiver_name(node_text(type_node, source)),
        "nested_type_identifier" | "generic_type" => find_child_by_field(type_node, "name")
            .and_then(|name| extract_ts_declared_receiver_type(name, source)),
        "union_type" => {
            let mut resolved_type: Option<String> = None;
            let mut cursor = type_node.walk();
            for member in type_node.named_children(&mut cursor) {
                let member_text = node_text(member, source).trim();
                if matches!(member_text, "null" | "undefined") {
                    continue;
                }
                let member_type = extract_ts_declared_receiver_type(member, source)?;
                if let Some(ref current_type) = resolved_type {
                    if !current_type.eq_ignore_ascii_case(&member_type) {
                        return None;
                    }
                } else {
                    resolved_type = Some(member_type);
                }
            }
            resolved_type
        }
        _ => None,
    }
}

fn normalize_ts_receiver_name(type_name: &str) -> Option<String> {
    let name = type_name.trim().rsplit('.').next().unwrap_or(type_name).trim();
    if !name.is_empty() && name.chars().next().is_some_and(|character| character.is_uppercase()) {
        Some(name.to_string())
    } else {
        None
    }
}

/// Recursively walk AST looking for call_expression and new_expression nodes.
struct TsInvocationWalkContext<'a, 'tree> {
    source: &'a str,
    class_name: &'a str,
    field_types: &'a HashMap<String, String>,
    lexical_model: &'a TsLexicalModel<'tree>,
    root_callable_node: usize,
}

fn walk_ts_for_invocations(
    node: tree_sitter::Node,
    scope: usize,
    context: &TsInvocationWalkContext<'_, '_>,
    calls: &mut Vec<CallSite>,
    depth: usize,
) {
    if depth > MAX_TYPESCRIPT_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded_at(
            "typescript",
            node,
            MAX_TYPESCRIPT_AST_RECURSION_DEPTH,
        );
        if !calls
            .iter()
            .any(|call| call.call_kind == CallSiteKind::TypeScriptAnalysisIncomplete)
        {
            calls.push(CallSite {
                method_name: "<ast-depth-limit>".to_string(),
                receiver_type: None,
                line: node.start_position().row as u32 + 1,
                call_kind: CallSiteKind::TypeScriptAnalysisIncomplete,
                receiver_is_generic: false,
            });
        }
        return;
    }
    let scope = context.lexical_model.scope_for_node(node, scope);
    if node.id() != context.root_callable_node
        && context.lexical_model.named_callable_nodes.contains(&node.id())
    {
        return;
    }

    match node.kind() {
        "call_expression" => {
            if let Some(call) = extract_ts_call(
                node,
                context.source,
                context.class_name,
                context.field_types,
                context.lexical_model,
                scope,
            ) {
                calls.push(call);
            }
        }
        "new_expression" => {
            if let Some(call) = extract_ts_new_expression(node, context.source) {
                calls.push(call);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_ts_for_invocations(child, scope, context, calls, depth + 1);
    }
}

/// Extract a call site from a call_expression node.
fn extract_ts_call(
    node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
    lexical_model: &TsLexicalModel,
    scope: usize,
) -> Option<CallSite> {
    let func_node = find_child_by_field(node, "function").or_else(|| node.child(0))?;
    let line = node.start_position().row as u32 + 1;

    match func_node.kind() {
        "identifier" => {
            let method_name = node_text(func_node, source).to_string();
            let call_kind = lexical_model.classify_bare_call(
                scope,
                &method_name,
                node.start_byte(),
            );
            Some(CallSite {
                method_name,
                receiver_type: None,
                line,
                call_kind,
                receiver_is_generic: false,
            })
        }
        "member_expression" => extract_ts_member_call(
            func_node,
            source,
            class_name,
            field_types,
            lexical_model,
            scope,
            line,
        ),
        _ => None,
    }
}

/// Extract a call site from a member_expression (e.g., `this.method()`, `service.method()`).
fn extract_ts_member_call(
    member_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
    lexical_model: &TsLexicalModel,
    scope: usize,
    line: u32,
) -> Option<CallSite> {
    let property_node = find_child_by_field(member_node, "property")?;
    let method_name = node_text(property_node, source).to_string();

    let object_node =
        find_child_by_field(member_node, "object").or_else(|| member_node.child(0))?;
    let call_kind = if object_node.kind() == "identifier" {
        let receiver_name = node_text(object_node, source);
        let has_known_type = field_types.contains_key(receiver_name)
            || receiver_name.chars().next().is_some_and(char::is_uppercase);
        match lexical_model.classify_bare_call(scope, receiver_name, member_node.start_byte()) {
            CallSiteKind::TypeScriptImported => CallSiteKind::TypeScriptImported,
            CallSiteKind::TypeScriptAnalysisIncomplete => {
                CallSiteKind::TypeScriptAnalysisIncomplete
            }
            CallSiteKind::TypeScriptUnknownGlobal if !has_known_type => {
                CallSiteKind::TypeScriptUnknownGlobal
            }
            CallSiteKind::TypeScriptDynamicCallableParameter if !has_known_type => {
                CallSiteKind::TypeScriptDynamicCallableParameter
            }
            CallSiteKind::TypeScriptUnresolvedLocal
            | CallSiteKind::TypeScriptSameFile { .. }
            | CallSiteKind::TypeScriptLocalCallable { .. }
                if !has_known_type => CallSiteKind::TypeScriptUnresolvedLocal,
            _ => CallSiteKind::TypeScriptMember,
        }
    } else {
        CallSiteKind::TypeScriptMember
    };
    // Preserve receiver telemetry even when the typed call kind suppresses an edge.
    let receiver_type = resolve_ts_receiver_type(object_node, source, class_name, field_types);

    Some(CallSite {
        method_name,
        receiver_type,
        line,
        call_kind,
        receiver_is_generic: false,
    })
}

fn resolve_known_ts_receiver_type(
    object_node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<String> {
    if object_node.kind() == "identifier" {
        let name = node_text(object_node, source).trim();
        return field_types.get(name).cloned();
    }

    resolve_ts_receiver_type(object_node, source, class_name, field_types)
}

fn resolve_matching_ts_receiver_types(
    left: tree_sitter::Node,
    right: tree_sitter::Node,
    source: &str,
    class_name: &str,
    field_types: &HashMap<String, String>,
) -> Option<String> {
    let left_type = resolve_known_ts_receiver_type(left, source, class_name, field_types)?;
    let right_type = resolve_known_ts_receiver_type(right, source, class_name, field_types)?;
    left_type
        .eq_ignore_ascii_case(&right_type)
        .then_some(left_type)
}

fn ts_has_direct_child_kind(node: tree_sitter::Node, kind: &str) -> bool {
    (0..node.child_count())
        .any(|index| node.child(index).is_some_and(|child| child.kind() == kind))
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
        "new_expression" => extract_type_from_new_expr(object_node, source),
        "parenthesized_expression" => object_node.named_child(0).and_then(|inner| {
            resolve_known_ts_receiver_type(inner, source, class_name, field_types)
        }),
        "ternary_expression" => {
            let consequence = find_child_by_field(object_node, "consequence")?;
            let alternative = find_child_by_field(object_node, "alternative")?;
            resolve_matching_ts_receiver_types(
                consequence,
                alternative,
                source,
                class_name,
                field_types,
            )
        }
        "binary_expression" if ts_has_direct_child_kind(object_node, "??") => {
            let left = find_child_by_field(object_node, "left")?;
            let right = find_child_by_field(object_node, "right")?;
            resolve_matching_ts_receiver_types(
                left,
                right,
                source,
                class_name,
                field_types,
            )
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
        walk_code_stats(body_node, &[], 0, 0, &mut stats, &TYPESCRIPT_CODE_STATS_CONFIG);
    }

    // callCount is filled separately from call_sites after invocations walk
    stats
}

pub(crate) fn count_parameters_typescript(method_node: tree_sitter::Node) -> u8 {
    // Direct formal_parameters child
    let count = find_child_by_kind(method_node, "formal_parameters")
        .or_else(|| {
            // For arrow function fields: public_field_definition -> value -> arrow_function -> formal_parameters
            find_child_by_field(method_node, "value")
                .filter(|v| v.kind() == "arrow_function")
                .and_then(|v| find_child_by_kind(v, "formal_parameters"))
        })
        .map(count_named_children)
        .unwrap_or(0);
    super::tree_sitter_utils::saturate_count_to_u8(count, "typescript_formal_parameters")
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
        call_kind: Default::default(),
        receiver_is_generic: is_generic,
    })
}