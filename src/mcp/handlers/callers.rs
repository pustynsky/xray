//! search_callers handler: call tree building (up/down).

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::ContentIndex;
use crate::definitions::{CallSite, DefinitionEntry, DefinitionIndex, DefinitionKind};
use search_index::generate_trigrams;

use super::HandlerContext;
use super::utils::{inject_branch_warning, json_to_string, sorted_intersect};

/// Built-in JavaScript/TypeScript types whose methods should never be resolved
/// to user-defined classes. When a call site has one of these as its receiver type,
/// we skip candidate matching to avoid false positives (e.g., Promise.resolve()
/// matching user-defined Deferred.resolve()).
const BUILTIN_RECEIVER_TYPES: &[&str] = &[
    // Core
    "Promise", "Array", "Map", "Set", "Object", "String", "Number", "Boolean",
    "Date", "RegExp", "Error", "Symbol", "BigInt", "Function",
    // Static namespaces
    "Math", "JSON", "Reflect", "Proxy", "Intl",
    // Typed arrays
    "Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array",
    "Int32Array", "Uint32Array", "Float32Array", "Float64Array",
    "BigInt64Array", "BigUint64Array",
    // Buffers
    "ArrayBuffer", "SharedArrayBuffer", "DataView",
    // Collections
    "WeakMap", "WeakSet", "WeakRef", "FinalizationRegistry",
    // Browser / Node globals
    "console", "window", "document", "globalThis", "navigator", "localStorage",
    "sessionStorage", "setTimeout", "setInterval", "fetch",
    // Iterators / Generators
    "Iterator", "Generator", "AsyncGenerator", "AsyncIterator",
    // Errors
    "TypeError", "RangeError", "ReferenceError", "SyntaxError", "URIError", "EvalError",
    // C# built-ins (for parity)
    "Task", "List", "Dictionary", "HashSet", "Queue", "Stack",
    "Console", "Convert", "Enum", "Guid", "Nullable",
    "Tuple", "ValueTuple", "Span", "Memory",
];

pub(crate) fn handle_search_callers(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let def_index = match &ctx.def_index {
        Some(idx) => idx,
        None => return ToolCallResult::error(
            "Definition index not available. Start server with --definitions flag.".to_string()
        ),
    };

    let method_name = match args.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return ToolCallResult::error("Missing required parameter: method".to_string()),
    };
    let class_filter = args.get("class").and_then(|v| v.as_str()).map(|s| s.to_string());

    let max_depth = {
        let raw = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        if raw == 0 {
            return ToolCallResult::error(
                "depth must be >= 1. Use depth=1 to find direct callers without recursion.".to_string()
            );
        }
        raw.min(10) as usize
    };
    let direction = {
        let raw = args.get("direction").and_then(|v| v.as_str()).unwrap_or("up");
        let d = raw.to_lowercase();
        if d != "up" && d != "down" {
            return ToolCallResult::error(format!(
                "Invalid direction '{}'. Must be 'up' or 'down'.", raw
            ));
        }
        d
    };
    let direction = direction.as_str();
    let ext_filter = args.get("ext").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.server_ext.clone());
    let resolve_interfaces = args.get("resolveInterfaces").and_then(|v| v.as_bool()).unwrap_or(true);
    let max_callers_per_level = args.get("maxCallersPerLevel").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let max_total_nodes = {
        let raw = args.get("maxTotalNodes").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
        if raw == 0 { usize::MAX } else { raw }
    };
    // Pre-lowercase exclude lists once to avoid repeated allocations in recursive tree functions.
    // The tree functions compare these against lowercased file paths, so pre-lowering is correct.
    let exclude_dir: Vec<String> = args.get("excludeDir")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect())
        .unwrap_or_default();
    let exclude_file: Vec<String> = args.get("excludeFile")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect())
        .unwrap_or_default();

    let search_start = Instant::now();

    let content_index = match ctx.index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire content index lock: {}", e)),
    };
    let def_idx = match def_index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire definition index lock: {}", e)),
    };

    let limits = CallerLimits { max_callers_per_level, max_total_nodes };
    let node_count = AtomicUsize::new(0);

    // Check for ambiguous method names and generate warning
    let method_lower = method_name.to_lowercase();
    let mut ambiguity_warning: Option<String> = None;
    if class_filter.is_none()
        && let Some(name_indices) = def_idx.name_index.get(&method_lower) {
            let method_defs: Vec<&DefinitionEntry> = name_indices.iter()
                .filter_map(|&di| def_idx.definitions.get(di as usize))
                .filter(|d| d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Constructor || d.kind == DefinitionKind::Function)
                .collect();

            let unique_classes: HashSet<&str> = method_defs.iter()
                .filter_map(|d| d.parent.as_deref())
                .collect();

            if unique_classes.len() > 1 {
                let total = unique_classes.len();
                let mut class_list: Vec<&str> = unique_classes.into_iter().collect();
                class_list.sort_unstable();
                const MAX_LISTED: usize = 10;
                if total <= MAX_LISTED {
                    ambiguity_warning = Some(format!(
                        "Method '{}' found in {} classes: {}. Results may mix callers from different classes. Use 'class' parameter to scope the search.",
                        method_name, total, class_list.join(", ")
                    ));
                } else {
                    let shown: Vec<&str> = class_list.into_iter().take(MAX_LISTED).collect();
                    ambiguity_warning = Some(format!(
                        "Method '{}' found in {} classes (showing first {}): {}… Use 'class' parameter to scope the search.",
                        method_name, total, MAX_LISTED, shown.join(", ")
                    ));
                }
            }
        }

    // ─── Angular template tree (check before standard call tree) ─────
    let is_down = direction == "down";
    let template_results = if is_down {
        let mut visited = HashSet::new();
        build_template_callee_tree(&method_name, max_depth, 0, &def_idx, &mut visited)
    } else {
        // For up direction, if method contains '-' it might be a selector
        if method_name.contains('-') {
            let mut visited = HashSet::new();
            find_template_parents(&method_name, max_depth, 0, &def_idx, &mut visited)
        } else {
            Vec::new()
        }
    };

    if !template_results.is_empty() {
        let search_elapsed = search_start.elapsed();
        let mut summary = json!({
            "totalNodes": template_results.len(),
            "templateNavigation": true,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        });
        inject_branch_warning(&mut summary, ctx);
        let mut output = json!({
            "callTree": template_results,
            "query": {
                "method": method_name,
                "direction": direction,
                "depth": max_depth,
                "templateNavigation": true,
            },
            "summary": summary
        });
        if let Some(ref cls) = class_filter {
            output["query"]["class"] = json!(cls);
        }
        return ToolCallResult::success(json_to_string(&output));
    }
    // ─── End Angular template tree ───────────────────────────────────

    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: &ext_filter,
        exclude_dir: &exclude_dir,
        exclude_file: &exclude_file,
        resolve_interfaces,
        limits: &limits,
        node_count: &node_count,
    };

    if direction == "up" {
        let mut visited: HashSet<String> = HashSet::new();
        let tree = build_caller_tree(
            &method_name,
            class_filter.as_deref(),
            max_depth,
            0,
            &caller_ctx,
            &mut visited,
        );

        // Dedup: remove duplicate nodes at root level (can happen with resolveInterfaces)
        let tree = dedup_caller_tree(tree);

        let total_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
        let truncated = total_nodes >= max_total_nodes;
        let search_elapsed = search_start.elapsed();
        let mut summary = json!({
            "nodesVisited": visited.len(),
            "totalNodes": total_nodes,
            "truncated": truncated,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        });
        inject_branch_warning(&mut summary, ctx);
        let mut output = json!({
            "callTree": tree,
            "query": {
                "method": method_name,
                "direction": "up",
                "depth": max_depth,
                "maxCallersPerLevel": max_callers_per_level,
                "maxTotalNodes": max_total_nodes,
            },
            "summary": summary
        });
        if tree.is_empty() && class_filter.is_some() {
            output["hint"] = json!(
                "No callers found. Possible reasons: (1) calls go through extension methods or DI wrappers, (2) class filter is too narrow. Try without 'class' parameter or with the interface name."
            );
        }
        if let Some(ref warning) = ambiguity_warning {
            output["warning"] = json!(warning);
        }
        if let Some(ref cls) = class_filter {
            output["query"]["class"] = json!(cls);
        }
        ToolCallResult::success(json_to_string(&output))
    } else {
        let tree = build_callee_tree(
            &method_name,
            class_filter.as_deref(),
            max_depth,
            0,
            &caller_ctx,
            &mut HashSet::new(),
        );

        let total_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
        let search_elapsed = search_start.elapsed();
        let mut summary = json!({
            "totalNodes": total_nodes,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        });
        inject_branch_warning(&mut summary, ctx);
        let mut output = json!({
            "callTree": tree,
            "query": {
                "method": method_name,
                "direction": "down",
                "depth": max_depth,
                "maxCallersPerLevel": max_callers_per_level,
                "maxTotalNodes": max_total_nodes,
            },
            "summary": summary
        });
        if tree.is_empty() && class_filter.is_some() {
            output["hint"] = json!(
                "No callees found. Possible reasons: (1) method body not parsed, (2) class filter is too narrow. Try without 'class' parameter or with the interface name."
            );
        }
        if let Some(ref warning) = ambiguity_warning {
            output["warning"] = json!(warning);
        }
        if let Some(ref cls) = class_filter {
            output["query"]["class"] = json!(cls);
        }
        ToolCallResult::success(json_to_string(&output))
    }
}

// ─── Internal helpers ───────────────────────────────────────────────

/// Remove duplicate nodes from the caller tree (can occur with resolveInterfaces
/// when the same caller is found through multiple interface implementations).
fn dedup_caller_tree(tree: Vec<Value>) -> Vec<Value> {
    let mut seen: HashSet<String> = HashSet::new();
    tree.into_iter()
        .filter(|node| {
            let key = format!(
                "{}.{}.{}.{}",
                node.get("class").and_then(|v| v.as_str()).unwrap_or("?"),
                node.get("method").and_then(|v| v.as_str()).unwrap_or("?"),
                node.get("file").and_then(|v| v.as_str()).unwrap_or("?"),
                node.get("line").and_then(|v| v.as_u64()).unwrap_or(0),
            );
            seen.insert(key)
        })
        .collect()
}

struct CallerLimits {
    max_callers_per_level: usize,
    max_total_nodes: usize,
}

/// Shared context for caller/callee tree building.
/// Reduces parameter count from 13 to 6 in build_caller_tree.
struct CallerTreeContext<'a> {
    content_index: &'a ContentIndex,
    def_idx: &'a DefinitionIndex,
    ext_filter: &'a str,
    exclude_dir: &'a [String],
    exclude_file: &'a [String],
    resolve_interfaces: bool,
    limits: &'a CallerLimits,
    node_count: &'a AtomicUsize,
}

/// Find the containing method for a given file_id and line number in the definition index.
/// Returns `(name, parent, line_start, definition_index)`.
pub(crate) fn find_containing_method(
    def_idx: &DefinitionIndex,
    file_id: u32,
    line: u32,
) -> Option<(String, Option<String>, u32, u32)> {
    let def_indices = def_idx.file_index.get(&file_id)?;

    let mut best: Option<(u32, &DefinitionEntry)> = None;
    for &di in def_indices {
        if let Some(def) = def_idx.definitions.get(di as usize) {
            match def.kind {
                DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Property | DefinitionKind::Function => {}
                _ => continue,
            }
            if def.line_start <= line && def.line_end >= line {
                if let Some((_, current_best)) = best {
                    if (def.line_end - def.line_start) < (current_best.line_end - current_best.line_start) {
                        best = Some((di, def));
                    }
                } else {
                    best = Some((di, def));
                }
            }
        }
    }

    best.map(|(di, d)| (d.name.clone(), d.parent.clone(), d.line_start, di))
}

/// Pre-compute which content index file_ids contain the parent class token.
/// Filters out files that use the same method name but from a different class.
/// Handles class name, interface name (IClassName), DI implementations, and trigram substring matching.
fn resolve_parent_file_ids(
    parent_class: &str,
    ctx: &CallerTreeContext,
) -> Option<HashSet<u32>> {
    let content_index = ctx.content_index;
    let def_idx = ctx.def_idx;

    let cls_lower = parent_class.to_lowercase();
    let mut file_ids: HashSet<u32> = HashSet::new();

    // Add files containing the class name directly
    if let Some(postings) = content_index.index.get(&cls_lower) {
        file_ids.extend(postings.iter().map(|p| p.file_id));
    }

    // Also check for interface name (IClassName pattern for DI)
    let interface_name = format!("i{}", cls_lower);
    if let Some(postings) = content_index.index.get(&interface_name) {
        file_ids.extend(postings.iter().map(|p| p.file_id));
    }

    // Fuzzy DI: find implementations of I{ClassName} via base_type_index
    // and add files containing those implementation class names
    let impls = find_implementations_of_interface(def_idx, &interface_name);
    for impl_lower in &impls {
        if let Some(postings) = content_index.index.get(impl_lower) {
            file_ids.extend(postings.iter().map(|p| p.file_id));
        }
    }
    // Also find implementations of the class itself (if cls IS an interface)
    let impls_of_cls = find_implementations_of_interface(def_idx, &cls_lower);
    for impl_lower in &impls_of_cls {
        if let Some(postings) = content_index.index.get(impl_lower) {
            file_ids.extend(postings.iter().map(|p| p.file_id));
        }
    }

    // Trigram substring matching: find files where class name appears as a
    // SUBSTRING of another token (e.g. m_storageIndexManager, _storageIndexManager).
    collect_substring_file_ids(&cls_lower, content_index, &mut file_ids);
    collect_substring_file_ids(&interface_name, content_index, &mut file_ids);

    if file_ids.is_empty() { None } else { Some(file_ids) }
}

/// Expand caller tree via interface implementations (direction = "up", depth == 0 only).
/// Finds related interfaces for the target class, then recursively searches for callers
/// of the method through interface implementations.
fn expand_interface_callers(
    method_name: &str,
    method_lower: &str,
    parent_class: Option<&str>,
    max_depth: usize,
    ctx: &CallerTreeContext,
    visited: &mut HashSet<String>,
) -> Vec<Value> {
    let def_idx = ctx.def_idx;
    let name_indices = match def_idx.name_index.get(method_lower) {
        Some(indices) => indices,
        None => return Vec::new(),
    };

    // Pre-compute which interfaces are related to the target class
    let related_interfaces: HashSet<String> = if let Some(pc) = parent_class {
        let pc_lower = pc.to_lowercase();
        let mut related = HashSet::new();
        // I-prefix variant: Foo → IFoo
        related.insert(format!("i{}", pc_lower));
        // Reverse: if target is IFoo, also consider Foo-related interfaces
        if pc_lower.starts_with('i') && pc_lower.len() > 1 {
            related.insert(pc_lower[1..].to_string());
        }
        // Target class's own base_types (interfaces it implements)
        if let Some(indices) = def_idx.name_index.get(&pc_lower) {
            for &idx in indices {
                if let Some(d) = def_idx.definitions.get(idx as usize)
                    && matches!(d.kind, DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record) {
                        for bt in &d.base_types {
                            related.insert(bt.to_lowercase());
                        }
                    }
            }
        }
        // Find implementations of the target class via base_type_index
        let impls = find_implementations_of_interface(def_idx, &pc_lower);
        for impl_name in &impls {
            related.insert(impl_name.clone());
        }
        // Also find implementations of I{ClassName}
        let iface_name = format!("i{}", pc_lower);
        let impls_iface = find_implementations_of_interface(def_idx, &iface_name);
        for impl_name in &impls_iface {
            related.insert(impl_name.clone());
        }
        // Also include the target class itself (it could be an interface)
        related.insert(pc_lower);
        related
    } else {
        HashSet::new() // no filter when no parent_class
    };

    let mut callers: Vec<Value> = Vec::new();

    for &di in name_indices {
        if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }
        if let Some(def) = def_idx.definitions.get(di as usize)
            && let Some(ref parent_class_name) = def.parent {
                let parent_lower = parent_class_name.to_lowercase();

                // Skip interfaces that are NOT related to the target class
                if parent_class.is_some() && !related_interfaces.contains(&parent_lower) {
                    continue;
                }

                if let Some(parent_indices) = def_idx.name_index.get(&parent_lower) {
                    for &pi in parent_indices {
                        if let Some(parent_def) = def_idx.definitions.get(pi as usize)
                            && parent_def.kind == DefinitionKind::Interface
                                && let Some(impl_indices) = def_idx.base_type_index.get(&parent_lower) {
                                    for &ii in impl_indices {
                                        if let Some(impl_def) = def_idx.definitions.get(ii as usize)
                                            && (impl_def.kind == DefinitionKind::Class || impl_def.kind == DefinitionKind::Struct) {
                                                let no_iface_ctx = CallerTreeContext {
                                                    resolve_interfaces: false,
                                                    ..*ctx
                                                };
                                                let impl_callers = build_caller_tree(
                                                    method_name,
                                                    Some(&impl_def.name),
                                                    max_depth,
                                                    1, // current_depth = 1 (interface expansion counts as one level)
                                                    &no_iface_ctx,
                                                    visited,
                                                );
                                                callers.extend(impl_callers);
                                            }
                                    }
                                }
                    }
                }
            }
    }

    callers
}

/// Collect file_ids from the content index where `term` appears as a SUBSTRING of
/// another token. Uses the trigram index for fast O(k) lookup.
/// Handles field naming patterns like m_storageIndexManager, _storageIndexManager, etc.
/// No-op if the trigram index is empty or the term is shorter than 3 chars.
fn collect_substring_file_ids(
    term: &str,
    content_index: &ContentIndex,
    file_ids: &mut HashSet<u32>,
) {
    if term.len() < 3 {
        return; // trigrams require at least 3 chars
    }
    let trigram_idx = &content_index.trigram;
    if trigram_idx.tokens.is_empty() {
        return; // trigram index not built yet
    }

    let trigrams = generate_trigrams(term);
    if trigrams.is_empty() {
        return;
    }

    // Intersect trigram posting lists to find candidate token indices
    let mut candidates: Option<Vec<u32>> = None;
    for tri in &trigrams {
        if let Some(posting_list) = trigram_idx.trigram_map.get(tri) {
            candidates = Some(match candidates {
                None => posting_list.clone(),
                Some(prev) => sorted_intersect(&prev, posting_list),
            });
        } else {
            // Trigram not found → no tokens can contain this term
            return;
        }
    }

    // Verify candidates actually contain the term, then collect their file_ids
    if let Some(candidate_indices) = candidates {
        for &ti in &candidate_indices {
            if let Some(tok) = trigram_idx.tokens.get(ti as usize) {
                // Only match tokens strictly LONGER than the term (substring, not exact)
                if tok.len() > term.len() && tok.contains(term)
                    && let Some(postings) = content_index.index.get(tok) {
                        file_ids.extend(postings.iter().map(|p| p.file_id));
                    }
            }
        }
    }
}

/// Verifies that a call on a specific line actually targets the expected class.
/// Uses pre-computed call-site data from the definition index.
///
/// Returns true if:
/// - The call-site has a receiver_type matching target_class (direct, interface I-prefix, or inheritance)
/// - The call-site has no receiver_type AND the caller is in the same class or inherits from target
/// - No call-site data exists (graceful fallback — don't filter what we can't verify)
/// - target_class is None (no filtering needed)
///
/// Returns false if:
/// - The call-site has a receiver_type that does NOT match target_class
fn verify_call_site_target(
    def_idx: &DefinitionIndex,
    caller_di: u32,
    call_line: u32,
    method_name: &str,
    target_class: Option<&str>,
) -> bool {
    // If no target class specified, accept everything
    let target_class = match target_class {
        Some(tc) => tc,
        None => return true,
    };

    // Get call sites for the caller method from the definition index
    let call_sites = match def_idx.method_calls.get(&caller_di) {
        Some(cs) => cs,
        None => return false, // no call-site data → reject (parser covers all patterns now)
    };

    // Find call sites on the specified line with the matching method name
    let method_name_lower = method_name.to_lowercase();
    let matching_calls: Vec<&CallSite> = call_sites
        .iter()
        .filter(|cs| cs.line == call_line && cs.method_name.to_lowercase() == method_name_lower)
        .collect();

    // If no call-site data found on this line:
    // Method has call-site data but no call at this line →
    // content index matched a comment or non-code text → filter out
    if matching_calls.is_empty() {
        return call_sites.is_empty(); // true only if method has zero call data (shouldn't happen but safe)
    }

    let target_lower = target_class.to_lowercase();
    // Also prepare interface variant: "IFoo" for "Foo"
    let target_interface = format!("i{}", target_lower);

    // Get the caller method's definition to check parent class
    let caller_def = match def_idx.definitions.get(caller_di as usize) {
        Some(d) => d,
        None => return true,
    };
    let caller_parent = caller_def.parent.as_deref();

    // Get target class's base_types for inheritance check
    let target_base_types: Vec<String> = def_idx
        .name_index
        .get(&target_lower)
        .map(|indices| {
            indices
                .iter()
                .filter_map(|&di| def_idx.definitions.get(di as usize))
                .filter(|d| {
                    matches!(
                        d.kind,
                        DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record
                    )
                })
                .flat_map(|d| d.base_types.iter().map(|bt| bt.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();

    // Check if the target class is an extension class for this method.
    // Extension methods can be called on any type (e.g., token.IsValidClrValue()
    // where IsValidClrValue is defined in static class TokenExtensions).
    // If the target class defines this method as an extension method, accept
    // any call site with a matching method name regardless of receiver type.
    // Note: extension_methods keys are original-case but method_name here is lowercased,
    // so we do a case-insensitive scan over the map keys.
    for (ext_method, ext_classes) in &def_idx.extension_methods {
        if ext_method.eq_ignore_ascii_case(method_name)
            && ext_classes.iter().any(|c| c.eq_ignore_ascii_case(target_class)) {
                return true;
            }
    }

    // Check if ANY matching call-site passes verification
    for cs in &matching_calls {
        match &cs.receiver_type {
            Some(rt) => {
                let rt_lower = rt.to_lowercase();
                // Direct match
                if rt_lower == target_lower {
                    return true;
                }
                // Interface match: receiver is IFoo, target is Foo
                if rt_lower == target_interface {
                    return true;
                }
                // Reverse interface: receiver is Foo, target is IFoo
                if target_lower.starts_with('i')
                    && rt_lower == target_lower[1..]
                {
                    return true;
                }
                // Inheritance: target class has base_types containing the receiver_type
                if target_base_types.contains(&rt_lower) {
                    return true;
                }
                // Fuzzy DI interface matching: IDataModelService → DataModelWebService
                // Check if target_class is an implementation of the receiver interface
                // NOTE: pass original-case values — is_implementation_of checks for
                // uppercase 'I' prefix and would always fail on lowercased inputs.
                if is_implementation_of(target_class, rt) {
                    return true;
                }
                // Reverse: receiver is the implementation, target is the interface
                if is_implementation_of(rt, target_class) {
                    return true;
                }
            }
            None => {
                // No receiver type — accept if caller is in the same class or a subclass
                if let Some(cp) = caller_parent {
                    let cp_lower = cp.to_lowercase();
                    if cp_lower == target_lower || cp_lower == target_interface {
                        return true;
                    }
                    // Check if caller's class inherits from target
                    let caller_inherits = def_idx
                        .name_index
                        .get(&cp_lower)
                        .map(|indices| {
                            indices.iter().any(|&di| {
                                def_idx
                                    .definitions
                                    .get(di as usize)
                                    .is_some_and(|d| {
                                        matches!(
                                            d.kind,
                                            DefinitionKind::Class
                                                | DefinitionKind::Struct
                                                | DefinitionKind::Record
                                        ) && d
                                            .base_types
                                            .iter()
                                            .any(|bt| bt.to_lowercase() == target_lower)
                                    })
                            })
                        })
                        .unwrap_or(false);
                    if caller_inherits {
                        return true;
                    }
                }
                // No receiver + different class + no inheritance → false positive
            }
        }
    }

    false
}

/// Build a caller tree recursively (direction = "up").
/// `parent_class` is used to disambiguate common method names -- when recursing,
/// we pass the parent class of the method being searched so that we only find
/// callers that actually reference that specific class (not any unrelated class
/// with a method of the same name).
fn build_caller_tree(
    method_name: &str,
    parent_class: Option<&str>,
    max_depth: usize,
    current_depth: usize,
    ctx: &CallerTreeContext,
    visited: &mut HashSet<String>,
) -> Vec<Value> {
    if current_depth >= max_depth {
        return Vec::new();
    }
    if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes {
        return Vec::new();
    }

    let content_index = ctx.content_index;
    let def_idx = ctx.def_idx;
    let ext_filter = ctx.ext_filter;
    let exclude_dir = ctx.exclude_dir;
    let exclude_file = ctx.exclude_file;
    let resolve_interfaces = ctx.resolve_interfaces;

    let method_lower = method_name.to_lowercase();

    // Find line_start of first matching method definition for overload disambiguation
    let target_line = def_idx.name_index.get(&method_lower)
        .and_then(|indices| indices.iter().find_map(|&di| {
            def_idx.definitions.get(di as usize).and_then(|d| {
                if matches!(d.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function) {
                    if let Some(cls) = parent_class {
                        if d.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(cls)) {
                            return Some(d.line_start);
                        }
                    } else {
                        return Some(d.line_start);
                    }
                }
                None
            })
        }));

    // Use class.method.line as visited key to distinguish overloads
    let visited_key = if let Some(cls) = parent_class {
        format!("{}.{}.{}", cls.to_lowercase(), method_lower, target_line.unwrap_or(0))
    } else {
        format!("{}.{}", method_lower, target_line.unwrap_or(0))
    };
    if !visited.insert(visited_key) {
        return Vec::new();
    }

    let postings = match content_index.index.get(&method_lower) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let parent_file_ids: Option<HashSet<u32>> = parent_class.and_then(|cls| {
        resolve_parent_file_ids(cls, ctx)
    });

    let mut callers: Vec<Value> = Vec::new();
    let mut seen_callers: HashSet<String> = HashSet::new();

    let mut definition_locations: HashSet<(u32, u32)> = HashSet::new();
    if let Some(name_indices) = def_idx.name_index.get(&method_lower) {
        for &di in name_indices {
            if let Some(def) = def_idx.definitions.get(di as usize)
                && (def.kind == DefinitionKind::Method || def.kind == DefinitionKind::Constructor || def.kind == DefinitionKind::Function) {
                    definition_locations.insert((def.file_id, def.line_start));
                }
        }
    }

    for posting in postings {
        if callers.len() >= ctx.limits.max_callers_per_level {
            break;
        }
        if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes {
            break;
        }

        // If we have a parent class context, skip files that don't reference that class
        if let Some(ref pids) = parent_file_ids
            && !pids.contains(&posting.file_id) {
                continue;
            }

        let file_path = match content_index.files.get(posting.file_id as usize) {
            Some(p) => p,
            None => continue,
        };

        let matches_ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                ext_filter.split(',')
                    .any(|allowed| e.eq_ignore_ascii_case(allowed.trim()))
            });
        if !matches_ext { continue; }

        let path_lower = file_path.to_lowercase();
        if exclude_dir.iter().any(|excl| path_lower.contains(excl.as_str())) { continue; }
        if exclude_file.iter().any(|excl| path_lower.contains(excl.as_str())) { continue; }

        let def_fid = match def_idx.path_to_id.get(&std::path::PathBuf::from(file_path)).copied() {
            Some(id) => id,
            None => continue,
        };

        for &line in &posting.lines {
            if callers.len() >= ctx.limits.max_callers_per_level { break; }
            if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }

            if definition_locations.contains(&(def_fid, line)) {
                continue;
            }

            if let Some((caller_name, caller_parent, caller_line, caller_di)) =
                find_containing_method(def_idx, def_fid, line)
            {
                // Verify the call on this line actually targets the expected class
                // using pre-computed call-site data from the AST
                if parent_class.is_some()
                    && !verify_call_site_target(
                        def_idx,
                        caller_di,
                        line,
                        &method_lower,
                        parent_class,
                    ) {
                        continue;
                    }

                let caller_key = format!("{}.{}.{}",
                    caller_parent.as_deref().unwrap_or("?"),
                    &caller_name,
                    caller_line
                );

                if seen_callers.contains(&caller_key) {
                    continue;
                }
                seen_callers.insert(caller_key.clone());

                ctx.node_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Recurse with the CALLER's parent class as the class filter.
                // This ensures that at depth > 0, we search for callers of
                // "CallerClass.CallerMethod" (not just "CallerMethod" across
                // ALL classes), preventing false positives from common method
                // names like Process, Execute, Handle, Run.
                let sub_callers = build_caller_tree(
                    &caller_name,
                    caller_parent.as_deref(),
                    max_depth,
                    current_depth + 1,
                    ctx,
                    visited,
                );

                let mut node = json!({
                    "method": caller_name,
                    "line": caller_line,
                    "callSite": line,
                });
                if let Some(ref parent) = caller_parent {
                    node["class"] = json!(parent);
                }
                if let Some(fname) = Path::new(file_path).file_name().and_then(|f| f.to_str()) {
                    node["file"] = json!(fname);
                }
                if !sub_callers.is_empty() {
                    node["callers"] = json!(sub_callers);
                }
                callers.push(node);
            }
        }
    }

    // Interface resolution: expand to find callers via interface implementations
    if resolve_interfaces && current_depth == 0 {
        let iface_callers = expand_interface_callers(
            method_name, &method_lower, parent_class, max_depth, ctx, visited,
        );
        callers.extend(iface_callers);
    }

    callers
}

/// Build a template callee tree (direction = "down"): find child components
/// referenced in Angular templates. Recursive — follows selector → class → template_children.
fn build_template_callee_tree(
    class_name: &str,
    max_depth: usize,
    current_depth: usize,
    def_idx: &DefinitionIndex,
    visited: &mut HashSet<String>,
) -> Vec<Value> {
    if current_depth >= max_depth {
        return Vec::new();
    }
    let class_lower = class_name.to_lowercase();

    // Find def indices for the target class
    let matching_defs: Vec<u32> = def_idx
        .name_index
        .get(&class_lower)
        .map(|indices| {
            indices
                .iter()
                .filter(|&&di| {
                    def_idx
                        .definitions
                        .get(di as usize)
                        .is_some_and(|d| d.kind == DefinitionKind::Class)
                })
                .copied()
                .collect()
        })
        .unwrap_or_default();

    let mut results: Vec<Value> = Vec::new();
    for di in matching_defs {
        if let Some(children) = def_idx.template_children.get(&di) {
            for child_selector in children {
                if !visited.insert(child_selector.clone()) {
                    continue;
                }
                let mut node = json!({ "selector": child_selector, "templateUsage": true });

                // Resolve child selector → class for recursion
                if let Some(child_def_indices) = def_idx.selector_index.get(child_selector)
                    && let Some(&child_di) = child_def_indices.first()
                        && let Some(child_def) = def_idx.definitions.get(child_di as usize) {
                            node["class"] = json!(child_def.name);
                            node["line"] = json!(child_def.line_start);
                            if let Some(f) = def_idx.files.get(child_def.file_id as usize)
                                && let Some(fname) =
                                    Path::new(f.as_str()).file_name().and_then(|f| f.to_str())
                                {
                                    node["file"] = json!(fname);
                                }
                            let sub = build_template_callee_tree(
                                &child_def.name,
                                max_depth,
                                current_depth + 1,
                                def_idx,
                                visited,
                            );
                            if !sub.is_empty() {
                                node["children"] = json!(sub);
                            }
                        }
                results.push(node);
            }
        }
    }
    results
}

/// Find parent components that reference a given selector in their templates
/// (direction = "up" for Angular template navigation). Recursive — follows
/// selector → parent class → parent's selector → grandparent class → ...
fn find_template_parents(
    selector: &str,
    max_depth: usize,
    current_depth: usize,
    def_idx: &DefinitionIndex,
    visited: &mut HashSet<String>,
) -> Vec<Value> {
    if current_depth >= max_depth {
        return Vec::new();
    }
    if !visited.insert(selector.to_string()) {
        return Vec::new();
    }

    let mut parents: Vec<Value> = Vec::new();
    for (parent_di, children) in &def_idx.template_children {
        if children.iter().any(|c| c == selector)
            && let Some(parent_def) = def_idx.definitions.get(*parent_di as usize) {
                let mut node = json!({
                    "class": parent_def.name,
                    "line": parent_def.line_start,
                    "templateUsage": true,
                });
                if let Some(f) = def_idx.files.get(parent_def.file_id as usize)
                    && let Some(fname) =
                        Path::new(f.as_str()).file_name().and_then(|f| f.to_str())
                    {
                        node["file"] = json!(fname);
                    }
                // Resolve this parent's own selector for recursion
                let mut parent_selector: Option<String> = None;
                for (sel, indices) in &def_idx.selector_index {
                    if indices.contains(parent_di) {
                        node["selector"] = json!(sel);
                        parent_selector = Some(sel.clone());
                        break;
                    }
                }
                // Recurse upward: find grandparents that use this parent's selector
                if let Some(ref ps) = parent_selector {
                    let grandparents = find_template_parents(
                        ps,
                        max_depth,
                        current_depth + 1,
                        def_idx,
                        visited,
                    );
                    if !grandparents.is_empty() {
                        node["parents"] = json!(grandparents);
                    }
                }
                parents.push(node);
            }
    }
    parents
}

/// Build a callee tree (direction = "down"): find what methods are called by this method.
/// Uses pre-computed call graph from AST analysis (method_calls in DefinitionIndex).
fn build_callee_tree(
    method_name: &str,
    class_filter: Option<&str>,
    max_depth: usize,
    current_depth: usize,
    ctx: &CallerTreeContext,
    visited: &mut HashSet<String>,
) -> Vec<Value> {
    if current_depth >= max_depth {
        return Vec::new();
    }
    let def_idx = ctx.def_idx;
    let ext_filter = ctx.ext_filter;
    let exclude_dir = ctx.exclude_dir;
    let exclude_file = ctx.exclude_file;
    let limits = ctx.limits;
    let node_count = ctx.node_count;

    if node_count.load(std::sync::atomic::Ordering::Relaxed) >= limits.max_total_nodes {
        return Vec::new();
    }

    let method_lower = method_name.to_lowercase();

    // Find line_start of first matching method definition for overload disambiguation
    let target_line = def_idx.name_index.get(&method_lower)
        .and_then(|indices| indices.iter().find_map(|&di| {
            def_idx.definitions.get(di as usize).and_then(|d| {
                if matches!(d.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function) {
                    if let Some(cls) = class_filter {
                        if d.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(cls)) {
                            return Some(d.line_start);
                        }
                    } else {
                        return Some(d.line_start);
                    }
                }
                None
            })
        }));

    // Use class.method.line as visit key to distinguish overloads
    let visit_key = if let Some(cls) = class_filter {
        format!("{}.{}.{}", cls.to_lowercase(), method_lower, target_line.unwrap_or(0))
    } else {
        format!("{}.{}", method_lower, target_line.unwrap_or(0))
    };
    if !visited.insert(visit_key) {
        return Vec::new();
    }

    // Find all definitions of this method (with their def_idx indices)
    let method_def_indices: Vec<u32> = def_idx.name_index
        .get(&method_lower)
        .map(|indices| {
            indices.iter()
                .filter(|&&di| {
                    def_idx.definitions.get(di as usize)
                        .is_some_and(|d| {
                            let kind_ok = d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Constructor || d.kind == DefinitionKind::Function;
                            if !kind_ok { return false; }

                            // Apply class filter: only match methods whose parent matches
                            if let Some(cls) = class_filter {
                                let cls_lower = cls.to_lowercase();
                                match &d.parent {
                                    Some(parent) => parent.to_lowercase() == cls_lower,
                                    None => false,
                                }
                            } else {
                                true
                            }
                        })
                })
                .copied()
                .collect()
        })
        .unwrap_or_default();

    if method_def_indices.is_empty() {
        return Vec::new();
    }

    let mut callees: Vec<Value> = Vec::new();
    let mut seen_callees: HashSet<String> = HashSet::new();

    for &method_di in &method_def_indices {
        if callees.len() >= limits.max_callers_per_level { break; }
        if node_count.load(std::sync::atomic::Ordering::Relaxed) >= limits.max_total_nodes { break; }

        // Get pre-computed call sites for this method
        let call_sites = match def_idx.method_calls.get(&method_di) {
            Some(calls) => calls,
            None => continue,
        };

        for call in call_sites {
            if callees.len() >= limits.max_callers_per_level { break; }
            if node_count.load(std::sync::atomic::Ordering::Relaxed) >= limits.max_total_nodes { break; }

            // Resolve this call site to actual definitions
            let caller_parent = def_idx.definitions.get(method_di as usize)
                .and_then(|d| d.parent.as_deref());
            let resolved = resolve_call_site(call, def_idx, caller_parent);

            for callee_di in resolved {
                if callees.len() >= limits.max_callers_per_level { break; }
                if node_count.load(std::sync::atomic::Ordering::Relaxed) >= limits.max_total_nodes { break; }

                let callee_def = match def_idx.definitions.get(callee_di as usize) {
                    Some(d) => d,
                    None => continue,
                };

                let callee_file = def_idx.files.get(callee_def.file_id as usize)
                    .map(|s| s.as_str()).unwrap_or("");

                // Apply extension filter
                let matches_ext = Path::new(callee_file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| {
                        ext_filter.split(',')
                            .any(|allowed| e.eq_ignore_ascii_case(allowed.trim()))
                    });
                if !matches_ext { continue; }

                // Apply directory/file exclusions (exclude lists are pre-lowercased)
                let path_lower = callee_file.to_lowercase();
                if exclude_dir.iter().any(|excl| path_lower.contains(excl.as_str())) { continue; }
                if exclude_file.iter().any(|excl| path_lower.contains(excl.as_str())) { continue; }

                let callee_key = format!("{}.{}.{}",
                    callee_def.parent.as_deref().unwrap_or("?"),
                    &callee_def.name,
                    callee_def.line_start
                );

                if seen_callees.contains(&callee_key) { continue; }
                seen_callees.insert(callee_key.clone());

                node_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let sub_callees = build_callee_tree(
                    &callee_def.name,
                    callee_def.parent.as_deref(), // scope recursion to the callee's own class
                    max_depth,
                    current_depth + 1,
                    ctx,
                    visited,
                );

                let mut node = json!({
                    "method": callee_def.name,
                    "line": callee_def.line_start,
                    "callSiteLine": call.line,
                });
                if let Some(ref parent) = callee_def.parent {
                    node["class"] = json!(parent);
                }
                if let Some(fname) = Path::new(callee_file).file_name().and_then(|f| f.to_str()) {
                    node["file"] = json!(fname);
                }
                if let Some(ref recv) = call.receiver_type {
                    node["receiverType"] = json!(recv);
                }
                if !sub_callees.is_empty() {
                    node["callees"] = json!(sub_callees);
                }
                callees.push(node);
            }
        }
    }

    callees
}

/// Check if `class_name` could be an implementation of `interface_name`.
///
/// Strategy (ordered by reliability):
/// 1. Exact I-prefix convention: IFoo → Foo
/// 2. Suffix-tolerant: strip "I" prefix from interface → stem; check if class_name
///    contains the stem (case-insensitive). E.g., IDataModelService → stem "DataModelService"
///    → matches "DataModelWebService" because it contains "DataModelService".
///
/// To reduce false positives, the stem must be at least 4 characters.
fn is_implementation_of(class_name: &str, interface_name: &str) -> bool {
    // Must start with "I" followed by uppercase
    if !interface_name.starts_with('I') || interface_name.len() < 2 {
        return false;
    }
    let second_char = interface_name.chars().nth(1).unwrap();
    if !second_char.is_uppercase() {
        return false;
    }

    let stem = &interface_name[1..]; // strip "I" prefix

    // Exact match (existing convention)
    if class_name.eq_ignore_ascii_case(stem) {
        return true;
    }

    // Stem too short → skip fuzzy to avoid false positives
    if stem.len() < 4 {
        return false;
    }

    // Suffix-tolerant matching (case-insensitive):
    let class_lower = class_name.to_lowercase();
    let stem_lower = stem.to_lowercase();

    // 1. class_name contains the entire stem as a contiguous substring
    //    e.g., "MyDataModelServiceImpl" contains "datamodelservice"
    if class_lower.contains(&stem_lower) {
        return true;
    }

    // 2. Shared prefix covering at least half the stem length (and at least 4 chars)
    //    e.g., "DataModelWebService" and "DataModelService" share prefix "datamodel" (9 chars)
    //    9 * 2 >= 16 (stem len) → match. This handles cases where extra words are inserted
    //    between stem parts, while avoiding false positives like "DataProcessor" matching
    //    "IDataModelService" (prefix "data" = 4 chars, 4*2=8 < 16 → no match).
    let common_prefix_len = class_lower.bytes()
        .zip(stem_lower.bytes())
        .take_while(|(a, b)| a == b)
        .count();

    common_prefix_len >= 4 && common_prefix_len * 2 >= stem_lower.len()
}

/// Find all classes that implement a given interface, using the base_type_index.
/// Returns lowercased class names.
fn find_implementations_of_interface(
    def_idx: &DefinitionIndex,
    interface_name_lower: &str,
) -> Vec<String> {
    let mut impls = Vec::new();
    if let Some(impl_indices) = def_idx.base_type_index.get(interface_name_lower) {
        for &ii in impl_indices {
            if let Some(impl_def) = def_idx.definitions.get(ii as usize)
                && matches!(impl_def.kind,
                    DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record)
                {
                    impls.push(impl_def.name.to_lowercase());
                }
        }
    }
    impls
}

/// Check if a class (by lowercased name) has generic parameters in its signature.
/// Returns true if ANY class definition with that name has `<` in its signature.
fn is_class_generic(def_idx: &DefinitionIndex, class_name_lower: &str) -> bool {
    if let Some(indices) = def_idx.name_index.get(class_name_lower) {
        for &di in indices {
            if let Some(def) = def_idx.definitions.get(di as usize)
                && matches!(def.kind, DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record | DefinitionKind::Interface)
                    && let Some(ref sig) = def.signature
                        && sig.contains('<') {
                            return true;
                        }
        }
    }
    false
}

/// Resolve a CallSite to actual definition indices in the definition index.
/// Uses receiver_type to disambiguate when available. When receiver is unknown,
/// scopes to the caller's own class if `caller_parent` is provided, otherwise
/// falls back to accepting all matching methods.
pub(crate) fn resolve_call_site(call: &CallSite, def_idx: &DefinitionIndex, caller_parent: Option<&str>) -> Vec<u32> {
    let name_lower = call.method_name.to_lowercase();
    let candidates = match def_idx.name_index.get(&name_lower) {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut resolved: Vec<u32> = Vec::new();

    // Skip matching for built-in types to avoid false positives
    if let Some(ref rt) = call.receiver_type
        && BUILTIN_RECEIVER_TYPES.iter().any(|&b| b.eq_ignore_ascii_case(rt.as_str())) {
            return Vec::new();
        }

    for &di in candidates {
        let def = match def_idx.definitions.get(di as usize) {
            Some(d) => d,
            None => continue,
        };

        // Only match methods, constructors, and functions
        if def.kind != DefinitionKind::Method && def.kind != DefinitionKind::Constructor && def.kind != DefinitionKind::Function {
            continue;
        }

        if let Some(ref recv_type) = call.receiver_type {
            // We have receiver type info -- use it to disambiguate
            let recv_lower = recv_type.to_lowercase();

            if let Some(ref parent) = def.parent {
                let parent_lower = parent.to_lowercase();

                // Direct match: parent class name == receiver type
                if parent_lower == recv_lower {
                    // Generic arity check: if call site is generic (e.g. new List<int>())
                    // but the resolved class is NOT generic, skip — likely BCL name collision
                    if call.receiver_is_generic && !is_class_generic(def_idx, &parent_lower) {
                        continue;
                    }
                    resolved.push(di);
                    continue;
                }

                // Interface match: receiver is an interface, parent implements it
                // Check if parent's class definition has recv_type in base_types
                if let Some(parent_defs) = def_idx.name_index.get(&parent_lower) {
                    for &pi in parent_defs {
                        if let Some(parent_def) = def_idx.definitions.get(pi as usize)
                            && matches!(parent_def.kind,
                                DefinitionKind::Class | DefinitionKind::Struct | DefinitionKind::Record)
                            {
                                let implements = parent_def.base_types.iter()
                                    .any(|bt| {
                                        let bt_base = bt.split('<').next().unwrap_or(bt);
                                        bt_base.eq_ignore_ascii_case(&recv_lower)
                                    });
                                if implements {
                                    resolved.push(di);
                                    break;
                                }
                            }
                    }
                }
            }
        } else {
            // No receiver type -- prefer methods in the same class as the caller
            if let Some(caller_cls) = caller_parent {
                if let Some(ref parent) = def.parent
                    && parent.eq_ignore_ascii_case(caller_cls) {
                        resolved.push(di);
                    }
            } else {
                // No caller class context -- accept all (backward-compatible)
                resolved.push(di);
            }
        }
    }

    resolved
}

// ─── Unit tests for verify_call_site_target ─────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{CallSite, DefinitionEntry, DefinitionIndex, DefinitionKind};
    use std::collections::HashMap;


    /// Helper: build a minimal DefinitionIndex with given definitions and method_calls.
    fn make_def_index(
        definitions: Vec<DefinitionEntry>,
        method_calls: HashMap<u32, Vec<CallSite>>,
    ) -> DefinitionIndex {
        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
        }

        DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["ts".to_string()],
            files: vec!["src/OrderController.ts".to_string(), "src/OrderValidator.ts".to_string()],
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id: HashMap::new(),
            method_calls,
            ..Default::default()
        }
    }

    /// Helper: create a DefinitionEntry for a class.
    fn class_def(file_id: u32, name: &str, base_types: Vec<&str>) -> DefinitionEntry {
        DefinitionEntry {
            file_id,
            name: name.to_string(),
            kind: DefinitionKind::Class,
            line_start: 1,
            line_end: 100,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: base_types.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Helper: create a DefinitionEntry for a method inside a class.
    fn method_def(file_id: u32, name: &str, parent: &str, line_start: u32, line_end: u32) -> DefinitionEntry {
        DefinitionEntry {
            file_id,
            name: name.to_string(),
            kind: DefinitionKind::Method,
            line_start,
            line_end,
            parent: Some(parent.to_string()),
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        }
    }

    // ─── Test 1: Direct receiver match ──────────────────────────────

    #[test]
    fn test_verify_call_site_target_direct_match() {
        // OrderController.processOrder() calls validator.validate() at line 25
        // receiver_type = "OrderValidator"
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
            class_def(1, "OrderValidator", vec![]),                // idx 2
            method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "validate".to_string(),
                receiver_type: Some("OrderValidator".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // caller_di=1 (processOrder), call_line=25, method="validate", target="OrderValidator"
        assert!(verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
    }

    // ─── Test 2: Different receiver → should reject ─────────────────

    #[test]
    fn test_verify_call_site_target_different_receiver() {
        // OrderController.processOrder() calls path.resolve() at line 25
        // receiver_type = "Path" — target is "DependencyTask", should NOT match
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
            class_def(1, "DependencyTask", vec![]),               // idx 2
            method_def(1, "resolve", "DependencyTask", 10, 30),  // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "resolve".to_string(),
                receiver_type: Some("Path".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // receiver is "Path" but target class is "DependencyTask" — should return false
        assert!(!verify_call_site_target(&def_idx, 1, 25, "resolve", Some("DependencyTask")));
    }

    // ─── Test 3: No receiver, same class (implicit this) ────────────

    #[test]
    fn test_verify_call_site_target_no_receiver_same_class() {
        // OrderValidator.check() calls this.validate() at line 55
        // receiver_type = None (implicit this), caller is in OrderValidator
        let definitions = vec![
            class_def(1, "OrderValidator", vec![]),                // idx 0
            method_def(1, "check", "OrderValidator", 50, 70),     // idx 1
            method_def(1, "validate", "OrderValidator", 10, 30),  // idx 2
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "validate".to_string(),
                receiver_type: None,
                line: 55,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // caller is in OrderValidator, target is OrderValidator, no receiver → true
        assert!(verify_call_site_target(&def_idx, 1, 55, "validate", Some("OrderValidator")));
    }

    // ─── Test 4: No receiver, different class ───────────────────────

    #[test]
    fn test_verify_call_site_target_no_receiver_different_class() {
        // OrderController.processOrder() calls validate() at line 25
        // receiver_type = None, caller is in OrderController, target is OrderValidator
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
            class_def(1, "OrderValidator", vec![]),                // idx 2
            method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "validate".to_string(),
                receiver_type: None,
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // caller is in OrderController, target is OrderValidator, no receiver → false
        assert!(!verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
    }

    // ─── Test 5: No target class → always accept ────────────────────

    #[test]
    fn test_verify_call_site_target_no_target_class() {
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "validate".to_string(),
                receiver_type: Some("SomeRandomClass".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // target_class = None → should always return true (no filtering)
        assert!(verify_call_site_target(&def_idx, 1, 25, "validate", None));
    }

    // ─── Test 6: No call-site data → graceful fallback (true) ───────

    #[test]
    fn test_verify_call_site_target_no_call_site_data() {
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
        ];

        // Empty method_calls — no call-site data for any method
        let method_calls = HashMap::new();

        let def_idx = make_def_index(definitions, method_calls);

        // No call-site data → rejection (parser covers all patterns now)
        assert!(!verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
    }

    // ─── Test 7: Interface match (IOrderValidator → OrderValidator) ─

    #[test]
    fn test_verify_call_site_target_interface_match() {
        // OrderController.processOrder() calls validator.validate() at line 25
        // receiver_type = "IOrderValidator", target_class = "OrderValidator"
        // Should match via interface I-prefix convention
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 20, 40), // idx 1
            class_def(1, "OrderValidator", vec!["IOrderValidator"]), // idx 2
            method_def(1, "validate", "OrderValidator", 10, 30),  // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "validate".to_string(),
                receiver_type: Some("IOrderValidator".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // receiver is "IOrderValidator", target is "OrderValidator" → should match via I-prefix
        assert!(verify_call_site_target(&def_idx, 1, 25, "validate", Some("OrderValidator")));
    }

    // ─── Test 8: Comment line — method has call sites but not at queried line ─

    #[test]
    fn test_verify_call_site_target_comment_line_not_real_call() {
        // OrderController.processOrder() has a call to endsWith() at line 10
        // but we query for "resolve" at line 5 where no call site exists
        // → content index matched a comment or non-code text → should return false
        let definitions = vec![
            class_def(0, "OrderController", vec![]),              // idx 0
            method_def(0, "processOrder", "OrderController", 1, 20), // idx 1
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "endsWith".to_string(),
                receiver_type: Some("String".to_string()),
                line: 10,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // Method has call-site data (endsWith at line 10), but no call at line 5
        // → this is a false positive from content index → should return false
        assert!(!verify_call_site_target(&def_idx, 1, 5, "resolve", Some("PathUtils")));
    }

    // ─── Test 9: Pre-filter does NOT expand by base_types ────────────

    #[test]
    fn test_prefilter_does_not_expand_by_base_types() {
        // Scenario:
        // - ResourceManager implements IDisposable, has method Dispose
        // - Many files mention "idisposable" (simulating a large codebase)
        // - Only one file actually calls resourceManager.Dispose()
        // - The pre-filter should NOT include all IDisposable files
        //
        // We test this by running build_caller_tree and verifying that
        // only the file with the actual call is in the results.

        use crate::{ContentIndex, Posting};
        use std::sync::atomic::AtomicUsize;
        use std::path::PathBuf;

        // --- Definition Index ---
        // file 0: ResourceManager.cs (defines ResourceManager : IDisposable + Dispose)
        // file 1: Caller.cs (calls resourceManager.Dispose())
        // files 2..11: IDisposable-mentioning files (no actual Dispose call on ResourceManager)

        let definitions = vec![
            // idx 0: class ResourceManager : IDisposable
            DefinitionEntry {
                file_id: 0,
                name: "ResourceManager".to_string(),
                kind: DefinitionKind::Class,
                line_start: 1,
                line_end: 50,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IDisposable".to_string()],
            },
            // idx 1: method ResourceManager.Dispose
            DefinitionEntry {
                file_id: 0,
                name: "Dispose".to_string(),
                kind: DefinitionKind::Method,
                line_start: 10,
                line_end: 20,
                parent: Some("ResourceManager".to_string()),
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // idx 2: class Caller (in file 1)
            DefinitionEntry {
                file_id: 1,
                name: "Caller".to_string(),
                kind: DefinitionKind::Class,
                line_start: 1,
                line_end: 30,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // idx 3: method Caller.DoWork (contains the actual call)
            DefinitionEntry {
                file_id: 1,
                name: "DoWork".to_string(),
                kind: DefinitionKind::Method,
                line_start: 5,
                line_end: 25,
                parent: Some("Caller".to_string()),
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
        ];

        // Call site: Caller.DoWork calls resourceManager.Dispose() at line 15
        let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
        method_calls.insert(3, vec![
            CallSite {
                method_name: "Dispose".to_string(),
                receiver_type: Some("ResourceManager".to_string()),
                line: 15,
                receiver_is_generic: false,
            },
        ]);

        // Build def index
        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
        }

        let num_files = 12u32; // file 0 + file 1 + 10 IDisposable files
        let mut files_list: Vec<String> = vec![
            "src/ResourceManager.cs".to_string(),
            "src/Caller.cs".to_string(),
        ];
        path_to_id.insert(PathBuf::from("src/ResourceManager.cs"), 0);
        path_to_id.insert(PathBuf::from("src/Caller.cs"), 1);

        for i in 2..num_files {
            let path = format!("src/Service{}.cs", i);
            files_list.push(path.clone());
            path_to_id.insert(PathBuf::from(&path), i);
        }

        let def_idx = DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["cs".to_string()],
            files: files_list.clone(),
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            method_calls,
            ..Default::default()
        };

        // --- Content Index ---
        // Token "dispose" appears in file 0 (definition) and file 1 (actual call)
        // Token "idisposable" appears in files 2..11 (many files mentioning the interface)
        // Token "resourcemanager" appears only in file 0 and file 1
        let mut index: HashMap<String, Vec<Posting>> = HashMap::new();

        // "dispose" in file 0 (definition, line 10) and file 1 (call, line 15)
        index.insert("dispose".to_string(), vec![
            Posting { file_id: 0, lines: vec![10] },
            Posting { file_id: 1, lines: vec![15] },
        ]);

        // "resourcemanager" in file 0 and file 1
        index.insert("resourcemanager".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![15] },
        ]);

        // "idisposable" in many files (simulating common interface)
        let idisposable_postings: Vec<Posting> = (2..num_files)
            .map(|fid| Posting { file_id: fid, lines: vec![1, 5, 10] })
            .collect();
        index.insert("idisposable".to_string(), idisposable_postings);

        let content_index = ContentIndex {
            root: ".".to_string(),
            files: files_list,
            index,
            total_tokens: 100,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![50; num_files as usize],
            ..Default::default()
        };

        // --- Run build_caller_tree ---
        let mut visited = HashSet::new();
        let limits = CallerLimits {
            max_callers_per_level: 50,
            max_total_nodes: 200,
        };
        let node_count = AtomicUsize::new(0);

        let caller_ctx = CallerTreeContext {
            content_index: &content_index,
            def_idx: &def_idx,
            ext_filter: "cs",
            exclude_dir: &[],
            exclude_file: &[],
            resolve_interfaces: false,
            limits: &limits,
            node_count: &node_count,
        };
        let callers = build_caller_tree(
            "Dispose",
            Some("ResourceManager"),
            3,
            0,
            &caller_ctx,
            &mut visited,
        );

        // Should find exactly one caller: Caller.DoWork
        assert_eq!(callers.len(), 1, "Expected exactly 1 caller, got {}: {:?}", callers.len(), callers);
        let caller = &callers[0];
        assert_eq!(caller["method"].as_str().unwrap(), "DoWork");
        assert_eq!(caller["class"].as_str().unwrap(), "Caller");

        // Verify no false positives from IDisposable files
        let caller_file = caller["file"].as_str().unwrap();
        assert_eq!(caller_file, "Caller.cs", "Caller should be from Caller.cs, not an IDisposable file");
    }

    // ─── Test 10: resolve_call_site scopes by caller_parent when no receiver_type ──

    #[test]
    fn test_resolve_call_site_scopes_by_caller_parent() {
        let definitions = vec![
            class_def(0, "ClassA", vec![]),
            method_def(0, "doWork", "ClassA", 5, 15),
            class_def(1, "ClassB", vec![]),
            method_def(1, "doWork", "ClassB", 5, 15),
        ];
        let def_idx = make_def_index(definitions, HashMap::new());
        let call = CallSite {
            method_name: "doWork".to_string(),
            receiver_type: None,
            line: 10,
            receiver_is_generic: false,
        };

        let resolved_a = resolve_call_site(&call, &def_idx, Some("ClassA"));
        assert_eq!(resolved_a.len(), 1);
        assert_eq!(def_idx.definitions[resolved_a[0] as usize].parent.as_deref(), Some("ClassA"));

        let resolved_b = resolve_call_site(&call, &def_idx, Some("ClassB"));
        assert_eq!(resolved_b.len(), 1);
        assert_eq!(def_idx.definitions[resolved_b[0] as usize].parent.as_deref(), Some("ClassB"));

        let resolved_all = resolve_call_site(&call, &def_idx, None);
        assert_eq!(resolved_all.len(), 2);
    }

    // ─── Test 11: build_callee_tree depth=2 no cross-class pollution ──

    #[test]
    fn test_callee_tree_depth2_no_cross_class_pollution() {
        use std::sync::atomic::AtomicUsize;

        let definitions = vec![
            DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 0, name: "process".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 20, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 0, name: "internalWork".to_string(), kind: DefinitionKind::Method, line_start: 22, line_end: 30, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "Helper".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 40, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "run".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 20, parent: Some("Helper".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "helperStep".to_string(), kind: DefinitionKind::Method, line_start: 22, line_end: 35, parent: Some("Helper".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 2, name: "ClassB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 40, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 2, name: "internalWork".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 15, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 2, name: "helperStep".to_string(), kind: DefinitionKind::Method, line_start: 17, line_end: 30, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ];

        let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
        method_calls.insert(1, vec![
            CallSite { method_name: "run".to_string(), receiver_type: Some("Helper".to_string()), line: 10, receiver_is_generic: false },
            CallSite { method_name: "internalWork".to_string(), receiver_type: None, line: 15, receiver_is_generic: false },
        ]);
        method_calls.insert(4, vec![
            CallSite { method_name: "helperStep".to_string(), receiver_type: None, line: 12, receiver_is_generic: false },
        ]);

        let def_idx = make_def_index(definitions, method_calls);
        let mut visited = HashSet::new();
        let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
        let node_count = AtomicUsize::new(0);

        let caller_ctx = CallerTreeContext {
            content_index: &crate::ContentIndex::default(),
            def_idx: &def_idx,
            ext_filter: "ts",
            exclude_dir: &[],
            exclude_file: &[],
            resolve_interfaces: false,
            limits: &limits,
            node_count: &node_count,
        };
        let callees = build_callee_tree("process", Some("ClassA"), 3, 0, &caller_ctx, &mut visited);

        assert_eq!(callees.len(), 2, "Should have 2 callees, got {:?}", callees);
        let callee_names: Vec<(&str, &str)> = callees.iter()
            .map(|c| (c["method"].as_str().unwrap(), c["class"].as_str().unwrap_or("?")))
            .collect();
        assert!(callee_names.contains(&("run", "Helper")));
        assert!(callee_names.contains(&("internalWork", "ClassA")));
        assert!(!callee_names.contains(&("internalWork", "ClassB")), "ClassB.internalWork should NOT appear");

        let run_node = callees.iter().find(|c| c["method"] == "run").unwrap();
        let run_callees = run_node["callees"].as_array().unwrap();
        assert_eq!(run_callees.len(), 1);
        assert_eq!(run_callees[0]["method"].as_str().unwrap(), "helperStep");
        assert_eq!(run_callees[0]["class"].as_str().unwrap(), "Helper", "helperStep should be Helper, not ClassB");
    }

    // ─── Test 12: Generic arity mismatch filters out non-generic class ──

    #[test]
    fn test_resolve_call_site_generic_arity_mismatch() {
        // Scenario: new DataList<int>() should NOT resolve to a non-generic DataList class
        // that happens to have the same name (e.g. ReportRenderingModel.DataList : DataRegion)
        // Note: uses "DataList" instead of "List" because "List" is on the built-in blocklist.
        let definitions = vec![
            // idx 0: non-generic DataList class (user-defined)
            DefinitionEntry {
                file_id: 0, name: "DataList".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 50, parent: None,
                signature: Some("internal sealed class DataList : DataRegion".to_string()),
                modifiers: vec![], attributes: vec![], base_types: vec!["DataRegion".to_string()],
            },
            // idx 1: constructor of non-generic DataList
            DefinitionEntry {
                file_id: 0, name: "DataList".to_string(), kind: DefinitionKind::Constructor,
                line_start: 10, line_end: 20, parent: Some("DataList".to_string()),
                signature: Some("internal DataList(int, ReportProcessing.DataList, ListInstance, RenderingContext)".to_string()),
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ];

        let def_idx = make_def_index(definitions, HashMap::new());

        // Call site: new DataList<CatalogEntry>() — generic call
        let call_generic = CallSite {
            method_name: "DataList".to_string(),
            receiver_type: Some("DataList".to_string()),
            line: 252,
            receiver_is_generic: true, // <-- the key: call site had generics
        };

        // Should NOT resolve because the only DataList class is non-generic
        let resolved = resolve_call_site(&call_generic, &def_idx, None);
        assert!(resolved.is_empty(),
            "Generic call new DataList<CatalogEntry>() should NOT resolve to non-generic DataList class, got {:?}", resolved);

        // Call site: new DataList() — non-generic call
        let call_non_generic = CallSite {
            method_name: "DataList".to_string(),
            receiver_type: Some("DataList".to_string()),
            line: 300,
            receiver_is_generic: false,
        };

        // SHOULD resolve — both non-generic
        let resolved2 = resolve_call_site(&call_non_generic, &def_idx, None);
        assert!(!resolved2.is_empty(),
            "Non-generic call new DataList() SHOULD resolve to non-generic DataList class");
    }

    // ─── Test 13: Built-in Promise.resolve() not matched to user Deferred.resolve() ──

    #[test]
    fn test_builtin_promise_resolve_not_matched() {
        // Scenario: doWork() calls Promise.resolve(42).
        // User has classes UserService.resolve() and Deferred.resolve().
        // The built-in blocklist should prevent resolving Promise.resolve()
        // to any user-defined class.
        let definitions = vec![
            class_def(0, "UserService", vec![]),                          // idx 0
            method_def(0, "resolve", "UserService", 10, 20),             // idx 1
            class_def(1, "Deferred", vec![]),                             // idx 2
            method_def(1, "resolve", "Deferred", 5, 15),                 // idx 3
            class_def(2, "Worker", vec![]),                               // idx 4
            method_def(2, "doWork", "Worker", 1, 30),                    // idx 5
        ];

        let mut method_calls = HashMap::new();
        // doWork calls Promise.resolve(42)
        method_calls.insert(5u32, vec![
            CallSite {
                method_name: "resolve".to_string(),
                receiver_type: Some("Promise".to_string()),
                line: 10,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // resolve_call_site with receiver_type = Promise should return empty
        let call = CallSite {
            method_name: "resolve".to_string(),
            receiver_type: Some("Promise".to_string()),
            line: 10,
            receiver_is_generic: false,
        };

        let resolved = resolve_call_site(&call, &def_idx, Some("Worker"));
        assert!(resolved.is_empty(),
            "Promise.resolve() should NOT resolve to any user-defined class, got {:?}", resolved);
    }

    // ─── Test 14: Built-in Array.map() not matched to user MyCollection.map() ──

    #[test]
    fn test_builtin_array_map_not_matched() {
        // Scenario: processItems() calls Array.map().
        // User has MyCollection.map().
        // The blocklist should prevent resolving Array.map() to MyCollection.map().
        let definitions = vec![
            class_def(0, "MyCollection", vec![]),                         // idx 0
            method_def(0, "map", "MyCollection", 5, 15),                 // idx 1
            class_def(1, "Processor", vec![]),                            // idx 2
            method_def(1, "processItems", "Processor", 1, 20),           // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(3u32, vec![
            CallSite {
                method_name: "map".to_string(),
                receiver_type: Some("Array".to_string()),
                line: 10,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        let call = CallSite {
            method_name: "map".to_string(),
            receiver_type: Some("Array".to_string()),
            line: 10,
            receiver_is_generic: false,
        };

        let resolved = resolve_call_site(&call, &def_idx, Some("Processor"));
        assert!(resolved.is_empty(),
            "Array.map() should NOT resolve to MyCollection.map(), got {:?}", resolved);
    }

    // ─── Test 15: Non-built-in type still matches normally ──

    #[test]
    fn test_non_builtin_type_still_matches() {
        // Scenario: caller calls MyService.process().
        // MyService is NOT a built-in type, so it should still resolve normally.
        let definitions = vec![
            class_def(0, "MyService", vec![]),                            // idx 0
            method_def(0, "process", "MyService", 5, 15),                // idx 1
            class_def(1, "Controller", vec![]),                           // idx 2
            method_def(1, "handleRequest", "Controller", 1, 20),         // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(3u32, vec![
            CallSite {
                method_name: "process".to_string(),
                receiver_type: Some("MyService".to_string()),
                line: 10,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        let call = CallSite {
            method_name: "process".to_string(),
            receiver_type: Some("MyService".to_string()),
            line: 10,
            receiver_is_generic: false,
        };

        let resolved = resolve_call_site(&call, &def_idx, Some("Controller"));
        assert!(!resolved.is_empty(),
            "MyService.process() SHOULD resolve (not a built-in type), got empty");
        assert_eq!(resolved.len(), 1);
        assert_eq!(def_idx.definitions[resolved[0] as usize].parent.as_deref(), Some("MyService"));
    }

    // ─── Test 16: is_implementation_of — exact I-prefix convention ──

    #[test]
    fn test_is_implementation_of_exact_prefix() {
        // IFooService → FooService (exact match after stripping I)
        assert!(is_implementation_of("FooService", "IFooService"));
        assert!(is_implementation_of("fooservice", "IFooService")); // case-insensitive
        assert!(is_implementation_of("FOOSERVICE", "IFooService")); // case-insensitive
    }

    // ─── Test 17: is_implementation_of — suffix-tolerant ──

    #[test]
    fn test_is_implementation_of_suffix_tolerant() {
        // IDataModelService → DataModelWebService (class contains the stem "DataModelService")
        assert!(is_implementation_of("DataModelWebService", "IDataModelService"));
        // stem "DataModelService" is contained in "MyDataModelServiceImpl"
        assert!(is_implementation_of("MyDataModelServiceImpl", "IDataModelService"));
    }

    // ─── Test 18: is_implementation_of — no false positive for short stems ──

    #[test]
    fn test_is_implementation_of_short_stem_no_match() {
        // IFoo → stem "Foo" (3 chars < 4 minimum) → no fuzzy match
        assert!(!is_implementation_of("FooBar", "IFoo"));
        // IFoo → "Foo" exact match should still work
        assert!(is_implementation_of("Foo", "IFoo"));
    }

    // ─── Test 19: is_implementation_of — no false positive for unrelated classes ──

    #[test]
    fn test_is_implementation_of_no_false_positive() {
        // IService → stem "Service" (7 chars, ≥ 4) but "UnrelatedRunner" does NOT contain "service"
        assert!(!is_implementation_of("UnrelatedRunner", "IService"));
        // Not an interface (doesn't start with I + uppercase)
        assert!(!is_implementation_of("FooService", "fooService"));
        assert!(!is_implementation_of("FooService", "invalidName"));
        // "I" alone is not valid
        assert!(!is_implementation_of("Foo", "I"));
    }

    // ─── Test 20: Fuzzy DI interface matching via verify_call_site_target ──

    #[test]
    fn test_verify_call_site_target_fuzzy_interface_match() {
        // IDataModelService → DataModelWebService (suffix-tolerant)
        // Caller calls svc.getData() with receiver_type = "IDataModelService"
        // Target class is "DataModelWebService" — should match via fuzzy DI
        let definitions = vec![
            class_def(0, "SomeController", vec![]),                        // idx 0
            method_def(0, "process", "SomeController", 20, 40),           // idx 1
            class_def(1, "DataModelWebService", vec!["IDataModelService"]), // idx 2
            method_def(1, "getData", "DataModelWebService", 10, 30),      // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "getData".to_string(),
                receiver_type: Some("IDataModelService".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // receiver is IDataModelService, target is DataModelWebService
        // Should match via fuzzy DI: stem "DataModelService" is contained in "DataModelWebService"
        assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("DataModelWebService")),
            "IDataModelService → DataModelWebService should match via fuzzy DI");
    }

    // ─── Test 21: Fuzzy DI — no false positive for unrelated class ──

    #[test]
    fn test_fuzzy_di_no_false_positive() {
        // IService → stem "Service", but "UnrelatedRunner" does NOT contain "Service"
        // So receiver IService should NOT match target UnrelatedRunner
        let definitions = vec![
            class_def(0, "SomeController", vec![]),                     // idx 0
            method_def(0, "process", "SomeController", 20, 40),        // idx 1
            class_def(1, "UnrelatedRunner", vec![]),                    // idx 2
            method_def(1, "run", "UnrelatedRunner", 10, 30),           // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "run".to_string(),
                receiver_type: Some("IService".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // receiver is IService, target is UnrelatedRunner
        // "UnrelatedRunner" does NOT contain "Service" → should NOT match
        assert!(!verify_call_site_target(&def_idx, 1, 25, "run", Some("UnrelatedRunner")),
            "IService → UnrelatedRunner should NOT match (no 'Service' in class name)");
    }

    // ─── Test 22a: Fuzzy DI via verify_call_site_target WITHOUT base_types ──
    // This is the key regression test for BUG #2: is_implementation_of was dead code
    // because verify_call_site_target passed lowercased inputs, but the function
    // checked for uppercase 'I'. Now we pass original-case values.

    #[test]
    fn test_verify_fuzzy_di_without_base_types() {
        // DataModelWebService does NOT declare IDataModelService in base_types
        // but follows the naming convention (contains stem "DataModelService")
        // Receiver is IDataModelService → should match via is_implementation_of
        let definitions = vec![
            class_def(0, "SomeController", vec![]),                        // idx 0
            method_def(0, "process", "SomeController", 20, 40),           // idx 1
            class_def(1, "DataModelWebService", vec![]),                   // idx 2 — NO base_types!
            method_def(1, "getData", "DataModelWebService", 10, 30),      // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "getData".to_string(),
                receiver_type: Some("IDataModelService".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // Without base_types, the only way to match is via is_implementation_of
        // This test would FAIL before the BUG #2 fix (lowercased inputs)
        assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("DataModelWebService")),
            "IDataModelService → DataModelWebService should match via is_implementation_of (fuzzy DI) even without base_types");
    }

    // ─── Test 22b: Reverse fuzzy DI — target is interface, receiver is implementation ──

    #[test]
    fn test_verify_reverse_fuzzy_di_without_base_types() {
        // Target is IDataModelService, receiver is DataModelWebService
        let definitions = vec![
            class_def(0, "SomeController", vec![]),                        // idx 0
            method_def(0, "process", "SomeController", 20, 40),           // idx 1
            class_def(1, "SomeService", vec![]),                           // idx 2
            method_def(1, "getData", "SomeService", 10, 30),              // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "getData".to_string(),
                receiver_type: Some("DataModelWebService".to_string()),
                line: 25,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // Reverse: target is IDataModelService, receiver is DataModelWebService
        assert!(verify_call_site_target(&def_idx, 1, 25, "getData", Some("IDataModelService")),
            "DataModelWebService → IDataModelService should match via reverse is_implementation_of");
    }

    // ─── Test 22: find_implementations_of_interface via base_type_index ──

    #[test]
    fn test_find_implementations_of_interface() {
        let definitions = vec![
            class_def(0, "DataModelWebService", vec!["IDataModelService"]), // idx 0
            class_def(1, "AnotherService", vec!["IDataModelService"]),      // idx 1
        ];

        let mut def_idx = make_def_index(definitions, HashMap::new());
        // Manually populate base_type_index
        def_idx.base_type_index.insert(
            "idatamodelservice".to_string(),
            vec![0, 1],
        );

        let impls = find_implementations_of_interface(&def_idx, "idatamodelservice");
        assert_eq!(impls.len(), 2);
        assert!(impls.contains(&"datamodelwebservice".to_string()));
        assert!(impls.contains(&"anotherservice".to_string()));
    }

    // ─── Test 25: Extension method — verify_call_site_target accepts mismatched receiver ──

    #[test]
    fn test_verify_call_site_target_extension_method() {
        // TokenExtensions (static class) defines IsValidClrValue as an extension method.
        // Consumer.Process calls token.IsValidClrValue() with receiver_type = "TokenType".
        // verify_call_site_target(target="TokenExtensions") should return true because
        // the extension_methods map tells us IsValidClrValue is an extension from TokenExtensions.
        let definitions = vec![
            class_def(0, "TokenExtensions", vec![]),                   // idx 0
            method_def(0, "IsValidClrValue", "TokenExtensions", 5, 10), // idx 1
            class_def(1, "Consumer", vec![]),                          // idx 2
            method_def(1, "Process", "Consumer", 10, 25),              // idx 3
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(3u32, vec![
            CallSite {
                method_name: "IsValidClrValue".to_string(),
                receiver_type: Some("TokenType".to_string()),
                line: 15,
                receiver_is_generic: false,
            },
        ]);

        let mut def_idx = make_def_index(definitions, method_calls);
        // Add extension method mapping
        def_idx.extension_methods.insert(
            "IsValidClrValue".to_string(),
            vec!["TokenExtensions".to_string()],
        );

        // Should match: target is TokenExtensions, which is an extension class for IsValidClrValue
        assert!(verify_call_site_target(&def_idx, 3, 15, "isvalidclrvalue", Some("TokenExtensions")),
            "Extension method IsValidClrValue should match when target class is the extension class");
    }

    #[test]
    fn test_verify_call_site_target_extension_method_no_match_without_map() {
        // Same setup but WITHOUT extension_methods map → should NOT match
        let definitions = vec![
            class_def(0, "TokenExtensions", vec![]),
            method_def(0, "IsValidClrValue", "TokenExtensions", 5, 10),
            class_def(1, "Consumer", vec![]),
            method_def(1, "Process", "Consumer", 10, 25),
        ];

        let mut method_calls = HashMap::new();
        method_calls.insert(3u32, vec![
            CallSite {
                method_name: "IsValidClrValue".to_string(),
                receiver_type: Some("TokenType".to_string()),
                line: 15,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);
        // No extension_methods mapping

        // Should NOT match: receiver is TokenType, target is TokenExtensions, no relationship
        assert!(!verify_call_site_target(&def_idx, 3, 15, "isvalidclrvalue", Some("TokenExtensions")),
            "Without extension_methods map, receiver=TokenType should NOT match target=TokenExtensions");
    }

    // ─── Test 24: Generic method call site — verify_call_site_target matches stripped name ──

    #[test]
    fn test_verify_call_site_target_generic_method_call() {
        // Bug: call sites for generic methods like SearchAsync<T>() were stored
        // with method_name = "SearchAsync<T>" (including type args), causing
        // verify_call_site_target to fail because it compared against "SearchAsync".
        // After the fix, method_name is stored as "SearchAsync" (stripped).
        let definitions = vec![
            class_def(0, "Controller", vec![]),                          // idx 0
            method_def(0, "Handle", "Controller", 10, 30),              // idx 1
            class_def(1, "SearchService", vec!["ISearchService"]),       // idx 2
            method_def(1, "SearchAsync", "SearchService", 5, 20),       // idx 3
        ];

        let mut method_calls = HashMap::new();
        // After fix: method_name is "SearchAsync" (not "SearchAsync<T>")
        method_calls.insert(1u32, vec![
            CallSite {
                method_name: "SearchAsync".to_string(),
                receiver_type: Some("ISearchService".to_string()),
                line: 20,
                receiver_is_generic: false,
            },
        ]);

        let def_idx = make_def_index(definitions, method_calls);

        // This should match: receiver ISearchService, target SearchService
        assert!(verify_call_site_target(&def_idx, 1, 20, "SearchAsync", Some("SearchService")),
            "Generic method call SearchAsync should match when method_name is properly stripped of type args");
    }

    // ─── B3: Template tree navigation tests ─────────────────────────────

    #[test]
    fn test_build_template_callee_tree_one_level() {
        // Parent component has template_children pointing to child selectors
        let definitions = vec![
            class_def(0, "ParentComponent", vec![]),    // idx 0
            class_def(0, "ChildWidget", vec![]),        // idx 1
            class_def(0, "DataGrid", vec![]),           // idx 2
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            // ParentComponent (idx 0) has children: ["child-widget", "data-grid"]
            idx.template_children.insert(0, vec!["child-widget".to_string(), "data-grid".to_string()]);
            // Register selectors mapping to separate components
            idx.selector_index.insert("child-widget".to_string(), vec![1]);
            idx.selector_index.insert("data-grid".to_string(), vec![2]);
            idx
        };

        let mut visited = HashSet::new();
        let result = build_template_callee_tree("ParentComponent", 2, 0, &def_idx, &mut visited);
        assert_eq!(result.len(), 2, "Should find 2 template children");
        let selectors: Vec<&str> = result.iter().filter_map(|n| n["selector"].as_str()).collect();
        assert!(selectors.contains(&"child-widget"));
        assert!(selectors.contains(&"data-grid"));
    }

    #[test]
    fn test_build_template_callee_tree_recursive_depth2() {
        // Parent → child → grandchild
        let definitions = vec![
            class_def(0, "GrandParent", vec![]),   // idx 0
            class_def(0, "ChildComp", vec![]),      // idx 1
            class_def(0, "GrandChild", vec![]),     // idx 2
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["child-comp".to_string()]);
            idx.template_children.insert(1, vec!["grand-child".to_string()]);
            idx.selector_index.insert("child-comp".to_string(), vec![1]);
            idx.selector_index.insert("grand-child".to_string(), vec![2]);
            idx
        };

        let mut visited = HashSet::new();
        let result = build_template_callee_tree("GrandParent", 3, 0, &def_idx, &mut visited);
        assert_eq!(result.len(), 1, "Should find 1 direct child");
        assert_eq!(result[0]["selector"].as_str().unwrap(), "child-comp");
        let children = result[0]["children"].as_array().unwrap();
        assert_eq!(children.len(), 1, "Child should have 1 grandchild");
        assert_eq!(children[0]["selector"].as_str().unwrap(), "grand-child");
    }

    #[test]
    fn test_build_template_callee_tree_cyclic() {
        // Component A uses component B, component B uses component A
        let definitions = vec![
            class_def(0, "CompA", vec![]),  // idx 0
            class_def(0, "CompB", vec![]),  // idx 1
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["comp-b".to_string()]);
            idx.template_children.insert(1, vec!["comp-a".to_string()]);
            idx.selector_index.insert("comp-a".to_string(), vec![0]);
            idx.selector_index.insert("comp-b".to_string(), vec![1]);
            idx
        };

        let mut visited = HashSet::new();
        let result = build_template_callee_tree("CompA", 10, 0, &def_idx, &mut visited);
        // Should not infinite loop — visited set prevents it
        assert_eq!(result.len(), 1, "Should find comp-b as child");
        assert_eq!(result[0]["selector"].as_str().unwrap(), "comp-b");
        // comp-b recurses into CompB which has comp-a as child.
        // comp-a selector was already visited (added when processing CompA's children)
        // so it should be skipped → comp-b has children with comp-a but comp-a has no further children
        // The visited set tracks selectors, not class names:
        // - "comp-b" was inserted when processing CompA's children
        // - When recursing into CompB, "comp-a" is NOT yet in visited (only "comp-b" is)
        // - So comp-a IS added as a child of comp-b
        // - But when comp-a recurses into CompA, "comp-b" is already visited → no further recursion
        // Total tree: CompA -> comp-b -> comp-a (with no further children)
        let children = result[0].get("children");
        assert!(children.is_some(), "comp-b should have children (comp-a)");
        let comp_a_children = children.unwrap().as_array().unwrap();
        assert_eq!(comp_a_children.len(), 1, "comp-b should have exactly 1 child (comp-a)");
        assert_eq!(comp_a_children[0]["selector"].as_str().unwrap(), "comp-a");
        // comp-a should have no further children (comp-b already visited)
        let grandchildren = comp_a_children[0].get("children");
        assert!(grandchildren.is_none() || grandchildren.unwrap().as_array().unwrap().is_empty(),
            "Cycle should be stopped: comp-a -> comp-b already visited");
    }

    #[test]
    fn test_build_template_callee_tree_no_children() {
        // Component with no template_children → empty result
        let definitions = vec![
            class_def(0, "LeafComponent", vec![]),  // idx 0
        ];
        let def_idx = make_def_index(definitions, HashMap::new());

        let mut visited = HashSet::new();
        let result = build_template_callee_tree("LeafComponent", 3, 0, &def_idx, &mut visited);
        assert!(result.is_empty(), "Component with no template_children should return empty");
    }

    #[test]
    fn test_find_template_parents_found() {
        // Selector appears in one parent's template_children
        let definitions = vec![
            class_def(0, "ParentComp", vec![]),  // idx 0
            class_def(0, "ChildComp", vec![]),   // idx 1
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["child-comp".to_string()]);
            idx.selector_index.insert("parent-comp".to_string(), vec![0]);
            idx
        };

        let mut visited = HashSet::new();
        let result = find_template_parents("child-comp", 3, 0, &def_idx, &mut visited);
        assert_eq!(result.len(), 1, "Should find 1 parent");
        assert_eq!(result[0]["class"].as_str().unwrap(), "ParentComp");
    }

    #[test]
    fn test_find_template_parents_multiple() {
        // Selector appears in two parents
        let definitions = vec![
            class_def(0, "ParentA", vec![]),  // idx 0
            class_def(0, "ParentB", vec![]),  // idx 1
            class_def(0, "SharedChild", vec![]),   // idx 2
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["shared-child".to_string()]);
            idx.template_children.insert(1, vec!["shared-child".to_string()]);
            idx
        };

        let mut visited = HashSet::new();
        let result = find_template_parents("shared-child", 3, 0, &def_idx, &mut visited);
        assert_eq!(result.len(), 2, "Should find 2 parents using the selector");
        let parent_names: Vec<&str> = result.iter().filter_map(|n| n["class"].as_str()).collect();
        assert!(parent_names.contains(&"ParentA"));
        assert!(parent_names.contains(&"ParentB"));
    }

    #[test]
    fn test_find_template_parents_recursive_depth() {
        // grandchild → child → parent
        // Searching UP from "grand-child" with depth=3 should find:
        //   level 1: ChildComp (direct parent)
        //   level 2: GrandParent (parent of ChildComp, nested in "parents")
        let definitions = vec![
            class_def(0, "GrandParent", vec![]),   // idx 0
            class_def(0, "ChildComp", vec![]),      // idx 1
            class_def(0, "GrandChild", vec![]),     // idx 2
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            // GrandParent uses child-comp in its template
            idx.template_children.insert(0, vec!["child-comp".to_string()]);
            // ChildComp uses grand-child in its template
            idx.template_children.insert(1, vec!["grand-child".to_string()]);
            // Register selectors
            idx.selector_index.insert("grand-parent".to_string(), vec![0]);
            idx.selector_index.insert("child-comp".to_string(), vec![1]);
            idx.selector_index.insert("grand-child".to_string(), vec![2]);
            idx
        };

        // Search UP from "grand-child" with depth=3
        let mut visited = HashSet::new();
        let result = find_template_parents("grand-child", 3, 0, &def_idx, &mut visited);

        // Level 1: ChildComp is a direct parent (its template contains "grand-child")
        assert!(result.iter().any(|n| n["class"].as_str() == Some("ChildComp")),
            "Should find ChildComp as direct parent, got {:?}", result);

        // Level 2: GrandParent is a grandparent, nested in ChildComp's "parents" field
        let child_node = result.iter().find(|n| n["class"].as_str() == Some("ChildComp")).unwrap();
        let grandparents = child_node["parents"].as_array()
            .expect("ChildComp should have 'parents' field with grandparents");
        assert!(grandparents.iter().any(|p| p["class"].as_str() == Some("GrandParent")),
            "Should find GrandParent as grandparent (depth 2). Got parents: {:?}", grandparents);
    }

    #[test]
    fn test_find_template_parents_respects_max_depth() {
        // Same 3-level hierarchy, but depth=1 should only return direct parent
        let definitions = vec![
            class_def(0, "GrandParent", vec![]),   // idx 0
            class_def(0, "ChildComp", vec![]),      // idx 1
            class_def(0, "GrandChild", vec![]),     // idx 2
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["child-comp".to_string()]);
            idx.template_children.insert(1, vec!["grand-child".to_string()]);
            idx.selector_index.insert("grand-parent".to_string(), vec![0]);
            idx.selector_index.insert("child-comp".to_string(), vec![1]);
            idx.selector_index.insert("grand-child".to_string(), vec![2]);
            idx
        };

        let mut visited = HashSet::new();
        let result = find_template_parents("grand-child", 1, 0, &def_idx, &mut visited);

        // depth=1: only direct parent, no recursion
        assert_eq!(result.len(), 1, "Should find exactly 1 parent with depth=1");
        assert_eq!(result[0]["class"].as_str().unwrap(), "ChildComp");
        assert!(result[0].get("parents").is_none(),
            "With depth=1, should NOT recurse to find grandparents");
    }

    #[test]
    fn test_find_template_parents_cyclic() {
        // CompA uses comp-b, CompB uses comp-a — upward should not infinite loop
        let definitions = vec![
            class_def(0, "CompA", vec![]),  // idx 0
            class_def(0, "CompB", vec![]),  // idx 1
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["comp-b".to_string()]);
            idx.template_children.insert(1, vec!["comp-a".to_string()]);
            idx.selector_index.insert("comp-a".to_string(), vec![0]);
            idx.selector_index.insert("comp-b".to_string(), vec![1]);
            idx
        };

        let mut visited = HashSet::new();
        let result = find_template_parents("comp-a", 10, 0, &def_idx, &mut visited);
        // Should find CompB as parent (uses comp-a), then CompB's selector is comp-b,
        // searching for comp-b finds CompA as parent, but comp-a is already visited → stops
        assert!(!result.is_empty(), "Should find at least CompB as parent");
        // Should not panic or infinite loop
    }

    #[test]
    fn test_find_template_parents_not_found() {
        // Non-existent selector → empty result
        let definitions = vec![
            class_def(0, "SomeComp", vec![]),  // idx 0
        ];
        let def_idx = {
            let mut idx = make_def_index(definitions, HashMap::new());
            idx.template_children.insert(0, vec!["existing-child".to_string()]);
            idx
        };

        let mut visited = HashSet::new();
        let result = find_template_parents("nonexistent-selector", 3, 0, &def_idx, &mut visited);
        assert!(result.is_empty(), "Non-existent selector should return empty");
    }

    // ─── Test: F-10 fix — class filter preserved during upward recursion ──

    #[test]
    fn test_caller_tree_preserves_class_filter_during_recursion() {
        // Scenario: We search for callers of ClassA.Process() (a common method name).
        // ClassB.Handle() calls classA.Process() → this is a valid caller at depth 0.
        // ClassC.Run() calls classB.Handle() → this is a valid caller at depth 1.
        // ClassD.Execute() also has a method named "Handle" but it's ClassD.Handle(),
        //   NOT ClassB.Handle(). Without the fix (passing None at recursion), depth 1
        //   would find ClassD.Execute() as a false positive because it searches for
        //   ALL "Handle" methods without class scoping.
        //
        // With the fix: at depth 1, we search for callers of "Handle" with class filter
        // "ClassB" (the caller's parent), so ClassD.Execute() is correctly excluded.

        use crate::{ContentIndex, Posting};
        use std::sync::atomic::AtomicUsize;
        use std::path::PathBuf;

        // --- Definition Index ---
        let definitions = vec![
            // idx 0: class ClassA
            DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 1: ClassA.Process (target method)
            DefinitionEntry { file_id: 0, name: "Process".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 20, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 2: class ClassB
            DefinitionEntry { file_id: 1, name: "ClassB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 3: ClassB.Handle (calls ClassA.Process)
            DefinitionEntry { file_id: 1, name: "Handle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 4: class ClassC
            DefinitionEntry { file_id: 2, name: "ClassC".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 5: ClassC.Run (calls ClassB.Handle)
            DefinitionEntry { file_id: 2, name: "Run".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassC".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 6: class ClassD (unrelated class with same method name "Handle")
            DefinitionEntry { file_id: 3, name: "ClassD".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 7: ClassD.Handle (DIFFERENT method, same name — should NOT appear)
            DefinitionEntry { file_id: 3, name: "Handle".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassD".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 8: class ClassE
            DefinitionEntry { file_id: 4, name: "ClassE".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            // idx 9: ClassE.Execute (calls ClassD.Handle — should NOT be in results)
            DefinitionEntry { file_id: 4, name: "Execute".to_string(), kind: DefinitionKind::Method, line_start: 10, line_end: 30, parent: Some("ClassE".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ];

        let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
        // ClassB.Handle calls ClassA.Process at line 20
        method_calls.insert(3, vec![
            CallSite { method_name: "Process".to_string(), receiver_type: Some("ClassA".to_string()), line: 20, receiver_is_generic: false },
        ]);
        // ClassC.Run calls ClassB.Handle at line 15
        method_calls.insert(5, vec![
            CallSite { method_name: "Handle".to_string(), receiver_type: Some("ClassB".to_string()), line: 15, receiver_is_generic: false },
        ]);
        // ClassE.Execute calls ClassD.Handle at line 15
        method_calls.insert(9, vec![
            CallSite { method_name: "Handle".to_string(), receiver_type: Some("ClassD".to_string()), line: 15, receiver_is_generic: false },
        ]);

        // Build def index
        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
        }

        let files_list = vec![
            "src/ClassA.cs".to_string(),
            "src/ClassB.cs".to_string(),
            "src/ClassC.cs".to_string(),
            "src/ClassD.cs".to_string(),
            "src/ClassE.cs".to_string(),
        ];
        for (i, f) in files_list.iter().enumerate() {
            path_to_id.insert(PathBuf::from(f), i as u32);
        }

        let def_idx = DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["cs".to_string()],
            files: files_list.clone(),
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            method_calls,
            code_stats: HashMap::new(),
            parse_errors: 0,
            lossy_file_count: 0,
            empty_file_ids: Vec::new(),
            extension_methods: HashMap::new(),
            selector_index: HashMap::new(),
            template_children: HashMap::new(),
        };

        // --- Content Index ---
        let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
        // "process" appears in file 0 (definition) and file 1 (ClassB calls it)
        index.insert("process".to_string(), vec![
            Posting { file_id: 0, lines: vec![10] },
            Posting { file_id: 1, lines: vec![20] },
        ]);
        // "handle" appears in files 1, 2, 3, 4
        index.insert("handle".to_string(), vec![
            Posting { file_id: 1, lines: vec![10] },  // definition of ClassB.Handle
            Posting { file_id: 2, lines: vec![15] },  // ClassC calls ClassB.Handle
            Posting { file_id: 3, lines: vec![10] },  // definition of ClassD.Handle
            Posting { file_id: 4, lines: vec![15] },  // ClassE calls ClassD.Handle
        ]);
        // Class name tokens for pre-filtering
        index.insert("classa".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![20] },
        ]);
        index.insert("classb".to_string(), vec![
            Posting { file_id: 1, lines: vec![1] },
            Posting { file_id: 2, lines: vec![15] },
        ]);
        index.insert("classd".to_string(), vec![
            Posting { file_id: 3, lines: vec![1] },
            Posting { file_id: 4, lines: vec![15] },
        ]);

        let content_index = ContentIndex {
            root: ".".to_string(),
            files: files_list,
            index,
            total_tokens: 100,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![50; 5],
            ..Default::default()
        };

        // --- Run build_caller_tree with depth=3 ---
        let mut visited = HashSet::new();
        let limits = CallerLimits { max_callers_per_level: 50, max_total_nodes: 200 };
        let node_count = AtomicUsize::new(0);

        let caller_ctx = CallerTreeContext {
            content_index: &content_index,
            def_idx: &def_idx,
            ext_filter: "cs",
            exclude_dir: &[],
            exclude_file: &[],
            resolve_interfaces: false,
            limits: &limits,
            node_count: &node_count,
        };
        let callers = build_caller_tree(
            "Process",
            Some("ClassA"),
            3,  // depth 3: should find ClassB.Handle (depth 0) → ClassC.Run (depth 1)
            0,
            &caller_ctx,
            &mut visited,
        );

        // Depth 0: should find ClassB.Handle as the caller of ClassA.Process
        assert_eq!(callers.len(), 1, "Expected 1 caller at depth 0, got {}: {:?}", callers.len(), callers);
        assert_eq!(callers[0]["method"].as_str().unwrap(), "Handle");
        assert_eq!(callers[0]["class"].as_str().unwrap(), "ClassB");

        // Depth 1: ClassB.Handle should have ClassC.Run as its caller
        let sub_callers = callers[0]["callers"].as_array()
            .expect("ClassB.Handle should have sub-callers");
        assert_eq!(sub_callers.len(), 1, "Expected 1 sub-caller of ClassB.Handle, got {}: {:?}", sub_callers.len(), sub_callers);
        assert_eq!(sub_callers[0]["method"].as_str().unwrap(), "Run");
        assert_eq!(sub_callers[0]["class"].as_str().unwrap(), "ClassC");

        // CRITICAL: ClassE.Execute should NOT appear anywhere in the tree.
        // It calls ClassD.Handle, not ClassB.Handle. Without the F-10 fix,
        // it would appear as a false positive because the class filter was
        // dropped at depth > 0.
        let total_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(total_nodes, 2, "Should have exactly 2 nodes (ClassB.Handle + ClassC.Run), got {}", total_nodes);
    }

    // ─── F-1: Hint when 0 callers with class filter ─────────────────

    #[test]
    fn test_search_callers_hint_when_empty_with_class_filter() {
        // When class filter is set and callTree is empty, response should include a hint
        let definitions = vec![
            class_def(0, "OrderService", vec![]),
            method_def(0, "process", "OrderService", 5, 15),
        ];
        let def_idx = make_def_index(definitions, HashMap::new());

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            files: vec!["src/OrderController.ts".to_string()],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["ts".to_string()],
            file_token_counts: vec![0],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
            ..Default::default()
        };

        // Search for callers of a method with a class filter that yields 0 results
        let result = handle_search_callers(&ctx, &serde_json::json!({
            "method": "process",
            "class": "NonExistentClass",
            "depth": 1
        }));
        assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v.get("hint").is_some(),
            "Should have hint when callTree is empty with class filter. Got: {}",
            serde_json::to_string_pretty(&v).unwrap());
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("class"), "Hint should mention class parameter");
    }

    #[test]
    fn test_search_callers_no_hint_without_class_filter() {
        // When no class filter is set, no hint should appear even if tree is empty
        let definitions = vec![
            class_def(0, "OrderService", vec![]),
            method_def(0, "process", "OrderService", 5, 15),
        ];
        let def_idx = make_def_index(definitions, HashMap::new());

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            files: vec!["src/OrderController.ts".to_string()],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["ts".to_string()],
            file_token_counts: vec![0],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
            ..Default::default()
        };

        // Search without class filter — no hint expected
        let result = handle_search_callers(&ctx, &serde_json::json!({
            "method": "nonexistentmethod",
            "depth": 1
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v.get("hint").is_none(),
            "Should NOT have hint when no class filter is set");
    }

    #[test]
    fn test_search_callers_no_hint_when_results_found() {
        // When callers ARE found with class filter, no hint should appear
        use crate::{ContentIndex, Posting};
        use std::path::PathBuf;

        let definitions = vec![
            class_def(0, "OrderService", vec![]),
            method_def(0, "process", "OrderService", 5, 15),
            class_def(1, "Consumer", vec![]),
            method_def(1, "run", "Consumer", 5, 20),
        ];

        let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
        method_calls.insert(3, vec![
            CallSite {
                method_name: "process".to_string(),
                receiver_type: Some("OrderService".to_string()),
                line: 10,
                receiver_is_generic: false,
            },
        ]);

        let mut def_idx = make_def_index(definitions, method_calls);
        def_idx.path_to_id.insert(PathBuf::from("src/OrderController.ts"), 0);
        def_idx.path_to_id.insert(PathBuf::from("src/OrderValidator.ts"), 1);

        let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
        index.insert("process".to_string(), vec![
            Posting { file_id: 0, lines: vec![5] },
            Posting { file_id: 1, lines: vec![10] },
        ]);
        index.insert("orderservice".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![10] },
        ]);

        let content_index = ContentIndex {
            root: ".".to_string(),
            files: vec![
                "src/OrderController.ts".to_string(),
                "src/OrderValidator.ts".to_string(),
            ],
            index,
            total_tokens: 100,
            extensions: vec!["ts".to_string()],
            file_token_counts: vec![50, 50],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_idx))),
            ..Default::default()
        };

        let result = handle_search_callers(&ctx, &serde_json::json!({
            "method": "process",
            "class": "OrderService",
            "depth": 1
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let tree = v["callTree"].as_array().unwrap();
        if !tree.is_empty() {
            assert!(v.get("hint").is_none(),
                "Should NOT have hint when callers are found");
        }
    }

    // ─── Test 23: resolve_call_site resolves via base_types (existing behavior preserved) ──

    #[test]
    fn test_resolve_call_site_via_base_types() {
        // IDataModelService.getData() should resolve to DataModelWebService.getData()
        // via base_types in the class definition
        let definitions = vec![
            class_def(0, "DataModelWebService", vec!["IDataModelService"]), // idx 0
            method_def(0, "getData", "DataModelWebService", 10, 30),       // idx 1
        ];

        let def_idx = make_def_index(definitions, HashMap::new());

        let call = CallSite {
            method_name: "getData".to_string(),
            receiver_type: Some("IDataModelService".to_string()),
            line: 5,
            receiver_is_generic: false,
        };

        let resolved = resolve_call_site(&call, &def_idx, None);
        assert!(!resolved.is_empty(),
            "IDataModelService.getData() should resolve to DataModelWebService.getData via base_types");
    }
}