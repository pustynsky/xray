//! search_callers handler: call tree building (up/down).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::ContentIndex;
use crate::definitions::{CallSite, DefinitionEntry, DefinitionIndex, DefinitionKind};
use search_index::generate_trigrams;

use super::HandlerContext;
use super::utils::{inject_body_into_obj, inject_branch_warning, json_to_string, sorted_intersect};

/// Test attribute markers (lowercase) used to identify test methods.
/// Covers: NUnit [Test], xUnit [Fact]/[Theory], MSTest [TestMethod],
/// Rust #[test]/#[tokio::test], and similar frameworks.
const TEST_ATTRIBUTE_MARKERS: &[&str] = &[
    "test",         // NUnit [Test], Rust #[test], #[tokio::test]
    "fact",         // xUnit [Fact]
    "theory",       // xUnit [Theory]
    "testmethod",   // MSTest [TestMethod]
];

/// File name patterns (lowercase) that indicate TypeScript/JavaScript test files.
/// Used as a heuristic since describe()/it() are call expressions, not decorators.
const TEST_FILE_PATTERNS: &[&str] = &[
    ".spec.ts", ".test.ts", ".spec.tsx", ".test.tsx",
    ".spec.js", ".test.js",
];

/// Check if a definition entry represents a test method.
/// Uses two strategies:
/// 1. Attribute-based: checks for test framework attributes (C#, Rust)
/// 2. File-name-based: checks for test file patterns (TypeScript/JavaScript)
fn is_test_method(def: &DefinitionEntry, file_path: &str) -> bool {
    // Strategy 1: check attributes (C#, Rust)
    for attr in &def.attributes {
        let lower = attr.to_lowercase();
        for marker in TEST_ATTRIBUTE_MARKERS {
            if lower.contains(marker) {
                return true;
            }
        }
    }

    // Strategy 2: file name heuristic (TypeScript/JavaScript)
    let file_lower = file_path.to_lowercase();
    for pattern in TEST_FILE_PATTERNS {
        if file_lower.ends_with(pattern) {
            return true;
        }
    }

    false
}

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

    let method_raw = match args.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return ToolCallResult::error("Missing required parameter: method".to_string()),
    };
    let class_filter = args.get("class").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Multi-method support: split by comma
    let methods: Vec<String> = method_raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if methods.is_empty() {
        return ToolCallResult::error("method parameter is empty after parsing".to_string());
    }

    // For multi-method batch, delegate to batch handler
    if methods.len() > 1 {
        return handle_multi_method_callers(ctx, args, def_index, &methods, class_filter.as_deref());
    }

    // Single method — existing behavior (backward compatible)
    let method_name = methods.into_iter().next().unwrap();

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

    // Body injection parameters
    let include_doc_comments = args.get("includeDocComments").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_body = args.get("includeBody").and_then(|v| v.as_bool()).unwrap_or(false)
        || include_doc_comments; // includeDocComments implies includeBody
    let max_body_lines = args.get("maxBodyLines").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let max_total_body_lines = args.get("maxTotalBodyLines").and_then(|v| v.as_u64()).unwrap_or(300) as usize;

    // Impact analysis parameter
    let impact_analysis = args.get("impactAnalysis").and_then(|v| v.as_bool()).unwrap_or(false);
    if impact_analysis && direction != "up" {
        return ToolCallResult::error(
            "impactAnalysis only works with direction='up'. It traces callers upward to find test methods covering the target.".to_string()
        );
    }

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
                .filter(|d| d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Constructor || d.kind == DefinitionKind::Function
                    || d.kind == DefinitionKind::StoredProcedure || d.kind == DefinitionKind::SqlFunction)
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
        include_body,
        include_doc_comments,
        max_body_lines,
        max_total_body_lines,
        impact_analysis,
    };

    // Mutable state for body injection (shared across recursive calls)
    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total_body_lines_emitted: usize = 0;

    // Build root method info (for includeBody — includes the searched method's own body)
    let root_method = if include_body {
        build_root_method_info(
            &method_lower,
            class_filter.as_deref(),
            &def_idx,
            &mut file_cache,
            &mut total_body_lines_emitted,
            max_body_lines,
            max_total_body_lines,
            include_doc_comments,
        )
    } else {
        None
    };

    if direction == "up" {
        let mut visited: HashSet<String> = HashSet::new();
        let mut tests_found: Vec<Value> = Vec::new();
        let initial_chain = vec![method_name.clone()];
        let tree = build_caller_tree(
            &method_name,
            class_filter.as_deref(),
            max_depth,
            0,
            &caller_ctx,
            &mut visited,
            &mut file_cache,
            &mut total_body_lines_emitted,
            &mut tests_found,
            &initial_chain,
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
        if let Some(root) = &root_method {
            output["rootMethod"] = root.clone();
        }

        // Impact analysis: add testsCovering section
        if impact_analysis {
            // Dedup by method+class+file
            tests_found.sort_by(|a, b| {
                let key = |v: &Value| format!("{}.{}.{}",
                    v["class"].as_str().unwrap_or(""),
                    v["method"].as_str().unwrap_or(""),
                    v["file"].as_str().unwrap_or(""));
                key(a).cmp(&key(b))
            });
            tests_found.dedup_by(|a, b| {
                a["method"] == b["method"]
                    && a["class"] == b["class"]
                    && a["file"] == b["file"]
            });
            output["testsCovering"] = json!(tests_found);
            summary["testsFound"] = json!(tests_found.len());
            output["summary"] = summary;
            output["query"]["impactAnalysis"] = json!(true);
        }

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
            &mut file_cache,
            &mut total_body_lines_emitted,
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
        if let Some(root) = &root_method {
            output["rootMethod"] = root.clone();
        }
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

// ─── Multi-method batch handler ──────────────────────────────────────

/// Handle multi-method batch: run independent call trees for each method,
/// return results grouped by method. Body budget is SHARED across all methods.
/// Each method gets its own independent `maxTotalNodes` and `visited` set.
fn handle_multi_method_callers(
    ctx: &HandlerContext,
    args: &Value,
    def_index: &std::sync::Arc<std::sync::RwLock<DefinitionIndex>>,
    methods: &[String],
    class_filter: Option<&str>,
) -> ToolCallResult {
    // Parse common parameters (same as single-method)
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
    let ext_filter = args.get("ext").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.server_ext.clone());
    let resolve_interfaces = args.get("resolveInterfaces").and_then(|v| v.as_bool()).unwrap_or(true);
    let max_callers_per_level = args.get("maxCallersPerLevel").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let max_total_nodes = {
        let raw = args.get("maxTotalNodes").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
        if raw == 0 { usize::MAX } else { raw }
    };
    let exclude_dir: Vec<String> = args.get("excludeDir")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect())
        .unwrap_or_default();
    let exclude_file: Vec<String> = args.get("excludeFile")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect())
        .unwrap_or_default();
    let include_doc_comments = args.get("includeDocComments").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_body = args.get("includeBody").and_then(|v| v.as_bool()).unwrap_or(false)
        || include_doc_comments;
    let max_body_lines = args.get("maxBodyLines").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let max_total_body_lines = args.get("maxTotalBodyLines").and_then(|v| v.as_u64()).unwrap_or(300) as usize;
    let impact_analysis = args.get("impactAnalysis").and_then(|v| v.as_bool()).unwrap_or(false);
    if impact_analysis && direction != "up" {
        return ToolCallResult::error(
            "impactAnalysis only works with direction='up'.".to_string()
        );
    }

    let search_start = Instant::now();

    let content_index = match ctx.index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire content index lock: {}", e)),
    };
    let def_idx = match def_index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire definition index lock: {}", e)),
    };

    // Shared mutable state across all methods:
    // - file_cache: reuse file reads across methods (optimization)
    // - total_body_lines_emitted: SHARED body budget (prevents response explosion)
    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total_body_lines_emitted: usize = 0;
    let mut total_nodes_all: usize = 0;

    let mut results: Vec<Value> = Vec::new();

    for method_name in methods {
        // Each method gets its OWN node_count and visited set (per-method budget)
        let node_count = AtomicUsize::new(0);
        let limits = CallerLimits { max_callers_per_level, max_total_nodes };

        let caller_ctx = CallerTreeContext {
            content_index: &content_index,
            def_idx: &def_idx,
            ext_filter: &ext_filter,
            exclude_dir: &exclude_dir,
            exclude_file: &exclude_file,
            resolve_interfaces,
            limits: &limits,
            node_count: &node_count,
            include_body,
            include_doc_comments,
            max_body_lines,
            max_total_body_lines,
            impact_analysis,
        };

        let method_lower = method_name.to_lowercase();

        // Build root method info
        let root_method = if include_body {
            build_root_method_info(
                &method_lower,
                class_filter,
                &def_idx,
                &mut file_cache,
                &mut total_body_lines_emitted,
                max_body_lines,
                max_total_body_lines,
                include_doc_comments,
            )
        } else {
            None
        };

        let mut method_result = json!({ "method": method_name });

        if direction == "up" {
            let mut visited: HashSet<String> = HashSet::new();
            let mut tests_found: Vec<Value> = Vec::new();
            let initial_chain = vec![method_name.clone()];
            let tree = build_caller_tree(
                method_name,
                class_filter,
                max_depth,
                0,
                &caller_ctx,
                &mut visited,
                &mut file_cache,
                &mut total_body_lines_emitted,
                &mut tests_found,
                &initial_chain,
            );
            let tree = dedup_caller_tree(tree);
            let method_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
            total_nodes_all += method_nodes;

            method_result["callTree"] = json!(tree);
            method_result["nodesInTree"] = json!(method_nodes);

            if let Some(root) = &root_method {
                method_result["rootMethod"] = root.clone();
            }

            if impact_analysis && !tests_found.is_empty() {
                // Dedup tests
                tests_found.sort_by(|a, b| {
                    let key = |v: &Value| format!("{}.{}.{}",
                        v["class"].as_str().unwrap_or(""),
                        v["method"].as_str().unwrap_or(""),
                        v["file"].as_str().unwrap_or(""));
                    key(a).cmp(&key(b))
                });
                tests_found.dedup_by(|a, b| {
                    a["method"] == b["method"]
                        && a["class"] == b["class"]
                        && a["file"] == b["file"]
                });
                method_result["testsCovering"] = json!(tests_found);
            }
        } else {
            let tree = build_callee_tree(
                method_name,
                class_filter,
                max_depth,
                0,
                &caller_ctx,
                &mut HashSet::new(),
                &mut file_cache,
                &mut total_body_lines_emitted,
            );
            let method_nodes = node_count.load(std::sync::atomic::Ordering::Relaxed);
            total_nodes_all += method_nodes;

            method_result["callTree"] = json!(tree);
            method_result["nodesInTree"] = json!(method_nodes);

            if let Some(root) = &root_method {
                method_result["rootMethod"] = root.clone();
            }
        }

        results.push(method_result);
    }

    let search_elapsed = search_start.elapsed();
    let mut summary = json!({
        "totalMethods": methods.len(),
        "totalNodes": total_nodes_all,
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
    });
    if include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
    }
    inject_branch_warning(&mut summary, ctx);

    let mut output = json!({
        "results": results,
        "query": {
            "methods": methods,
            "direction": direction,
            "depth": max_depth,
            "maxCallersPerLevel": max_callers_per_level,
            "maxTotalNodes": max_total_nodes,
        },
        "summary": summary,
    });
    if let Some(cls) = class_filter {
        output["query"]["class"] = json!(cls);
    }
    if impact_analysis {
        output["query"]["impactAnalysis"] = json!(true);
    }

    ToolCallResult::success(json_to_string(&output))
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
    include_body: bool,
    include_doc_comments: bool,
    max_body_lines: usize,
    max_total_body_lines: usize,
    impact_analysis: bool,
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
                DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Property | DefinitionKind::Function
                | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction => {}
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
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    tests_found: &mut Vec<Value>,
    call_chain: &[String],
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
                                                                    file_cache,
                                                                    total_body_lines_emitted,
                                                                    tests_found,
                                                                    call_chain,
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

/// Build a JSON object describing the root method (the method being searched for).
/// Includes body if includeBody is true. Returns None if the method is not found in the definition index.
fn build_root_method_info(
    method_lower: &str,
    class_filter: Option<&str>,
    def_idx: &DefinitionIndex,
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    max_body_lines: usize,
    max_total_body_lines: usize,
    include_doc_comments: bool,
) -> Option<Value> {
    let name_indices = def_idx.name_index.get(method_lower)?;

    // Find the best matching method definition
    for &di in name_indices {
        let def = def_idx.definitions.get(di as usize)?;
        if !matches!(def.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
            | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction) {
            continue;
        }
        // Apply class filter if provided
        if let Some(cls) = class_filter {
            if !def.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(cls)) {
                continue;
            }
        }

        let file_path = def_idx.files.get(def.file_id as usize)?;
        let mut node = json!({
            "method": def.name,
            "line": def.line_start,
        });
        if let Some(ref parent) = def.parent {
            node["class"] = json!(parent);
        }
        if let Some(fname) = Path::new(file_path.as_str()).file_name().and_then(|f| f.to_str()) {
            node["file"] = json!(fname);
        }

        inject_body_into_obj(
            &mut node,
            file_path,
            def.line_start,
            def.line_end,
            file_cache,
            total_body_lines_emitted,
            max_body_lines,
            max_total_body_lines,
            include_doc_comments,
        );

        return Some(node);
    }
    None
}

/// Find the `line_start` of the first matching method definition for overload disambiguation.
/// Searches the name index for definitions with callable kinds (Method, Constructor, Function,
/// StoredProcedure, SqlFunction). When `parent_class` is provided, only matches definitions
/// whose parent matches (case-insensitive).
fn find_target_line(
    def_idx: &DefinitionIndex,
    method_lower: &str,
    parent_class: Option<&str>,
) -> Option<u32> {
    def_idx.name_index.get(method_lower)
        .and_then(|indices| indices.iter().find_map(|&di| {
            def_idx.definitions.get(di as usize).and_then(|d| {
                if !matches!(d.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
                    | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction) {
                    return None;
                }
                if let Some(cls) = parent_class {
                    if d.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(cls)) {
                        return Some(d.line_start);
                    }
                    None
                } else {
                    Some(d.line_start)
                }
            })
        }))
}

/// Collect all (file_id, line_start) pairs for method definitions matching `method_lower`.
/// These represent definition sites that should be excluded from caller results
/// (a method's own definition line is not a call site).
fn collect_definition_locations(
    def_idx: &DefinitionIndex,
    method_lower: &str,
) -> HashSet<(u32, u32)> {
    let mut locations: HashSet<(u32, u32)> = HashSet::new();
    if let Some(name_indices) = def_idx.name_index.get(method_lower) {
        for &di in name_indices {
            if let Some(def) = def_idx.definitions.get(di as usize) {
                if matches!(def.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
                    | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction) {
                    locations.insert((def.file_id, def.line_start));
                }
            }
        }
    }
    locations
}

/// Check if a file path matches the extension filter and does not match any exclusion patterns.
/// Returns `true` if the file passes all filters.
fn passes_caller_file_filters(
    file_path: &str,
    ext_filter: &str,
    exclude_dir: &[String],
    exclude_file: &[String],
) -> bool {
    let matches_ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ext_filter.split(',')
                .any(|allowed| e.eq_ignore_ascii_case(allowed.trim()))
        });
    if !matches_ext { return false; }

    let path_lower = file_path.to_lowercase();
    if exclude_dir.iter().any(|excl| path_lower.contains(excl.as_str())) { return false; }
    if exclude_file.iter().any(|excl| path_lower.contains(excl.as_str())) { return false; }

    true
}

/// Build a JSON node for a single caller in the call tree.
/// Includes method name, definition line, call site line(s), optional class, optional file name,
/// and optional sub-callers array.
/// When `call_site_lines` has more than 1 entry, includes `callSites` array with all call lines.
fn build_caller_node(
    caller_name: &str,
    caller_parent: Option<&str>,
    caller_line: u32,
    call_site_lines: &[u32],
    file_path: &str,
    sub_callers: Vec<Value>,
) -> Value {
    let first_call_site = call_site_lines.first().copied().unwrap_or(0);
    let mut node = json!({
        "method": caller_name,
        "line": caller_line,
        "callSite": first_call_site,
    });
    // Include callSites array only when there are multiple call sites (saves tokens)
    if call_site_lines.len() > 1 {
        node["callSites"] = json!(call_site_lines);
    }
    if let Some(parent) = caller_parent {
        node["class"] = json!(parent);
    }
    if let Some(fname) = Path::new(file_path).file_name().and_then(|f| f.to_str()) {
        node["file"] = json!(fname);
    }
    if !sub_callers.is_empty() {
        node["callers"] = json!(sub_callers);
    }
    node
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
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    tests_found: &mut Vec<Value>,
    call_chain: &[String],
) -> Vec<Value> {
    if current_depth >= max_depth {
        return Vec::new();
    }
    if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes {
        return Vec::new();
    }

    let method_lower = method_name.to_lowercase();

    let target_line = find_target_line(ctx.def_idx, &method_lower, parent_class);

    // Use class.method.line as visited key to distinguish overloads
    let visited_key = if let Some(cls) = parent_class {
        format!("{}.{}.{}", cls.to_lowercase(), method_lower, target_line.unwrap_or(0))
    } else {
        format!("{}.{}", method_lower, target_line.unwrap_or(0))
    };
    if !visited.insert(visited_key) {
        return Vec::new();
    }

    let postings = match ctx.content_index.index.get(&method_lower) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let parent_file_ids: Option<HashSet<u32>> = parent_class.and_then(|cls| {
        resolve_parent_file_ids(cls, ctx)
    });

    let definition_locations = collect_definition_locations(ctx.def_idx, &method_lower);

    // Two-phase approach: first collect all call sites per caller, then build nodes.
    // This allows us to gather ALL call site lines for a single caller method.
    struct CallerInfo {
        name: String,
        parent: Option<String>,
        line: u32,
        di: u32,
        file_path: String,
        call_sites: Vec<u32>,
    }

    let mut caller_map: HashMap<String, CallerInfo> = HashMap::new();
    let mut caller_order: Vec<String> = Vec::new(); // preserve insertion order

    for posting in postings {
        if caller_map.len() >= ctx.limits.max_callers_per_level { break; }
        if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }

        // If we have a parent class context, skip files that don't reference that class
        if let Some(ref pids) = parent_file_ids {
            if !pids.contains(&posting.file_id) { continue; }
        }

        let file_path = match ctx.content_index.files.get(posting.file_id as usize) {
            Some(p) => p,
            None => continue,
        };

        if !passes_caller_file_filters(file_path, ctx.ext_filter, ctx.exclude_dir, ctx.exclude_file) {
            continue;
        }

        let def_fid = match ctx.def_idx.path_to_id.get(&std::path::PathBuf::from(file_path)).copied() {
            Some(id) => id,
            None => continue,
        };

        for &line in &posting.lines {
            if caller_map.len() >= ctx.limits.max_callers_per_level && !caller_map.values().any(|_| true) { break; }
            if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }

            if definition_locations.contains(&(def_fid, line)) { continue; }

            let (caller_name, caller_parent, caller_line, caller_di) =
                match find_containing_method(ctx.def_idx, def_fid, line) {
                    Some(v) => v,
                    None => continue,
                };

            // Verify the call on this line actually targets the expected class
            if parent_class.is_some()
                && !verify_call_site_target(ctx.def_idx, caller_di, line, &method_lower, parent_class)
            {
                continue;
            }

            let caller_key = format!("{}.{}.{}",
                caller_parent.as_deref().unwrap_or("?"),
                &caller_name,
                caller_line
            );

            if let Some(existing) = caller_map.get_mut(&caller_key) {
                // Same caller method — just add the call site line
                if !existing.call_sites.contains(&line) {
                    existing.call_sites.push(line);
                }
            } else {
                if caller_map.len() >= ctx.limits.max_callers_per_level { continue; }
                caller_order.push(caller_key.clone());
                caller_map.insert(caller_key, CallerInfo {
                    name: caller_name,
                    parent: caller_parent,
                    line: caller_line,
                    di: caller_di,
                    file_path: file_path.clone(),
                    call_sites: vec![line],
                });
            }
        }
    }

    // Phase 2: Build nodes from collected caller info
    let mut callers: Vec<Value> = Vec::new();

    for caller_key in &caller_order {
        let info = match caller_map.get(caller_key) {
            Some(i) => i,
            None => continue,
        };

        if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }
        ctx.node_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut sorted_call_sites = info.call_sites.clone();
        sorted_call_sites.sort();

        // Impact analysis: check if this caller is a test method
        if ctx.impact_analysis {
            let caller_def = &ctx.def_idx.definitions[info.di as usize];
            if is_test_method(caller_def, &info.file_path) {
                let mut chain = call_chain.to_vec();
                chain.push(info.name.clone());
                let mut test_info = json!({
                    "method": &info.name,
                    "line": info.line,
                    "file": &info.file_path,
                    "depth": current_depth + 1,
                    "callChain": chain,
                });
                if let Some(ref parent) = info.parent {
                    test_info["class"] = json!(parent);
                }
                tests_found.push(test_info);

                let mut node = build_caller_node(
                    &info.name,
                    info.parent.as_deref(),
                    info.line,
                    &sorted_call_sites,
                    &info.file_path,
                    vec![], // no sub-callers — test is a leaf
                );
                node["isTest"] = json!(true);

                if ctx.include_body {
                    let caller_line_end = ctx.def_idx.definitions.get(info.di as usize)
                        .map(|d| d.line_end)
                        .unwrap_or(info.line);
                    inject_body_into_obj(
                        &mut node,
                        &info.file_path,
                        info.line,
                        caller_line_end,
                        file_cache,
                        total_body_lines_emitted,
                        ctx.max_body_lines,
                        ctx.max_total_body_lines,
                        ctx.include_doc_comments,
                    );
                }

                callers.push(node);
                continue;
            }
        }

        // Recurse with the CALLER's parent class as the class filter.
        let mut next_chain = call_chain.to_vec();
        next_chain.push(info.name.clone());
        let sub_callers = build_caller_tree(
            &info.name,
            info.parent.as_deref(),
            max_depth,
            current_depth + 1,
            ctx,
            visited,
            file_cache,
            total_body_lines_emitted,
            tests_found,
            &next_chain,
        );

        let mut node = build_caller_node(
            &info.name,
            info.parent.as_deref(),
            info.line,
            &sorted_call_sites,
            &info.file_path,
            sub_callers,
        );

        // Inject body if requested
        if ctx.include_body {
            let caller_line_end = ctx.def_idx.definitions.get(info.di as usize)
                .map(|d| d.line_end)
                .unwrap_or(info.line);
            inject_body_into_obj(
                &mut node,
                &info.file_path,
                info.line,
                caller_line_end,
                file_cache,
                total_body_lines_emitted,
                ctx.max_body_lines,
                ctx.max_total_body_lines,
                ctx.include_doc_comments,
            );
        }

        callers.push(node);
    }

    // Interface resolution: expand to find callers via interface implementations
    if ctx.resolve_interfaces && current_depth == 0 {
        let iface_callers = expand_interface_callers(
            method_name, &method_lower, parent_class, max_depth, ctx, visited,
            file_cache, total_body_lines_emitted, tests_found, call_chain,
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
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
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

    let target_line = find_target_line(def_idx, &method_lower, class_filter);

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
                            let kind_ok = d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Constructor || d.kind == DefinitionKind::Function
                                || d.kind == DefinitionKind::StoredProcedure || d.kind == DefinitionKind::SqlFunction;
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

                if !passes_caller_file_filters(callee_file, ext_filter, exclude_dir, exclude_file) {
                    continue;
                }

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
                    file_cache,
                    total_body_lines_emitted,
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

                // Inject body if requested
                if ctx.include_body {
                    inject_body_into_obj(
                        &mut node,
                        callee_file,
                        callee_def.line_start,
                        callee_def.line_end,
                        file_cache,
                        total_body_lines_emitted,
                        ctx.max_body_lines,
                        ctx.max_total_body_lines,
                        ctx.include_doc_comments,
                    );
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

        // Only match methods, constructors, functions, stored procedures, and SQL functions
        if def.kind != DefinitionKind::Method && def.kind != DefinitionKind::Constructor && def.kind != DefinitionKind::Function
            && def.kind != DefinitionKind::StoredProcedure && def.kind != DefinitionKind::SqlFunction {
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
#[path = "callers_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "callers_tests_additional.rs"]
mod callers_additional_tests;
