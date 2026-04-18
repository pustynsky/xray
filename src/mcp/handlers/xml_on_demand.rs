//! XML on-demand handler module.
//!
//! Extracted from `handlers::definitions` to keep that file focused on the
//! index-based code-search path. The XML on-demand feature is architecturally
//! orthogonal: it doesn't touch the DefinitionIndex, it parses a single file
//! per call, and it has its own diagnostic surface (`parse_warnings`). Keeping
//! it side-by-side made `definitions.rs` a 2000+ line god-module.
//!
//! ## Entry point
//!
//! [`try_intercept`] is called from `handle_xray_definitions` before any
//! index-based logic. It returns `Some(ToolCallResult)` iff the query targets
//! an XML file (by extension) AND the request is something the on-demand
//! parser can answer (`containsLine` or `name` filter). Otherwise it returns
//! `None` and the index-based path continues.
//!
//! ## Sandbox and security
//!
//! - [`resolve_xml_file_path`] canonicalizes every input path and rejects any
//!   target outside the workspace (MAJOR-2 sandbox in the code review).
//! - On Windows the UNC prefix `\\?\` added by `canonicalize()` is stripped
//!   before the path is surfaced in JSON/errors (UX regression from canonicalize).
//!
//! ## Feature gate
//!
//! The whole module lives behind `#[cfg(feature = "lang-xml")]`. Other handler
//! modules never depend on it at the type level — they only see the feature
//! flag at the call site in `handle_xray_definitions`.

// cfg(feature = "lang-xml") is set at the module level in mod.rs

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{json, Value};

use crate::definitions::parser_xml::{
    self, is_xml_extension, parse_xml_on_demand_with_warnings, XmlDefinition, XmlParseError,
};
use crate::mcp::handlers::utils::json_to_string;
use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::definitions::DefinitionSearchArgs;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Try to answer the request via the XML on-demand parser.
///
/// Returns `Some(result)` if the request was intercepted (hit or user-facing
/// error), `None` if the request is not XML-related and index-based logic
/// should take over.
pub(crate) fn try_intercept(
    args: &DefinitionSearchArgs,
    search_start: Instant,
    ctx: &HandlerContext,
) -> Option<ToolCallResult> {
    // Only activate if file filter is set with XML extension
    let file_filter = args.file_filter.as_ref()?;
    let ext = extract_file_extension(file_filter)?;
    if !is_xml_extension(&ext) {
        return None;
    }

    // Only handle containsLine or name filter queries — other shapes fall
    // through to the index-based path (which will usually return "no such
    // extension indexed" with a hint).
    if args.contains_line.is_none() && args.name_filter.is_none() {
        return None;
    }

    // Resolve file path (sandboxed to workspace, MAJOR-1/MAJOR-2 in review)
    let file_path = match resolve_xml_file_path(file_filter, ctx) {
        Ok(p) => p,
        Err(diag) => return Some(ToolCallResult::error(diag)),
    };

    // Directories are explicitly rejected — on-demand parsing needs a single
    // file. The error message points the user at the right discovery tool.
    if std::path::Path::new(&file_path).is_dir() {
        return Some(ToolCallResult::error(format!(
            "XML on-demand requires a file path, not a directory: '{}'. \
             Use xray_fast pattern='*.xml' dir='{}' to find specific XML files, \
             then pass the full file path to xray_definitions.",
            file_filter, file_filter
        )));
    }

    // Blocking file read. `handle_xray_definitions` is called from a sync
    // dispatch (see `src/mcp/server.rs`), so no `spawn_blocking` is needed.
    // If that ever changes, this is the place to switch to `tokio::fs`.
    let source = match std::fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(e) => {
            return Some(ToolCallResult::error(format!(
                "Failed to read XML file '{}': {}. Hint: check file path and permissions.",
                file_path, e
            )));
        }
    };

    // Typed error discrimination (MAJOR-3 in review).
    let parse_result = match parse_xml_on_demand_with_warnings(&source, &file_path) {
        Ok(r) => r,
        Err(e @ XmlParseError::GrammarLoad(_)) => {
            return Some(ToolCallResult::error(format!(
                "Internal error: {}. \
                 This indicates a bug in xray's XML grammar integration — \
                 please report it. File attempted: '{}'.",
                e, file_path
            )));
        }
        Err(e @ XmlParseError::TreeSitterReturnedNone) => {
            return Some(ToolCallResult::error(format!(
                "Failed to parse XML file '{}': {}. Hint: check if the file is valid XML.",
                file_path, e
            )));
        }
    };

    let xml_defs = parse_result.definitions;
    let warnings = parse_result.warnings;

    if xml_defs.is_empty() {
        return Some(ToolCallResult::success(json_to_string(&json!({
            "definitions": [],
            "summary": {
                "totalResults": 0,
                "xmlOnDemand": true,
                "hint": "XML file parsed but no elements found.",
                "parseWarnings": warnings,
            }
        }))));
    }

    if let Some(line_num) = args.contains_line {
        Some(handle_contains_line(
            &xml_defs,
            &source,
            &file_path,
            line_num,
            args,
            search_start,
            &warnings,
        ))
    } else { args.name_filter.as_ref().map(|name| handle_name_filter(
            &xml_defs,
            &source,
            &file_path,
            name,
            args,
            search_start,
            &warnings,
        )) }
}

// ---------------------------------------------------------------------------
// Path resolution (sandboxed)
// ---------------------------------------------------------------------------

/// Resolve a user-supplied file filter to an absolute, existing, workspace-
/// sandboxed path.
///
/// Contract:
/// - Input may be absolute or relative (relative = resolved against `ctx.server_dir`).
/// - Result must be inside the workspace; symlink escapes are caught via
///   `canonicalize()` + `starts_with` on the canonical workspace root.
/// - Non-existent paths are rejected (canonicalize requires existence).
/// - On Windows the UNC prefix `\\?\` is stripped in the *returned* string so
///   it does not leak into user-visible JSON. The sandbox check uses the UNC
///   form, so the stripping does not weaken security.
pub(crate) fn resolve_xml_file_path(
    file_filter: &str,
    ctx: &HandlerContext,
) -> Result<String, String> {
    let server_dir = ctx.server_dir();
    let server_canonical = std::path::Path::new(&server_dir)
        .canonicalize()
        .map_err(|e| {
            format!(
                "Failed to canonicalize server directory '{}': {}. \
                 Hint: ensure the workspace is resolved; try xray_reindex.",
                server_dir, e
            )
        })?;

    let raw_path = std::path::Path::new(file_filter);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        std::path::Path::new(&server_dir).join(file_filter)
    };

    // canonicalize() requires the path to exist — this also validates existence.
    let canonical = candidate.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve XML file '{}': {}. \
             Hint: pass a path relative to the workspace ('{}') or an absolute path \
             inside it. Use xray_fast pattern='*.xml' to discover XML files.",
            file_filter, e, server_dir
        )
    })?;

    // Sandbox check: the resolved path must be inside the workspace.
    if !canonical.starts_with(&server_canonical) {
        return Err(format!(
            "XML file '{}' is outside the workspace ('{}'). \
             Absolute paths outside the workspace are not allowed.",
            canonical.display(),
            server_canonical.display()
        ));
    }

    // UX: strip Windows UNC prefix (`\\?\C:\...`) that canonicalize() adds on Windows.
    // The sandbox check above is performed against the UNC form for safety; stripping
    // only affects the returned string surfaced to the user in JSON responses and errors.
    let canonical_str = canonical.to_string_lossy().to_string();
    #[cfg(windows)]
    let canonical_str = canonical_str
        .strip_prefix(r"\\?\")
        .map(String::from)
        .unwrap_or(canonical_str);

    Ok(canonical_str)
}

/// Extract the last `.ext` from a (possibly comma-separated) file filter.
///
/// For `file='Service,web.config'` we look at the **last** term because
/// comma-separated filters are OR-lists and the XML extension is the discriminator
/// that activates on-demand parsing. Returning the last extension matches what
/// users intuitively mean when they mix a substring filter with a concrete file.
pub(crate) fn extract_file_extension(file_filter: &str) -> Option<String> {
    // Take the last term in comma-separated filter (most specific)
    let last_term = file_filter.split(',').next_back()?.trim();
    let dot_pos = last_term.rfind('.')?;
    let ext = &last_term[dot_pos + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_lowercase())
    }
}

// ---------------------------------------------------------------------------
// containsLine path
// ---------------------------------------------------------------------------

fn handle_contains_line(
    xml_defs: &[XmlDefinition],
    source: &str,
    file_path: &str,
    line_num: u32,
    args: &DefinitionSearchArgs,
    search_start: Instant,
    warnings: &[String],
) -> ToolCallResult {
    // Find all elements containing this line, sorted by range (smallest first)
    let mut containing: Vec<(usize, &XmlDefinition)> = xml_defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.entry.line_start <= line_num && d.entry.line_end >= line_num)
        .collect();
    containing.sort_by_key(|(_, d)| d.entry.line_end - d.entry.line_start);

    if containing.is_empty() {
        return ToolCallResult::success(json_to_string(&json!({
            "definitions": [],
            "query": { "file": file_path, "line": line_num },
            "summary": {
                "totalResults": 0,
                "xmlOnDemand": true,
                "hint": format!("Line {} is not inside any XML element in '{}'.", line_num, file_path),
                "parseWarnings": warnings,
            }
        })));
    }

    let innermost = containing[0].1;

    // === PARENT PROMOTION RULE ===
    // If innermost is a leaf (no child elements), promote to parent block
    let (result_def, matched_child, matched_line) = if !innermost.has_child_elements {
        // Leaf element → promote to parent
        if let Some(parent_index) = innermost.parent_index {
            if let Some(parent_def) = xml_defs.get(parent_index) {
                (parent_def, Some(innermost.entry.name.clone()), Some(line_num))
            } else {
                (innermost, None, None) // Fallback: can't find parent
            }
        } else {
            (innermost, None, None) // No parent (root element)
        }
    } else {
        // Block element → no promotion
        (innermost, None, None)
    };

    // Build result definition JSON
    let mut def_obj = build_xml_def_json(result_def, file_path);

    // Add matchedChild/matchedLine for promoted results
    if let Some(child) = &matched_child {
        def_obj["matchedChild"] = json!(child);
    }
    if let Some(line) = matched_line {
        def_obj["matchedLine"] = json!(line);
    }

    // Include body if requested — we cache source.lines() once per request
    // (MINOR-4 in review: previously every call rebuilt the Vec of lines).
    let source_lines = SourceLines::new(source);
    if args.include_body {
        let body = source_lines.extract_body(
            result_def.entry.line_start,
            result_def.entry.line_end,
            args.max_body_lines,
        );
        def_obj["body"] = json!(body);
        def_obj["bodyStartLine"] = json!(result_def.entry.line_start);
    }

    // Build parent chain (ancestors above the result def, excluding result itself)
    let parent_chain = build_parent_chain(xml_defs, result_def, file_path);

    let search_elapsed = search_start.elapsed();
    let output = json!({
        "definitions": [def_obj],
        "parentChain": parent_chain,
        "query": { "file": file_path, "line": line_num },
        "summary": {
            "totalResults": 1,
            "xmlOnDemand": true,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
            "parseWarnings": warnings,
        }
    });
    ToolCallResult::success(json_to_string(&output))
}

// ---------------------------------------------------------------------------
// name filter path — decomposed into 4 small functions
// ---------------------------------------------------------------------------

/// One match discovered during `classify_matches`.
struct XmlMatch {
    def_index: usize,
    is_text_content: bool,
}

/// Group of leaf textContent matches that share the same parent block.
struct PromotedGroup {
    parent_index: usize,
    matched_children: Vec<(String, u32)>, // (child_name, child_line_start)
}

fn handle_name_filter(
    xml_defs: &[XmlDefinition],
    source: &str,
    file_path: &str,
    name: &str,
    args: &DefinitionSearchArgs,
    search_start: Instant,
    warnings: &[String],
) -> ToolCallResult {
    // Phase 1: classify matches
    let matches = classify_matches(xml_defs, name);

    // Phase 2: compute de-duplication set (name-matched indices).
    let name_matched: HashSet<usize> = matches
        .iter()
        .filter(|m| !m.is_text_content)
        .map(|m| m.def_index)
        .collect();

    // Phase 3: build per-kind buckets, running the parent-promotion rule for
    // textContent-matched leaves.
    let source_lines = SourceLines::new(source);
    let (name_results, promoted_groups, text_content_direct) = build_result_buckets(
        xml_defs,
        &matches,
        &name_matched,
        args,
        &source_lines,
        file_path,
    );

    // Phase 4: assemble the promoted results in deterministic order, then
    // combine all three buckets and apply the max_results cap.
    let promoted_results = assemble_promoted_results(
        xml_defs,
        &promoted_groups,
        args,
        &source_lines,
        file_path,
    );

    let max_results = if args.max_results > 0 { args.max_results } else { 100 };
    let total_results =
        name_results.len() + promoted_results.len() + text_content_direct.len();
    let defs_json: Vec<Value> = name_results
        .into_iter()
        .chain(promoted_results)
        .chain(text_content_direct)
        .take(max_results)
        .collect();

    let search_elapsed = search_start.elapsed();
    let output = json!({
        "definitions": defs_json,
        "summary": {
            "totalResults": total_results,
            "returned": defs_json.len(),
            "xmlOnDemand": true,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
            "parseWarnings": warnings,
        }
    });
    ToolCallResult::success(json_to_string(&output))
}

/// Phase 1: walk the definitions and classify each as a name-match,
/// textContent-match, or non-match. Comma-separated search terms use OR
/// semantics. textContent search ignores terms shorter than 3 chars to avoid
/// noise (a 1-char query against long paragraphs matches almost everything).
fn classify_matches(xml_defs: &[XmlDefinition], name: &str) -> Vec<XmlMatch> {
    let name_lower = name.to_lowercase();
    let terms: Vec<&str> = name_lower
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    // Min-length guard: only search textContent for terms >= 3 chars
    let long_terms: Vec<&str> = terms.iter().filter(|t| t.len() >= 3).copied().collect();

    let mut matches = Vec::new();
    for (idx, def) in xml_defs.iter().enumerate() {
        let def_name_lower = def.entry.name.to_lowercase();
        if terms.iter().any(|t| def_name_lower.contains(t)) {
            matches.push(XmlMatch { def_index: idx, is_text_content: false });
            continue;
        }
        if !long_terms.is_empty()
            && let Some(ref tc) = def.text_content {
                let tc_lower = tc.to_lowercase();
                if long_terms.iter().any(|t| tc_lower.contains(t)) {
                    matches.push(XmlMatch { def_index: idx, is_text_content: true });
                }
            }
    }
    matches
}

/// Phase 3: split matches into three buckets.
///
/// - **name_results**: direct name matches, always returned as-is.
/// - **promoted_groups**: leaf textContent matches grouped under the parent
///   block they belong to. Populated here; the final JSON for them is built
///   later by [`assemble_promoted_results`] so we can sort by parent line.
/// - **text_content_direct**: textContent matches that can't be promoted
///   (root leaves, or blocks that matched via their own text_content).
///
/// De-duplication rule: a textContent leaf is **not** promoted if the parent
/// already appears among name matches — we don't want to show the same parent
/// block twice with different `matchedBy` annotations.
fn build_result_buckets(
    xml_defs: &[XmlDefinition],
    matches: &[XmlMatch],
    name_matched: &HashSet<usize>,
    args: &DefinitionSearchArgs,
    source_lines: &SourceLines<'_>,
    file_path: &str,
) -> (Vec<Value>, HashMap<usize, PromotedGroup>, Vec<Value>) {
    let mut name_results = Vec::new();
    let mut promoted_groups: HashMap<usize, PromotedGroup> = HashMap::new();
    let mut text_content_direct = Vec::new();

    for m in matches {
        let def = &xml_defs[m.def_index];

        if !m.is_text_content {
            // Direct name match — no promotion
            let mut obj = build_xml_def_json(def, file_path);
            obj["matchedBy"] = json!("name");
            attach_body(&mut obj, def, args, source_lines);
            name_results.push(obj);
            continue;
        }

        // textContent match — branch on leaf vs block
        if def.has_child_elements {
            // Block matched by its own textContent (unusual path — block's
            // text_content is None by construction, but we keep this branch
            // defensive against future changes in extract_text_content).
            let mut obj = build_xml_def_json(def, file_path);
            obj["matchedBy"] = json!("textContent");
            attach_body(&mut obj, def, args, source_lines);
            text_content_direct.push(obj);
            continue;
        }

        // Leaf — promote to parent if possible
        match def.parent_index {
            Some(parent_idx) if !name_matched.contains(&parent_idx) => {
                promoted_groups
                    .entry(parent_idx)
                    .or_insert_with(|| PromotedGroup {
                        parent_index: parent_idx,
                        matched_children: Vec::new(),
                    })
                    .matched_children
                    .push((def.entry.name.clone(), def.entry.line_start));
            }
            Some(_) => {
                // parent already in name bucket → drop leaf (de-duplication)
            }
            None => {
                // Root leaf — return as-is, it has nowhere to promote to
                let mut obj = build_xml_def_json(def, file_path);
                obj["matchedBy"] = json!("textContent");
                attach_body(&mut obj, def, args, source_lines);
                text_content_direct.push(obj);
            }
        }
    }

    (name_results, promoted_groups, text_content_direct)
}

/// Phase 4: take the promoted-groups hashmap, sort by parent line (stable
/// deterministic order across runs), and emit the JSON objects.
fn assemble_promoted_results(
    xml_defs: &[XmlDefinition],
    promoted_groups: &HashMap<usize, PromotedGroup>,
    args: &DefinitionSearchArgs,
    source_lines: &SourceLines<'_>,
    file_path: &str,
) -> Vec<Value> {
    let mut sorted: Vec<&PromotedGroup> = promoted_groups.values().collect();
    sorted.sort_by_key(|g| xml_defs[g.parent_index].entry.line_start);

    sorted
        .into_iter()
        .map(|group| {
            let parent_def = &xml_defs[group.parent_index];
            let mut obj = build_xml_def_json(parent_def, file_path);
            obj["matchedBy"] = json!("textContent");

            if group.matched_children.len() == 1 {
                obj["matchedChild"] = json!(group.matched_children[0].0);
                obj["matchedLine"] = json!(group.matched_children[0].1);
            } else {
                let children: Vec<Value> = group
                    .matched_children
                    .iter()
                    .map(|(name, line)| json!({"name": name, "line": line}))
                    .collect();
                obj["matchedChildren"] = json!(children);
            }

            attach_body(&mut obj, parent_def, args, source_lines);
            obj
        })
        .collect()
}

/// Helper: attach `body` + `bodyStartLine` to a result object iff the user
/// asked for it. Centralized here so all 3 buckets emit bodies the same way.
fn attach_body(
    obj: &mut Value,
    def: &XmlDefinition,
    args: &DefinitionSearchArgs,
    source_lines: &SourceLines<'_>,
) {
    if !args.include_body {
        return;
    }
    let body = source_lines.extract_body(
        def.entry.line_start,
        def.entry.line_end,
        args.max_body_lines,
    );
    obj["body"] = json!(body);
    obj["bodyStartLine"] = json!(def.entry.line_start);
}

// ---------------------------------------------------------------------------
// JSON builders
// ---------------------------------------------------------------------------

/// Build the standard JSON view of an XML element: name, kind, file, lines,
/// plus optional parent/signature/attributes/textContent. Used by every
/// result bucket so the response shape is consistent across them.
pub(crate) fn build_xml_def_json(def: &XmlDefinition, file_path: &str) -> Value {
    let mut obj = json!({
        "name": def.entry.name,
        "kind": def.entry.kind.as_str(),
        "file": file_path,
        "lines": format!("{}-{}", def.entry.line_start, def.entry.line_end),
    });
    if let Some(ref parent) = def.entry.parent {
        obj["parent"] = json!(parent);
    }
    if let Some(ref sig) = def.entry.signature {
        obj["signature"] = json!(sig);
    }
    if !def.entry.attributes.is_empty() {
        obj["attributes"] = json!(def.entry.attributes);
    }
    if let Some(ref tc) = def.text_content {
        obj["textContent"] = json!(tc);
    }
    obj
}

/// Walk `target.parent_index` → root, collecting each ancestor as a JSON stub
/// with `bodyOmitted: true`. Used by `containsLine` to expose the structural
/// context without blowing out the response budget (the leaf-to-root chain
/// can be dozens of blocks for deeply nested XML).
pub(crate) fn build_parent_chain(
    xml_defs: &[XmlDefinition],
    target: &XmlDefinition,
    file_path: &str,
) -> Vec<Value> {
    let mut chain = Vec::new();
    let mut current_parent_idx = target.parent_index;

    while let Some(idx) = current_parent_idx {
        if let Some(parent_def) = xml_defs.get(idx) {
            chain.push(json!({
                "name": parent_def.entry.name,
                "kind": parent_def.entry.kind.as_str(),
                "file": file_path,
                "lines": format!("{}-{}", parent_def.entry.line_start, parent_def.entry.line_end),
                "bodyOmitted": true,
            }));
            current_parent_idx = parent_def.parent_index;
        } else {
            break;
        }
    }
    chain
}

// ---------------------------------------------------------------------------
// Line-caching helper (MINOR-4 in review)
// ---------------------------------------------------------------------------

/// Cached, zero-copy view over `source.lines()`.
///
/// The previous implementation rebuilt `source.lines().collect::<Vec<&str>>()`
/// on every call to `extract_body_from_source`. With 100 matches in a 10k-line
/// file that's 1M&nbsp;line copies of work that never changes. This helper
/// builds the vector once per request and lends it out through
/// [`SourceLines::extract_body`].
///
/// The returned body is a `Vec<String>` (owned) because the caller (`json!`)
/// needs owned strings for serialization. We cannot simply hand back `&str`
/// slices because they would be tied to the lifetime of the `SourceLines`
/// instance, which conflicts with JSON assembly.
pub(crate) struct SourceLines<'a> {
    lines: Vec<&'a str>,
}

impl<'a> SourceLines<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Self { lines: source.lines().collect() }
    }

    /// Return lines in the inclusive range `[line_start, line_end]` (1-based),
    /// truncated to `max_body_lines`. A `max_body_lines == 0` means "no limit"
    /// (current callers still cap at ~100 by default in `DefinitionSearchArgs`).
    pub(crate) fn extract_body(
        &self,
        line_start: u32,
        line_end: u32,
        max_body_lines: usize,
    ) -> Vec<String> {
        let start = (line_start as usize).saturating_sub(1);
        let end = (line_end as usize).min(self.lines.len());
        if start >= end {
            return Vec::new();
        }
        let body_lines: Vec<String> = self.lines[start..end]
            .iter()
            .map(|s| (*s).to_string())
            .collect();

        if max_body_lines > 0 && body_lines.len() > max_body_lines {
            let mut truncated = body_lines[..max_body_lines].to_vec();
            truncated.push(format!(
                "... ({} more lines)",
                body_lines.len() - max_body_lines
            ));
            truncated
        } else {
            body_lines
        }
    }
}

// ---------------------------------------------------------------------------
// Hint helper — used by the fallback path in `hint_unsupported_extension`.
// ---------------------------------------------------------------------------

/// Build the "XML structural context available" hint. Extracted so the
/// unsupported-extension hinting code in `definitions.rs` doesn't have to know
/// the XML-on-demand UX phrasing.
pub(crate) fn hint_for_xml_extension(ext_lower: &str) -> String {
    debug_assert!(parser_xml::is_xml_extension(ext_lower));
    format!(
        "XML structural context is available for '.{}' files. \
         Use xray_definitions file='<path.{}>' containsLine=<N> includeBody=true \
         or name='<element>' to get XML structural context on-demand.",
        ext_lower, ext_lower
    )
}