//! xray_callers handler: call tree building (up/down).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::ContentIndex;
use crate::definitions::{CallSite, DefinitionEntry, DefinitionIndex, DefinitionKind};
use code_xray::generate_trigrams;

use super::HandlerContext;
#[allow(unused_imports)] // `self` needed by test submodules for utils::ExcludePatterns
use super::utils::{self, inject_body_into_obj, inject_branch_warning, json_to_string, name_similarity, sorted_intersect};

/// Compute total body lines available from a call tree (for size hint).
/// Walks the JSON tree and sums body lines: `body.len()` for emitted bodies,
/// `totalBodyLines` for truncated bodies. Does NOT count `bodyOmitted` nodes
/// (those were skipped due to budget, but we know they had lines available).
fn compute_body_lines_from_tree(tree: &[Value], root_method: Option<&Value>) -> (usize, usize) {
    let mut emitted: usize = 0;
    let mut available: usize = 0;

    fn walk(node: &Value, emitted: &mut usize, available: &mut usize) {
        if let Some(body) = node.get("body").and_then(|v| v.as_array()) {
            let len = body.len();
            *emitted += len;
            if let Some(total) = node.get("totalBodyLines").and_then(|v| v.as_u64()) {
                *available += total as usize; // truncated: available = totalBodyLines
            } else {
                *available += len; // not truncated: available = emitted
            }
        } else if node.get("bodyOmitted").is_some() {
            // Body was skipped due to budget — we don't know exact lines, skip
            // (inject_body_into_obj doesn't record the original line count for omitted nodes)
        }
        // Recurse into sub-callers/callees
        for key in &["callers", "callees", "children"] {
            if let Some(children) = node.get(key).and_then(|v| v.as_array()) {
                for child in children {
                    walk(child, emitted, available);
                }
            }
        }
    }

    for node in tree {
        walk(node, &mut emitted, &mut available);
    }
    // Include rootMethod body
    if let Some(root) = root_method {
        walk(root, &mut emitted, &mut available);
    }
    (emitted, available)
}

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

/// Check if a file path indicates a test file.
/// Covers: Rust `_tests.rs`, TypeScript/JavaScript `.spec.ts`/`.test.ts`,
/// and directory conventions (`/tests/`, `/test/`).
fn is_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("_tests.")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("/tests/")
        || lower.contains("\\tests\\")
        || lower.contains("/test/")
        || lower.contains("\\test\\")
}

/// Popularity proxy: total posting line count for a method name across all files.
/// O(1) HashMap lookup. Used as a secondary sort key to prioritize more-referenced
/// callers within the same group (test vs non-test).
fn caller_popularity(content_index: &ContentIndex, method_name: &str) -> usize {
    content_index.index
        .get(&method_name.to_lowercase())
        .map(|postings| postings.iter().map(|p| p.lines.len()).sum())
        .unwrap_or(0)
}

/// Check if a caller (identified by its CallerInfo) is a test method or resides in a test file.
fn is_test_caller(def_idx: &DefinitionIndex, di: u32, file_path: &str) -> bool {
    if is_test_file(file_path) {
        return true;
    }
    if let Some(def) = def_idx.definitions.get(di as usize) {
        return is_test_method(def, file_path);
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

pub(crate) fn handle_xray_callers(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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
    let body_line_start = args.get("bodyLineStart").and_then(|v| v.as_u64()).map(|v| v as u32);
    let body_line_end = args.get("bodyLineEnd").and_then(|v| v.as_u64()).map(|v| v as u32);

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
    let ambiguity_warning = check_method_ambiguity(&method_name, &method_lower, class_filter.as_deref(), &def_idx);

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

    let exclude_patterns = super::utils::ExcludePatterns::from_dirs(&exclude_dir);
    let exclude_file_lower: Vec<String> = exclude_file.iter().map(|s| s.to_lowercase()).collect();
    let ext_filter_list = super::utils::prepare_ext_filter(&ext_filter);
    let caller_ctx = CallerTreeContext {
        content_index: &content_index,
        def_idx: &def_idx,
        ext_filter: &ext_filter,
        resolve_interfaces,
        limits: &limits,
        node_count: &node_count,
        include_body,
        include_doc_comments,
        max_body_lines,
        max_total_body_lines,
        impact_analysis,
        exclude_patterns,
        exclude_file_lower,
        ext_filter_list,
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
            body_line_start,
            body_line_end,
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
        if include_body {
            summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
            let (_, tree_available) = compute_body_lines_from_tree(&tree, root_method.as_ref());
            if total_body_lines_emitted < tree_available {
                summary["totalBodyLinesAvailable"] = json!(tree_available);
            }
        }
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

        // Nearest-match hints when callTree is empty
        if tree.is_empty() {
            let hint = generate_callers_hint(&method_name, class_filter.as_deref(), &def_idx);
            if let Some(h) = hint {
                output["hint"] = json!(h);
            }
        }

        // Cross-index enrichment: grep references not in call tree
        let include_grep_refs = args.get("includeGrepReferences").and_then(|v| v.as_bool()).unwrap_or(false);
        if include_grep_refs && method_name.len() >= 4 {
            let tree_files = collect_files_from_tree(&tree);
            let grep_refs = build_grep_references(&method_name, &content_index, &tree_files, &def_idx);
            if !grep_refs.is_empty() {
                output["grepReferences"] = json!(grep_refs);
                output["grepReferencesNote"] = json!(
                    "Text references not captured by AST call analysis. May include delegate usage, method groups, reflection, or comments."
                );
            }
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
        if include_body {
            summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
            let (_, tree_available) = compute_body_lines_from_tree(&tree, root_method.as_ref());
            if total_body_lines_emitted < tree_available {
                summary["totalBodyLinesAvailable"] = json!(tree_available);
            }
        }
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
        // Nearest-match hints when callTree is empty
        if tree.is_empty() {
            let hint = generate_callers_hint(&method_name, class_filter.as_deref(), &def_idx);
            if let Some(h) = hint {
                output["hint"] = json!(h);
            }
        }

        // Cross-index enrichment: grep references not in call tree
        let include_grep_refs = args.get("includeGrepReferences").and_then(|v| v.as_bool()).unwrap_or(false);
        if include_grep_refs && method_name.len() >= 4 {
            let tree_files = collect_files_from_tree(&tree);
            let grep_refs = build_grep_references(&method_name, &content_index, &tree_files, &def_idx);
            if !grep_refs.is_empty() {
                output["grepReferences"] = json!(grep_refs);
                output["grepReferencesNote"] = json!(
                    "Text references not captured by AST call analysis. May include delegate usage, method groups, reflection, or comments."
                );
            }
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
    let body_line_start = args.get("bodyLineStart").and_then(|v| v.as_u64()).map(|v| v as u32);
    let body_line_end = args.get("bodyLineEnd").and_then(|v| v.as_u64()).map(|v| v as u32);
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

    // Pre-compute filter patterns once for all methods
    let exclude_patterns = super::utils::ExcludePatterns::from_dirs(&exclude_dir);
    let exclude_file_lower: Vec<String> = exclude_file.iter().map(|s| s.to_lowercase()).collect();
    let ext_filter_list = super::utils::prepare_ext_filter(&ext_filter);

    for method_name in methods {
        // Each method gets its OWN node_count and visited set (per-method budget)
        let node_count = AtomicUsize::new(0);
        let limits = CallerLimits { max_callers_per_level, max_total_nodes };

        let caller_ctx = CallerTreeContext {
            content_index: &content_index,
            def_idx: &def_idx,
            ext_filter: &ext_filter,
            resolve_interfaces,
            limits: &limits,
            node_count: &node_count,
            include_body,
            include_doc_comments,
            max_body_lines,
            max_total_body_lines,
            impact_analysis,
            exclude_patterns: exclude_patterns.clone(),
            exclude_file_lower: exclude_file_lower.clone(),
            ext_filter_list: ext_filter_list.clone(),
        };

        let method_lower = method_name.to_lowercase();

        // Per-method ambiguity check (same logic as single-method path)
        let method_warning = check_method_ambiguity(method_name, &method_lower, class_filter, &def_idx);

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
                body_line_start,
                body_line_end,
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
            method_result["nodesVisited"] = json!(visited.len());
            let method_truncated = method_nodes >= max_total_nodes;
            method_result["truncated"] = json!(method_truncated);

            if let Some(root) = &root_method {
                method_result["rootMethod"] = root.clone();
            }

            // Nearest-match hint when callTree is empty
            if tree.is_empty()
                && let Some(h) = generate_callers_hint(method_name, class_filter, &def_idx) {
                    method_result["hint"] = json!(h);
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
            let method_truncated = method_nodes >= max_total_nodes;
            method_result["truncated"] = json!(method_truncated);

            if let Some(root) = &root_method {
                method_result["rootMethod"] = root.clone();
            }

            // Nearest-match hint when callTree is empty
            if tree.is_empty()
                && let Some(h) = generate_callers_hint(method_name, class_filter, &def_idx) {
                    method_result["hint"] = json!(h);
                }
        }

        // Add per-method ambiguity warning
        if let Some(ref warning) = method_warning {
            method_result["warning"] = json!(warning);
        }

        results.push(method_result);
    }

    let search_elapsed = search_start.elapsed();
    let mut summary = json!({
        "totalMethods": methods.len(),
        "totalNodes": total_nodes_all,
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
    });
    // Any method truncated = overall truncated
    let any_truncated = results.iter().any(|r| r["truncated"].as_bool().unwrap_or(false));
    if any_truncated {
        summary["truncated"] = json!(true);
    }
    if include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
        // Compute available from all result trees
        let mut total_available: usize = 0;
        for result in &results {
            let tree = result["callTree"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            let root = result.get("rootMethod");
            let (_, avail) = compute_body_lines_from_tree(tree, root);
            total_available += avail;
        }
        if total_body_lines_emitted < total_available {
            summary["totalBodyLinesAvailable"] = json!(total_available);
        }
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

// ─── Nearest-match hints for callers ─────────────────────────────────

/// Check if method name is ambiguous (exists in multiple classes) and return warning.
/// Returns None if class_filter is set or method exists in 0-1 classes.
fn check_method_ambiguity(
    method_name: &str,
    method_lower: &str,
    class_filter: Option<&str>,
    def_idx: &DefinitionIndex,
) -> Option<String> {
    if class_filter.is_some() {
        return None;
    }
    let name_indices = def_idx.name_index.get(method_lower)?;
    let method_defs: Vec<&DefinitionEntry> = name_indices.iter()
        .filter_map(|&di| def_idx.definitions.get(di as usize))
        .filter(|d| d.kind == DefinitionKind::Method || d.kind == DefinitionKind::Constructor || d.kind == DefinitionKind::Function
            || d.kind == DefinitionKind::StoredProcedure || d.kind == DefinitionKind::SqlFunction)
        .collect();

    let unique_classes: HashSet<&str> = method_defs.iter()
        .filter_map(|d| d.parent.as_deref())
        .collect();

    if unique_classes.len() <= 1 {
        return None;
    }

    let total = unique_classes.len();
    let mut class_list: Vec<&str> = unique_classes.into_iter().collect();
    class_list.sort_unstable();
    const MAX_LISTED: usize = 10;
    if total <= MAX_LISTED {
        Some(format!(
            "Method '{}' found in {} classes: {}. Results may mix callers from different classes. Use 'class' parameter to scope the search.",
            method_name, total, class_list.join(", ")
        ))
    } else {
        let shown: Vec<&str> = class_list.into_iter().take(MAX_LISTED).collect();
        Some(format!(
            "Method '{}' found in {} classes (showing first {}): {}… Use 'class' parameter to scope the search.",
            method_name, total, MAX_LISTED, shown.join(", ")
        ))
    }
}


/// Generate a nearest-match hint when callTree is empty.
/// Checks both method name and class name for typos using Jaro-Winkler similarity.
/// Falls back to a generic hint if both names exist but no call sites were found.
fn generate_callers_hint(
    method_name: &str,
    class_filter: Option<&str>,
    def_idx: &DefinitionIndex,
) -> Option<String> {
    let method_lower = method_name.to_lowercase();
    let mut hint_parts: Vec<String> = Vec::new();

    // A. Nearest method name — check if method exists in index
    let method_exists = def_idx.name_index.get(&method_lower)
        .map(|indices| indices.iter().any(|&i| {
            def_idx.definitions.get(i as usize)
                .is_some_and(|d| matches!(d.kind,
                    DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
                    | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction))
        }))
        .unwrap_or(false);

    if !method_exists {
        let mut best_score = 0.0f64;
        let mut best_name = String::new();
        for (name, indices) in &def_idx.name_index {
            // Only consider callable definitions
            let has_callable = indices.iter().any(|&i| {
                def_idx.definitions.get(i as usize)
                    .is_some_and(|d| matches!(d.kind,
                        DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
                        | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction))
            });
            if !has_callable { continue; }

            let score = name_similarity(&method_lower, name);
            if score > best_score && score > 0.75 {
                best_score = score;
                best_name = name.clone();
            }
        }
        if !best_name.is_empty() {
            hint_parts.push(format!(
                "Method '{}' not found. Nearest match: '{}' (similarity {:.0}%)",
                method_name, best_name, best_score * 100.0
            ));
        }
    }

    // B. Nearest class name (if class filter is set)
    if let Some(cls) = class_filter {
        let cls_lower = cls.to_lowercase();
        // Pre-filter: collect class/interface/struct names from kind_index
        let class_exists = def_idx.name_index.get(&cls_lower)
            .map(|indices| indices.iter().any(|&i| {
                def_idx.definitions.get(i as usize)
                    .is_some_and(|d| matches!(d.kind,
                        DefinitionKind::Class | DefinitionKind::Interface | DefinitionKind::Struct | DefinitionKind::Record))
            }))
            .unwrap_or(false);

        if !class_exists {
            let mut best_score = 0.0f64;
            let mut best_class = String::new();
            for (name, indices) in &def_idx.name_index {
                let has_type = indices.iter().any(|&i| {
                    def_idx.definitions.get(i as usize)
                        .is_some_and(|d| matches!(d.kind,
                            DefinitionKind::Class | DefinitionKind::Interface | DefinitionKind::Struct | DefinitionKind::Record))
                });
                if !has_type { continue; }

                let score = name_similarity(&cls_lower, name);
                if score > best_score && score > 0.75 {
                    best_score = score;
                    best_class = name.clone();
                }
            }
            if !best_class.is_empty() {
                hint_parts.push(format!(
                    "Class '{}' not found. Nearest: '{}' (similarity {:.0}%)",
                    cls, best_class, best_score * 100.0
                ));
            }
        }
    }

    // C. Fallback generic hint — method and class both exist but no callers found
    if hint_parts.is_empty() && class_filter.is_some() {
        hint_parts.push(
            "No callers found. Possible reasons: (1) calls go through extension methods or DI wrappers, (2) class filter is too narrow. Try without 'class' parameter or with the interface name.".to_string()
        );
    }

    if hint_parts.is_empty() {
        None
    } else {
        Some(hint_parts.join(". "))
    }
}

// ─── Cross-index grep references ─────────────────────────────────────

/// Collect all file paths from a call tree (recursive).
/// Used to exclude files already in the call tree from grep references.
fn collect_files_from_tree(tree: &[Value]) -> HashSet<String> {
    let mut files = HashSet::new();
    for node in tree {
        if let Some(f) = node.get("file").and_then(|v| v.as_str()) {
            files.insert(f.to_string());
        }
        // Recurse into sub-callers (direction=up) and sub-callees (direction=down)
        for key in &["callers", "callees", "children"] {
            if let Some(children) = node.get(key).and_then(|v| v.as_array()) {
                files.extend(collect_files_from_tree(children));
            }
        }
    }
    files
}

/// Build grep references: files containing the method name as text
/// but NOT in the call tree. Uses the content index for O(1) lookup.
fn build_grep_references(
    method_name: &str,
    content_index: &ContentIndex,
    tree_files: &HashSet<String>,
    def_index: &DefinitionIndex,
) -> Vec<Value> {
    let method_lower = method_name.to_lowercase();
    let postings = match content_index.index.get(&method_lower) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // A3 fix: Collect files where the method is DEFINED to exclude them.
    // The definition file contains the method name in its declaration, which would
    // show up as a false positive in grep references (it's already the root method).
    let definition_file_names: HashSet<String> = if let Some(indices) = def_index.name_index.get(&method_lower) {
        indices.iter()
            .filter_map(|&idx| {
                let def = def_index.definitions.get(idx as usize)?;
                let file_path = def_index.files.get(def.file_id as usize)?;
                Path::new(file_path).file_name()
                    .and_then(|f| f.to_str())
                    .map(|s| s.to_string())
            })
            .collect()
    } else {
        HashSet::new()
    };

    let mut grep_refs: Vec<Value> = Vec::new();
    for posting in postings {
        if let Some(file) = content_index.files.get(posting.file_id as usize) {
            // Compare by filename only (call tree stores filenames, not full paths).
            // Known limitation (A4): if two files in different directories share the
            // same filename and one is in the call tree, the other is also excluded.
            // This is a rare edge case; a proper fix would require storing full paths
            // in call tree nodes.
            let fname = Path::new(file).file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(file);
            if !tree_files.contains(fname) && !definition_file_names.contains(fname) {
                grep_refs.push(json!({
                    "file": file,
                    "tokenCount": posting.lines.len(),
                }));
            }
        }
    }
    grep_refs
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
    resolve_interfaces: bool,
    limits: &'a CallerLimits,
    node_count: &'a AtomicUsize,
    include_body: bool,
    include_doc_comments: bool,
    max_body_lines: usize,
    max_total_body_lines: usize,
    impact_analysis: bool,
    /// Pre-computed exclude dir patterns (avoids per-file allocations)
    exclude_patterns: super::utils::ExcludePatterns,
    /// Pre-lowercased exclude file substrings
    exclude_file_lower: Vec<String>,
    /// Pre-split extension filter list
    ext_filter_list: Vec<String>,
}

#[cfg(test)]
impl CallerTreeContext<'_> {
    /// Test-only default with "cs" ext filter and empty excludes.
    /// Override specific fields with struct update syntax:
    /// `CallerTreeContext { ext_filter: "ts", ..CallerTreeContext::test_default(&ci, &di, &l, &nc) }`
    fn test_default<'a>(
        content_index: &'a ContentIndex,
        def_idx: &'a DefinitionIndex,
        limits: &'a CallerLimits,
        node_count: &'a AtomicUsize,
    ) -> CallerTreeContext<'a> {
        CallerTreeContext {
            content_index,
            def_idx,
            ext_filter: "cs",
            resolve_interfaces: true,
            limits,
            node_count,
            include_body: false,
            include_doc_comments: false,
            max_body_lines: 0,
            max_total_body_lines: 0,
            impact_analysis: false,
            exclude_patterns: super::utils::ExcludePatterns::from_dirs(&[]),
            exclude_file_lower: vec![],
            ext_filter_list: super::utils::prepare_ext_filter("cs"),
        }
    }
}

impl CallerTreeContext<'_> {
    /// Optimized file filter using pre-computed patterns.
    fn passes_file_filters(&self, file_path: &str) -> bool {
        let matches_ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.ext_filter_list.iter().any(|allowed| e.eq_ignore_ascii_case(allowed)));
        if !matches_ext { return false; }

        // Use pre-computed patterns (zero per-file allocations for the patterns)
        if !self.exclude_patterns.is_empty() {
            let path_lower = file_path.to_lowercase().replace('\\', "/");
            if self.exclude_patterns.matches(&path_lower) { return false; }
        }

        if !self.exclude_file_lower.is_empty() {
            let path_lower = file_path.to_lowercase();
            if self.exclude_file_lower.iter().any(|excl| path_lower.contains(excl.as_str())) {
                return false;
            }
        }

        true
    }
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
#[allow(clippy::too_many_arguments)]
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
            // A1 fix: Only expand interface callers for method-like definitions.
            // Without this filter, properties/fields/enum members with the same name
            // as the target method would be included, creating false callers.
            && matches!(def.kind, DefinitionKind::Method | DefinitionKind::Constructor
                | DefinitionKind::Function | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction)
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
                                                    exclude_patterns: ctx.exclude_patterns.clone(),
                                                    exclude_file_lower: ctx.exclude_file_lower.clone(),
                                                    ext_filter_list: ctx.ext_filter_list.clone(),
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
#[allow(clippy::too_many_arguments)]
fn build_root_method_info(
    method_lower: &str,
    class_filter: Option<&str>,
    def_idx: &DefinitionIndex,
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    max_body_lines: usize,
    max_total_body_lines: usize,
    include_doc_comments: bool,
    body_line_start: Option<u32>,
    body_line_end: Option<u32>,
) -> Option<Value> {
    let name_indices = def_idx.name_index.get(method_lower)?;

    // Find the best matching method definition
    for &di in name_indices {
        let def = match def_idx.definitions.get(di as usize) {
            Some(d) => d,
            None => continue, // tombstone or invalid index — skip
        };
        if !matches!(def.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
            | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction) {
            continue;
        }
        // Apply class filter if provided
        if let Some(cls) = class_filter
            && !def.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(cls)) {
                continue;
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
            body_line_start, body_line_end,
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
            if let Some(def) = def_idx.definitions.get(di as usize)
                && matches!(def.kind, DefinitionKind::Method | DefinitionKind::Constructor | DefinitionKind::Function
                    | DefinitionKind::StoredProcedure | DefinitionKind::SqlFunction) {
                    locations.insert((def.file_id, def.line_start));
                }
        }
    }
    locations
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
#[allow(clippy::too_many_arguments)]
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

    // Safety cap: collect more callers than needed so we can sort test vs non-test
    // before truncating. This avoids scanning ALL postings for popular tokens.
    let collection_limit = if ctx.impact_analysis {
        usize::MAX  // don't cap when searching for tests
    } else {
        ctx.limits.max_callers_per_level * 3
    };

    for posting in postings {
        if caller_map.len() >= collection_limit { break; }
        if ctx.node_count.load(std::sync::atomic::Ordering::Relaxed) >= ctx.limits.max_total_nodes { break; }

        // If we have a parent class context, skip files that don't reference that class
        if let Some(ref pids) = parent_file_ids
            && !pids.contains(&posting.file_id) { continue; }

        let file_path = match ctx.content_index.files.get(posting.file_id as usize) {
            Some(p) => p,
            None => continue,
        };

        if !ctx.passes_file_filters(file_path) {
            continue;
        }

        let def_fid = match ctx.def_idx.path_to_id.get(&std::path::PathBuf::from(file_path)).copied() {
            Some(id) => id,
            None => continue,
        };

        for &line in &posting.lines {
            if caller_map.len() >= collection_limit { break; }
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
                if caller_map.len() >= collection_limit { continue; }
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

    // Phase 1.5: Sort callers — non-test first (primary), popularity DESC (secondary)
    // Then truncate to max_callers_per_level (with impactAnalysis exception for tests)
    // C2 fix: Precache popularity scores to avoid O(n log n) string allocations in sort closure
    let popularity_cache: HashMap<String, usize> = caller_order.iter()
        .map(|key| {
            let info = &caller_map[key];
            (key.clone(), caller_popularity(ctx.content_index, &info.name))
        })
        .collect();
    caller_order.sort_by(|a, b| {
        let info_a = &caller_map[a];
        let info_b = &caller_map[b];
        let is_test_a = is_test_caller(ctx.def_idx, info_a.di, &info_a.file_path);
        let is_test_b = is_test_caller(ctx.def_idx, info_b.di, &info_b.file_path);

        // Primary: non-test (false=0) before test (true=1)
        is_test_a.cmp(&is_test_b)
            // Secondary: more popular callers first (DESC)
            .then_with(|| {
                let pop_a = popularity_cache.get(a).copied().unwrap_or(0);
                let pop_b = popularity_cache.get(b).copied().unwrap_or(0);
                pop_b.cmp(&pop_a)
            })
    });

    // Truncate: apply maxCallersPerLevel limit
    if ctx.impact_analysis {
        // When impactAnalysis is enabled, don't truncate test callers —
        // they're needed for the testsCovering array.
        // Keep: up to max_callers_per_level non-test callers + ALL test callers.
        let non_test_end = caller_order.iter()
            .position(|k| is_test_caller(ctx.def_idx, caller_map[k].di, &caller_map[k].file_path))
            .unwrap_or(caller_order.len());
        if non_test_end > ctx.limits.max_callers_per_level {
            // Too many non-test callers: truncate non-test portion, keep all tests
            let tests: Vec<String> = caller_order.split_off(ctx.limits.max_callers_per_level);
            // tests now contains: remaining non-test (if any) + all test callers
            // We need to keep only test callers from `tests`
            let test_keys: Vec<String> = tests.into_iter()
                .filter(|k| is_test_caller(ctx.def_idx, caller_map[k].di, &caller_map[k].file_path))
                .collect();
            caller_order.extend(test_keys);
        }
        // If non_test_end <= max_callers_per_level, all non-test fit + all tests stay
    } else {
        caller_order.truncate(ctx.limits.max_callers_per_level);
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
                        None, None,
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
                None, None,
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
#[allow(clippy::too_many_arguments)]
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
    let _ext_filter = ctx.ext_filter;
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

                if !ctx.passes_file_filters(callee_file) {
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
                        None, None,
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
    let second_char = match interface_name.strip_prefix('I').and_then(|s| s.chars().next()) {
        Some(c) => c,
        None => return false,
    };
    if !second_char.is_uppercase() {
        return false;
    }

    let stem = &interface_name[1..]; // strip "I" prefix

    // Exact match (existing convention)
    if class_name.eq_ignore_ascii_case(stem) {
        return true;
    }

    // Stem too short → skip fuzzy to avoid false positives
    if stem.len() < 5 {
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
