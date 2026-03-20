//! xray_definitions handler: AST-based code definition search.
//!
//! The main entry point is [`handle_xray_definitions`], which orchestrates:
//! 1. Argument parsing ([`parse_definition_args`])
//! 2. Mode dispatch (audit / containsLine / normal search)
//! 3. Index-based candidate collection ([`collect_candidates`])
//! 4. Entry-level filtering ([`apply_entry_filters`])
//! 5. Code stats filtering ([`apply_stats_filters`])
//! 6. Sorting ([`sort_results`])
//! 7. Output formatting ([`format_search_output`])

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::definitions::{DefinitionEntry, DefinitionIndex, DefinitionKind, CodeStats};

use super::utils::{inject_body_into_obj, inject_branch_warning, best_match_tier, json_to_string, name_similarity};
use super::HandlerContext;

#[cfg(feature = "lang-xml")]
use crate::definitions::parser_xml::{is_xml_extension, parse_xml_on_demand};

// ─── Parsed arguments struct ─────────────────────────────────────────

/// Parsed and validated arguments for the xray_definitions tool.
/// Extracted from raw JSON [`Value`] by [`parse_definition_args`].
#[derive(Debug, Clone)]
pub(crate) struct DefinitionSearchArgs {
    pub name_filter: Option<String>,
    pub kind_filter: Option<String>,
    pub attribute_filter: Option<String>,
    pub base_type_filter: Option<String>,
    pub base_type_transitive: bool,
    pub file_filter: Option<String>,
    pub parent_filter: Option<String>,
    pub contains_line: Option<u32>,
    pub use_regex: bool,
    pub max_results: usize,
    pub include_body: bool,
    pub include_doc_comments: bool,
    pub max_body_lines: usize,
    pub max_total_body_lines: usize,
    pub body_line_start: Option<u32>,
    pub body_line_end: Option<u32>,
    pub audit: bool,
    pub audit_min_bytes: u64,
    pub cross_validate: bool,
    // Code stats
    pub sort_by: Option<String>,
    pub min_complexity: Option<u16>,
    pub min_cognitive: Option<u16>,
    pub min_nesting: Option<u8>,
    pub min_params: Option<u8>,
    pub min_returns: Option<u8>,
    pub min_calls: Option<u16>,
    pub include_code_stats: bool,
    // Cross-index enrichment
    pub include_usage_count: bool,
    // Detail level: None = auto (compact when >20 results), Some("full") = always full
    pub detail: Option<String>,
    // Pre-computed filter patterns (avoid per-item allocations)
    pub exclude_patterns: super::utils::ExcludePatterns,
    pub file_filter_terms: Option<Vec<String>>,
    pub parent_filter_terms: Option<Vec<String>>,
}

impl DefinitionSearchArgs {
    /// Returns true if any code stats filter (sortBy or min*) is active.
    pub fn has_stats_filter(&self) -> bool {
        self.sort_by.is_some()
            || self.min_complexity.is_some()
            || self.min_cognitive.is_some()
            || self.min_nesting.is_some()
            || self.min_params.is_some()
            || self.min_returns.is_some()
            || self.min_calls.is_some()
    }
}

// ─── Argument parsing ────────────────────────────────────────────────

/// Parse and validate arguments from raw JSON into [`DefinitionSearchArgs`].
///
/// Returns `Err(message)` for validation failures (invalid containsLine, invalid sortBy).
fn parse_definition_args(args: &Value) -> Result<DefinitionSearchArgs, String> {
    let name_filter = args.get("name").and_then(|v| v.as_str())
        .and_then(|s| if s.is_empty() { None } else { Some(s.to_string()) });
    let kind_filter = args.get("kind").and_then(|v| v.as_str()).map(|s| s.to_string());
    let attribute_filter = args.get("attribute").and_then(|v| v.as_str()).map(|s| s.to_string());
    let base_type_filter = args.get("baseType").and_then(|v| v.as_str())
        .and_then(|s| if s.is_empty() { None } else { Some(s.to_string()) });
    let base_type_transitive = args.get("baseTypeTransitive").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_filter = args.get("file").and_then(|v| v.as_str()).map(|s| s.to_string());
    let parent_filter = args.get("parent").and_then(|v| v.as_str()).map(|s| s.to_string());

    let contains_line = match args.get("containsLine") {
        Some(v) if v.is_i64() || v.is_u64() => {
            match v.as_i64() {
                Some(n) if n < 1 => return Err(
                    format!("containsLine must be >= 1, got {}", n)
                ),
                Some(n) => Some(n as u32),
                None => None,
            }
        }
        _ => None,
    };

    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_results = args.get("maxResults")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    let exclude_dir: Vec<String> = args.get("excludeDir")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let include_doc_comments = args.get("includeDocComments").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_body = args.get("includeBody").and_then(|v| v.as_bool()).unwrap_or(false)
        || include_doc_comments; // includeDocComments implies includeBody
    let max_body_lines = args.get("maxBodyLines").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let max_total_body_lines = args.get("maxTotalBodyLines").and_then(|v| v.as_u64()).unwrap_or(500) as usize;
    let body_line_start = args.get("bodyLineStart").and_then(|v| v.as_u64()).map(|v| v as u32);
    let body_line_end = args.get("bodyLineEnd").and_then(|v| v.as_u64()).map(|v| v as u32);
    let audit = args.get("audit").and_then(|v| v.as_bool()).unwrap_or(false);
    let audit_min_bytes = args.get("auditMinBytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(500);
    let cross_validate = args.get("crossValidate").and_then(|v| v.as_bool()).unwrap_or(false);

    // Code stats parameters
    let sort_by = args.get("sortBy").and_then(|v| v.as_str()).map(|s| s.to_string());
    let min_complexity = args.get("minComplexity").and_then(|v| v.as_u64()).map(|v| v as u16);
    let min_cognitive = args.get("minCognitive").and_then(|v| v.as_u64()).map(|v| v as u16);
    let min_nesting = args.get("minNesting").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_params = args.get("minParams").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_returns = args.get("minReturns").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_calls = args.get("minCalls").and_then(|v| v.as_u64()).map(|v| v as u16);

    // Validate sortBy value
    if let Some(ref sort_field) = sort_by {
        let valid = ["cyclomaticComplexity", "cognitiveComplexity", "maxNestingDepth",
                     "paramCount", "returnCount", "callCount", "lambdaCount", "lines"];
        if !valid.contains(&sort_field.as_str()) {
            return Err(format!(
                "Invalid sortBy value '{}'. Valid values: {}",
                sort_field, valid.join(", ")
            ));
        }
    }

    // Compute derived fields
    let has_stats = sort_by.is_some()
        || min_complexity.is_some()
        || min_cognitive.is_some()
        || min_nesting.is_some()
        || min_params.is_some()
        || min_returns.is_some()
        || min_calls.is_some();

    let include_code_stats = args.get("includeCodeStats").and_then(|v| v.as_bool()).unwrap_or(false)
        || has_stats;

    let include_usage_count = args.get("includeUsageCount").and_then(|v| v.as_bool()).unwrap_or(false);
    let detail = args.get("detail").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Pre-compute filter patterns to avoid per-item allocations
    let exclude_patterns = super::utils::ExcludePatterns::from_dirs(&exclude_dir);
    let file_filter_terms = file_filter.as_ref().map(|ff| {
        ff.split(',')
            .map(|s| s.trim().replace('\\', "/").to_lowercase())
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>()
    });
    let parent_filter_terms = parent_filter.as_ref().map(|pf| {
        pf.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>()
    });

    Ok(DefinitionSearchArgs {
        name_filter,
        kind_filter,
        attribute_filter,
        base_type_filter,
        base_type_transitive,
        file_filter,
        parent_filter,
        contains_line,
        use_regex,
        max_results,
        include_body,
        include_doc_comments,
        max_body_lines,
        max_total_body_lines,
        body_line_start,
        body_line_end,
        audit,
        audit_min_bytes,
        cross_validate,
        sort_by,
        min_complexity,
        min_cognitive,
        min_nesting,
        min_params,
        min_returns,
        min_calls,
        include_code_stats,
        include_usage_count,
        detail,
        exclude_patterns,
        file_filter_terms,
        parent_filter_terms,
    })
}

// ─── Kind priority ───────────────────────────────────────────────────

/// Returns 0 for type-level definitions (class, interface, enum, struct, record),
/// 1 for everything else. Used as a tiebreaker in relevance ranking.
fn kind_priority(kind: &DefinitionKind) -> u8 {
    match kind {
        DefinitionKind::Class
        | DefinitionKind::Interface
        | DefinitionKind::Enum
        | DefinitionKind::Struct
        | DefinitionKind::Record => 0,
        _ => 1,
    }
}

// ─── Main entry point (orchestrator) ─────────────────────────────────

pub(crate) fn handle_xray_definitions(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let def_index = match &ctx.def_index {
        Some(idx) => idx,
        None => return ToolCallResult::error(
            "Definition index not available. Start server with --definitions flag.".to_string()
        ),
    };

    let index = match def_index.read() {
        Ok(idx) => idx,
        Err(e) => return ToolCallResult::error(format!("Failed to acquire definition index lock: {}", e)),
    };

    let search_start = Instant::now();

    // 1. Parse and validate arguments
    let parsed = match parse_definition_args(args) {
        Ok(a) => a,
        Err(msg) => return ToolCallResult::error(msg),
    };

    // 2. Audit mode — early return
    if parsed.audit {
        return handle_audit_mode(&index, &parsed, ctx);
    }

    // 2b. XML on-demand: intercept XML file requests before index-based logic
    #[cfg(feature = "lang-xml")]
    if let Some(result) = try_xml_on_demand(&parsed, search_start, ctx) {
        return result;
    }

    // 3. ContainsLine mode — early return
    if let Some(line_num) = parsed.contains_line {
        return handle_contains_line_mode(&index, &parsed, line_num, search_start, ctx);
    }

    // 4. Collect candidate indices from index-based filters
    let (candidates, def_to_term) = match collect_candidates(&index, &parsed) {
        Ok(result) => result,
        Err(msg) => return ToolCallResult::error(msg),
    };

    // 5. Apply entry-level filters (file, parent, excludeDir)
    let mut results = apply_entry_filters(&index, &candidates, &parsed);

    // 6. Apply code stats filters
    let stats_info = match apply_stats_filters(&index, &mut results, &parsed) {
        Ok(info) => info,
        Err(msg) => return ToolCallResult::error(msg),
    };

    let total_results = results.len();

    // 6a. Auto-correction: if 0 results, try kind/name correction before generating hints
    if total_results == 0 {
        if let Some(corrected_result) = attempt_auto_correction(&index, &parsed, search_start, ctx) {
            return corrected_result;
        }
    }

    // 7. Compute term breakdown (before truncation)
    let term_breakdown = compute_term_breakdown(&results, &def_to_term, &parsed);

    // 8. Sort results
    sort_results(&mut results, &index, &parsed);

    // 9. Auto-summary: if results won't fit AND it's a broad query, return grouped summary
    if should_auto_summary(&parsed, total_results) {
        return build_auto_summary(&index, &results, &parsed, total_results, search_start, ctx);
    }

    // 10. Apply max results
    if parsed.max_results > 0 && results.len() > parsed.max_results {
        results.truncate(parsed.max_results);
    }

    let search_elapsed = search_start.elapsed();

    // 11. Format output
    format_search_output(&index, &results, &parsed, total_results, &stats_info,
                         &term_breakdown, search_elapsed, ctx)
}

// ─── Audit mode ──────────────────────────────────────────────────────

fn handle_audit_mode(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
    ctx: &HandlerContext,
) -> ToolCallResult {
    let files_with_defs = index.file_index.len();
    let total_files = index.files.len();
    let files_without_defs = index.empty_file_ids.len();

    let suspicious: Vec<Value> = index.empty_file_ids.iter()
        .filter(|(_, size)| *size > args.audit_min_bytes)
        .map(|(fid, size)| {
            let path = index.files.get(*fid as usize).map(|s| s.as_str()).unwrap_or("?");
            json!({ "file": path, "bytes": size })
        })
        .collect();

    let mut output = json!({
        "audit": {
            "totalFiles": total_files,
            "filesWithDefinitions": files_with_defs,
            "filesWithoutDefinitions": files_without_defs,
            "readErrors": index.parse_errors,
            "lossyUtf8Files": index.lossy_file_count,
            "suspiciousFiles": suspicious.len(),
            "suspiciousThresholdBytes": args.audit_min_bytes,
        },
        "suspiciousFiles": suspicious,
    });

    // Cross-index validation
    if args.cross_validate {
        let cross = cross_validate_indexes(index, &ctx.server_dir(), &ctx.index_base);
        output["crossValidation"] = cross;
    }

    ToolCallResult::success(json_to_string(&output))
}

// ─── ContainsLine mode ───────────────────────────────────────────────

fn handle_contains_line_mode(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
    line_num: u32,
    search_start: Instant,
    ctx: &HandlerContext,
) -> ToolCallResult {
    let file_filter = match &args.file_filter {
        Some(f) => f,
        None => return ToolCallResult::error(
            "containsLine requires 'file' parameter to identify the file.".to_string()
        ),
    };
    let file_substr = file_filter.replace('\\', "/").to_lowercase();

    let mut containing_defs: Vec<Value> = Vec::new();
    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total_body_lines_emitted: usize = 0;
    let mut total_body_lines_available: usize = 0;

    for (file_id, file_path) in index.files.iter().enumerate() {
        if !file_path.replace('\\', "/").to_lowercase().contains(&file_substr) {
            continue;
        }
        // A2 fix: Apply excludeDir filter using pre-computed patterns
        if !args.exclude_patterns.is_empty() {
            let path_lower = file_path.to_lowercase().replace('\\', "/");
            if args.exclude_patterns.matches(&path_lower) {
                continue;
            }
        }
        if let Some(def_indices) = index.file_index.get(&(file_id as u32)) {
            let mut matching: Vec<&DefinitionEntry> = def_indices.iter()
                .filter_map(|&di| index.definitions.get(di as usize))
                .filter(|d| d.line_start <= line_num && d.line_end >= line_num)
                .collect();

            // A2 fix: Apply kind filter
            if let Some(ref kind_str) = args.kind_filter {
                let kinds: Vec<&str> = kind_str.split(',').map(|s| s.trim()).collect();
                matching.retain(|d| kinds.iter().any(|k| d.kind.as_str().eq_ignore_ascii_case(k)));
            }
            // A2 fix: Apply parent filter
            if let Some(ref parent_str) = args.parent_filter {
                let parent_lower = parent_str.to_lowercase();
                let parent_terms: Vec<&str> = parent_lower.split(',').map(|s| s.trim()).collect();
                matching.retain(|d| {
                    d.parent.as_ref()
                        .map(|p| {
                            let p_lower = p.to_lowercase();
                            parent_terms.iter().any(|term| p_lower.contains(term))
                        })
                        .unwrap_or(false)
                });
            }

            // Sort by range size (smallest first = most specific)
            matching.sort_by_key(|d| d.line_end - d.line_start);

            for (i, def) in matching.iter().enumerate() {
                let mut obj = json!({
                    "name": def.name,
                    "kind": def.kind.as_str(),
                    "file": file_path,
                    "lines": format!("{}-{}", def.line_start, def.line_end),
                });
                if let Some(ref parent) = def.parent {
                    obj["parent"] = json!(parent);
                }
                if let Some(ref sig) = def.signature {
                    obj["signature"] = json!(sig);
                }
                if !def.modifiers.is_empty() {
                    obj["modifiers"] = json!(def.modifiers);
                }
                if args.include_body {
                    if i == 0 {
                        // Track available lines for innermost only (parents don't emit body)
                        total_body_lines_available += (def.line_end.saturating_sub(def.line_start) + 1) as usize;
                        // Innermost definition (smallest range) — emit full body
                        inject_body_into_obj(
                            &mut obj, file_path, def.line_start, def.line_end,
                            &mut file_cache, &mut total_body_lines_emitted,
                            args.max_body_lines, args.max_total_body_lines,
                            args.include_doc_comments,
                            args.body_line_start, args.body_line_end,
                        );
                    } else {
                        // Parent definition — metadata only, skip body to save budget
                        obj["bodyOmitted"] = json!(
                            "parent definition - use includeBody with name filter to get full body"
                        );
                    }
                }
                containing_defs.push(obj);
            }
        }
    }

    // Cross-index enrichment: add usageCount from content index
    if args.include_usage_count {
        if let Ok(content_idx) = ctx.index.read() {
            for obj in &mut containing_defs {
                if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                    let name_lower = name.to_lowercase();
                    let count = content_idx.index.get(&name_lower)
                        .map(|postings| postings.len())
                        .unwrap_or(0);
                    obj["usageCount"] = json!(count);
                }
            }
        }
    }

    let search_elapsed = search_start.elapsed();
    let mut summary = json!({
        "totalResults": containing_defs.len(),
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
    });
    if args.include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
        if total_body_lines_emitted < total_body_lines_available {
            summary["totalBodyLinesAvailable"] = json!(total_body_lines_available);
        }
    }
    inject_branch_warning(&mut summary, ctx);
    let output = json!({
        "containingDefinitions": containing_defs,
        "query": {
            "file": file_filter,
            "line": line_num,
        },
        "summary": summary,
    });
    ToolCallResult::success(json_to_string(&output))
}

// ─── Candidate collection (index-based filtering) ────────────────────

/// Collect candidate definition indices using index-based filters
/// (kind, attribute, baseType, name). Returns candidate indices and
/// a mapping from definition index to which name term matched it
/// (used for termBreakdown in multi-term queries).
fn collect_candidates(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) -> Result<(Vec<u32>, HashMap<u32, usize>), String> {
    let mut candidate_indices: Option<Vec<u32>> = None;
    let mut def_to_term: HashMap<u32, usize> = HashMap::new();

    // Filter by kind first (most selective usually)
    if let Some(ref kind_str) = args.kind_filter {
        let kind_parts: Vec<&str> = kind_str.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let mut all_kind_indices = Vec::new();
        for part in &kind_parts {
            match part.parse::<DefinitionKind>() {
                Ok(kind) => {
                    if let Some(indices) = index.kind_index.get(&kind) {
                        all_kind_indices.extend(indices);
                    }
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
        all_kind_indices.sort_unstable();
        all_kind_indices.dedup();
        candidate_indices = Some(all_kind_indices);
    }

    // Filter by attribute
    if let Some(ref attr) = args.attribute_filter {
        let attr_lower = attr.to_lowercase();
        if let Some(indices) = index.attribute_index.get(&attr_lower) {
            candidate_indices = Some(match candidate_indices {
                Some(existing) => {
                    let set: std::collections::HashSet<u32> = indices.iter().cloned().collect();
                    existing.into_iter().filter(|i| set.contains(i)).collect()
                }
                None => indices.clone(),
            });
        } else {
            candidate_indices = Some(Vec::new());
        }
    }

    // Filter by base type (with optional transitive BFS traversal)
    if let Some(ref bt) = args.base_type_filter {
        let bt_lower = bt.to_lowercase();
        let matching_indices = if args.base_type_transitive {
            collect_transitive_base_type_indices(index, &bt_lower)
        } else {
            let mut indices = Vec::new();
            for (key, idx_list) in &index.base_type_index {
                if key.contains(&bt_lower) {
                    indices.extend(idx_list);
                }
            }
            indices.sort_unstable();
            indices.dedup();
            indices
        };

        if matching_indices.is_empty() {
            candidate_indices = Some(Vec::new());
        } else {
            candidate_indices = Some(match candidate_indices {
                Some(existing) => {
                    let set: std::collections::HashSet<u32> = matching_indices.into_iter().collect();
                    existing.into_iter().filter(|i| set.contains(i)).collect()
                }
                None => matching_indices,
            });
        }
    }

    // Filter by name
    if let Some(ref name) = args.name_filter {
        if args.use_regex {
            let re = match regex::Regex::new(&format!("(?i){}", name)) {
                Ok(r) => r,
                Err(e) => return Err(format!("Invalid regex '{}': {}", name, e)),
            };
            let mut matching_indices = Vec::new();
            for (n, indices) in &index.name_index {
                if re.is_match(n) {
                    matching_indices.extend(indices);
                }
            }
            candidate_indices = Some(match candidate_indices {
                Some(existing) => {
                    let set: std::collections::HashSet<u32> = matching_indices.into_iter().cloned().collect();
                    existing.into_iter().filter(|i| set.contains(i)).collect()
                }
                None => matching_indices.into_iter().cloned().collect(),
            });
        } else {
            // Comma-separated OR search with substring matching
            let terms: Vec<String> = name.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            let mut matching_indices = Vec::new();
            for (n, indices) in &index.name_index {
                if let Some(term_idx) = terms.iter().position(|t| n.contains(t)) {
                    for idx in indices {
                        def_to_term.entry(*idx).or_insert(term_idx);
                    }
                    matching_indices.extend(indices);
                }
            }
            candidate_indices = Some(match candidate_indices {
                Some(existing) => {
                    let set: std::collections::HashSet<u32> = matching_indices.into_iter().cloned().collect();
                    existing.into_iter().filter(|i| set.contains(i)).collect()
                }
                None => matching_indices.into_iter().cloned().collect(),
            });
        }
    }

    // If no filters applied, return all ACTIVE definitions (via file_index to exclude tombstoned)
    let mut candidates = candidate_indices.unwrap_or_else(|| {
        index.file_index.values().flat_map(|v| v.iter().copied()).collect()
    });

    // Deduplicate
    candidates.sort_unstable();
    candidates.dedup();

    Ok((candidates, def_to_term))
}

// ─── Entry-level filtering ───────────────────────────────────────────

/// Apply post-index filters on actual definition entries: file, parent, excludeDir.
fn apply_entry_filters<'a>(
    index: &'a DefinitionIndex,
    candidates: &[u32],
    args: &DefinitionSearchArgs,
) -> Vec<(u32, &'a DefinitionEntry)> {
    candidates.iter()
        .filter_map(|&idx| {
            let def = index.definitions.get(idx as usize)?;
            let file_path = index.files.get(def.file_id as usize)?;

            // File filter: use pre-parsed terms (avoids re-parsing per candidate)
            if let Some(ref file_terms) = args.file_filter_terms {
                let file_lower = file_path.replace('\\', "/").to_lowercase();
                if !file_terms.iter().any(|t| file_lower.contains(t.as_str())) {
                    return None;
                }
            }

            // Parent filter: use pre-parsed terms
            if let Some(ref parent_terms) = args.parent_filter_terms {
                match &def.parent {
                    Some(parent) => {
                        let parent_lower = parent.to_lowercase();
                        if !parent_terms.iter().any(|t| parent_lower.contains(t.as_str())) {
                            return None;
                        }
                    }
                    None => return None,
                }
            }

            // Exclude dir: use pre-computed patterns (avoids per-candidate allocations)
            if !args.exclude_patterns.is_empty() {
                let path_lower = file_path.to_lowercase().replace('\\', "/");
                if args.exclude_patterns.matches(&path_lower) {
                    return None;
                }
            }

            Some((idx, def))
        })
        .collect()
}

// ─── Code stats filtering ────────────────────────────────────────────

/// Information about stats filtering applied to results.
#[derive(Debug)]
struct StatsFilterInfo {
    /// Whether stats filters were actually applied (modified the result set).
    applied: bool,
    /// Number of results before stats filtering was applied.
    before_count: usize,
}

/// Apply code stats filters (minComplexity, minCognitive, etc.) to results.
/// Returns error if stats are needed but not available.
fn apply_stats_filters(
    index: &DefinitionIndex,
    results: &mut Vec<(u32, &DefinitionEntry)>,
    args: &DefinitionSearchArgs,
) -> Result<StatsFilterInfo, String> {
    let before_count = results.len();

    if !args.has_stats_filter() {
        return Ok(StatsFilterInfo { applied: false, before_count });
    }

    // sortBy='lines' works without code_stats, but min* filters always need code_stats
    let has_min_filters = args.min_complexity.is_some() || args.min_cognitive.is_some()
        || args.min_nesting.is_some() || args.min_params.is_some()
        || args.min_returns.is_some() || args.min_calls.is_some();
    let needs_code_stats = has_min_filters || args.sort_by.as_deref() != Some("lines");

    if needs_code_stats && index.code_stats.is_empty() {
        return Err(
            "Code stats not available for this index. Run xray_reindex_definitions to compute metrics.".to_string()
        );
    }

    if needs_code_stats {
        results.retain(|(def_idx, _def)| {
            let stats = match index.code_stats.get(def_idx) {
                Some(s) => s,
                None => return false,
            };

            if let Some(min) = args.min_complexity
                && stats.cyclomatic_complexity < min { return false; }
            if let Some(min) = args.min_cognitive
                && stats.cognitive_complexity < min { return false; }
            if let Some(min) = args.min_nesting
                && stats.max_nesting_depth < min { return false; }
            if let Some(min) = args.min_params
                && stats.param_count < min { return false; }
            if let Some(min) = args.min_returns
                && stats.return_count < min { return false; }
            if let Some(min) = args.min_calls
                && stats.call_count < min { return false; }
            true
        });

        return Ok(StatsFilterInfo { applied: true, before_count });
    }

    Ok(StatsFilterInfo { applied: false, before_count })
}

// ─── Term breakdown ──────────────────────────────────────────────────

/// Compute term breakdown for multi-term name queries.
/// Returns per-term result counts (computed from full result set before truncation).
fn compute_term_breakdown(
    results: &[(u32, &DefinitionEntry)],
    def_to_term: &HashMap<u32, usize>,
    args: &DefinitionSearchArgs,
) -> Option<Value> {
    if args.use_regex {
        return None;
    }

    let name = args.name_filter.as_deref()?;
    let terms: Vec<String> = name.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    if terms.len() < 2 {
        return None;
    }

    let mut breakdown = serde_json::Map::new();
    for (i, term) in terms.iter().enumerate() {
        let count = results.iter()
            .filter(|(idx, _)| def_to_term.get(idx) == Some(&i))
            .count();
        breakdown.insert(term.clone(), json!(count));
    }
    Some(json!(breakdown))
}

/// Detect missing terms in multi-name queries when kind filter causes some terms to silently drop.
/// Returns a JSON array of `{term, reason}` objects for terms that exist in the index
/// but were filtered out by the kind constraint.
fn compute_missing_terms(
    index: &DefinitionIndex,
    defs_json: &[Value],
    args: &DefinitionSearchArgs,
) -> Option<Value> {
    // Only relevant for multi-name + kind filter + non-regex + results > 0
    if args.use_regex { return None; }
    let name = args.name_filter.as_deref()?;
    let kind_str = args.kind_filter.as_deref()?;

    let terms: Vec<String> = name.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if terms.len() < 2 { return None; }
    if defs_json.is_empty() { return None; } // 0-result case handled by auto-correction

    // Determine which terms produced results (from JSON output)
    let found_names: std::collections::HashSet<String> = defs_json.iter()
        .filter_map(|d| d.get("name").and_then(|v| v.as_str()))
        .map(|n| n.to_lowercase())
        .collect();

    let mut missing = Vec::new();
    for term in &terms {
        let term_found = found_names.iter().any(|n| n.contains(term.as_str()));
        if term_found { continue; }

        // Check if this term exists in name_index with a DIFFERENT kind
        let mut actual_kinds: Vec<&str> = Vec::new();
        for (n, indices) in &index.name_index {
            if n.contains(term.as_str()) {
                for &idx in indices {
                    if let Some(def) = index.definitions.get(idx as usize) {
                        let k = def.kind.as_str();
                        if !actual_kinds.contains(&k) {
                            actual_kinds.push(k);
                        }
                    }
                }
            }
        }

        if !actual_kinds.is_empty() {
            missing.push(json!({
                "term": term,
                "reason": format!("kind mismatch: found as {}, not {}",
                    actual_kinds.join("/"), kind_str)
            }));
        } else {
            missing.push(json!({
                "term": term,
                "reason": "not found in index"
            }));
        }
    }

    if missing.is_empty() { None } else { Some(json!(missing)) }
}

// ─── Sorting ─────────────────────────────────────────────────────────

/// Sort results by metric (descending) or by relevance ranking.
fn sort_results(
    results: &mut Vec<(u32, &DefinitionEntry)>,
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) {
    if let Some(ref sort_field) = args.sort_by {
        // Sort by metric (descending — worst first)
        results.sort_by(|(idx_a, def_a), (idx_b, def_b)| {
            let va = get_sort_value(index.code_stats.get(idx_a), def_a, sort_field);
            let vb = get_sort_value(index.code_stats.get(idx_b), def_b, sort_field);
            vb.cmp(&va)
        });
    } else if (args.name_filter.is_some() && !args.use_regex) || args.parent_filter.is_some() {
        // Relevance ranking when name or parent filter is active (not regex)
        let name_terms: Vec<String> = args.name_filter.as_deref()
            .map(|n| n.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();

        let parent_terms: Vec<String> = args.parent_filter.as_deref()
            .map(|p| p.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();

        results.sort_by(|(_, a), (_, b)| {
            let parent_tier_a = if !parent_terms.is_empty() {
                a.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
            } else { 0 };
            let parent_tier_b = if !parent_terms.is_empty() {
                b.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
            } else { 0 };

            parent_tier_a.cmp(&parent_tier_b)
                .then_with(|| {
                    if !name_terms.is_empty() {
                        let tier_a = best_match_tier(&a.name, &name_terms);
                        let tier_b = best_match_tier(&b.name, &name_terms);
                        tier_a.cmp(&tier_b)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| kind_priority(&a.kind).cmp(&kind_priority(&b.kind)))
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.name.cmp(&b.name))
        });
    }
}

// ─── Output formatting ──────────────────────────────────────────────

/// Format the final JSON output from filtered, sorted results.
#[allow(clippy::too_many_arguments)]
fn format_search_output(
    index: &DefinitionIndex,
    results: &[(u32, &DefinitionEntry)],
    args: &DefinitionSearchArgs,
    total_results: usize,
    stats_info: &StatsFilterInfo,
    term_breakdown: &Option<Value>,
    search_elapsed: std::time::Duration,
    ctx: &HandlerContext,
) -> ToolCallResult {
    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total_body_lines_emitted: usize = 0;
    // Count total body lines available (before truncation) for size hint
    let total_body_lines_available: usize = if args.include_body {
        results.iter().map(|(_, def)| (def.line_end.saturating_sub(def.line_start) + 1) as usize).sum()
    } else { 0 };

    // Determine auto-compact mode
    let force_full = args.detail.as_deref() == Some("full");
    let auto_compact = !force_full
        && !args.include_body
        && results.len() > 20
        && args.name_filter.is_none();

    let mut defs_json: Vec<Value> = results.iter().map(|(def_idx_value, def)| {
        format_definition_entry(
            index, *def_idx_value, def, args,
            &mut file_cache, &mut total_body_lines_emitted,
            auto_compact,
        )
    }).collect();

    // Cross-index enrichment: add usageCount from content index
    if args.include_usage_count {
        if let Ok(content_idx) = ctx.index.read() {
            for obj in &mut defs_json {
                if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                    let name_lower = name.to_lowercase();
                    let count = content_idx.index.get(&name_lower)
                        .map(|postings| postings.len())
                        .unwrap_or(0);
                    obj["usageCount"] = json!(count);
                }
            }
        }
    }

    let mut summary = build_search_summary(
        index, &defs_json, args, total_results,
        stats_info, term_breakdown, total_body_lines_emitted,
        total_body_lines_available, search_elapsed, ctx,
    );

    // Add compact mode indicators to summary
    if auto_compact {
        summary["compactMode"] = json!(true);
        summary["compactReason"] = json!(format!(
            "Auto-compact: {} results without name filter. Use detail='full' for signatures/modifiers, or name='X' to get full details for specific definitions.",
            results.len()
        ));
    }

    let output = json!({
        "definitions": defs_json,
        "summary": summary,
    });

    ToolCallResult::success(json_to_string(&output))
}

/// Format a single definition entry as a JSON object.
fn format_definition_entry(
    index: &DefinitionIndex,
    def_idx: u32,
    def: &DefinitionEntry,
    args: &DefinitionSearchArgs,
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    compact: bool,
) -> Value {
    let file_path = index.files.get(def.file_id as usize)
        .map(|s| s.as_str())
        .unwrap_or("");

    let mut obj = json!({
        "name": def.name,
        "kind": def.kind.as_str(),
        "file": file_path,
        "lines": format!("{}-{}", def.line_start, def.line_end),
    });

    // In compact mode, only include parent (for context) — skip signature, modifiers, attributes, baseTypes
    if let Some(ref parent) = def.parent {
        obj["parent"] = json!(parent);
    }

    if compact {
        return obj;
    }

    if !def.modifiers.is_empty() {
        obj["modifiers"] = json!(def.modifiers);
    }
    if !def.attributes.is_empty() {
        obj["attributes"] = json!(def.attributes);
    }
    if !def.base_types.is_empty() {
        obj["baseTypes"] = json!(def.base_types);
    }
    if let Some(ref sig) = def.signature {
        obj["signature"] = json!(sig);
    }
    // Angular template metadata
    if let Some(children) = index.template_children.get(&def_idx) {
        obj["templateChildren"] = json!(children);
    }
    for (selector, sel_indices) in &index.selector_index {
        if sel_indices.contains(&def_idx) {
            obj["selector"] = json!(selector);
            break;
        }
    }

    if args.include_body {
        inject_body_into_obj(
            &mut obj, file_path, def.line_start, def.line_end,
            file_cache, total_body_lines_emitted,
            args.max_body_lines, args.max_total_body_lines,
            args.include_doc_comments,
            args.body_line_start, args.body_line_end,
        );
    }

    // Inject codeStats if requested
    if args.include_code_stats
        && let Some(stats) = index.code_stats.get(&def_idx) {
            let lines = def.line_end.saturating_sub(def.line_start) + 1;
            obj["codeStats"] = json!({
                "lines": lines,
                "cyclomaticComplexity": stats.cyclomatic_complexity,
                "cognitiveComplexity": stats.cognitive_complexity,
                "maxNestingDepth": stats.max_nesting_depth,
                "paramCount": stats.param_count,
                "returnCount": stats.return_count,
                "callCount": stats.call_count,
                "lambdaCount": stats.lambda_count,
            });
        }

    obj
}

/// Build the summary JSON object for the search response.
#[allow(clippy::too_many_arguments)]
fn build_search_summary(
    index: &DefinitionIndex,
    defs_json: &[Value],
    args: &DefinitionSearchArgs,
    total_results: usize,
    stats_info: &StatsFilterInfo,
    term_breakdown: &Option<Value>,
    total_body_lines_emitted: usize,
    total_body_lines_available: usize,
    search_elapsed: std::time::Duration,
    ctx: &HandlerContext,
) -> Value {
    let active_definitions: usize = index.file_index.values().map(|v| v.len()).sum();
    let mut summary = json!({
        "totalResults": total_results,
        "returned": defs_json.len(),
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        "indexFiles": index.files.len(),
        "totalDefinitions": active_definitions,
    });

    // Hint: kind="property" returning 0 results for TypeScript — suggest kind="field"
    if total_results == 0
        && let Some(ref kind_str) = args.kind_filter
            && kind_str.eq_ignore_ascii_case("property") {
                // Check if there ARE "field" definitions that would match the same filters
                let has_fields = index.kind_index.get(&DefinitionKind::Field)
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                if has_fields {
                    summary["hint"] = json!(
                        "kind='property' returned 0 results. In TypeScript, class members are indexed as kind='field', \
                         while only interface property signatures use kind='property'. Try kind='field' instead."
                    );
                }
            }

    // Hint for large transitive hierarchies
    if args.base_type_transitive && total_results > 5000
        && let Some(ref bt) = args.base_type_filter {
            summary["hint"] = json!(format!(
                "Hierarchy of '{}' has {} transitive descendants. Consider adding 'kind' or 'file' filters to narrow results.",
                bt, total_results
            ));
        }

    // Hint: name+kind mismatch — user asked for name=X with kind=method/property/field,
    // but X is actually a class/interface/struct. Suggest using parent=X instead.
    if let Some(ref kind_str) = args.kind_filter {
        let is_member_kind = kind_str.eq_ignore_ascii_case("method")
            || kind_str.eq_ignore_ascii_case("property")
            || kind_str.eq_ignore_ascii_case("field")
            || kind_str.eq_ignore_ascii_case("constructor");
        if is_member_kind
            && args.name_filter.is_some()
            && total_results > 0
            && summary.get("hint").is_none()
        {
            // Check if ALL returned results are type-level (class/interface/struct/enum/record)
            let all_type_level = defs_json.iter().all(|d| {
                d.get("kind").and_then(|k| k.as_str()).map(|k| {
                    matches!(k, "class" | "interface" | "struct" | "enum" | "record")
                }).unwrap_or(false)
            });
            if all_type_level {
                let name_val = args.name_filter.as_ref().unwrap();
                let first_kind = defs_json.first()
                    .and_then(|d| d.get("kind").and_then(|k| k.as_str()))
                    .unwrap_or("class");
                summary["hint"] = json!(format!(
                    "'{}' is a {}. To find its members, use parent='{}' with kind='{}' instead of name='{}'.",
                    name_val, first_kind, name_val, kind_str, name_val
                ));
            }
        }
    }

    // ─── Zero-result hints (A/B/C/D) ────────────────────────────────
    // Generate contextual hints when search returns 0 results to help LLM self-correct.
    // Only one hint per query (first matching wins). Guards ensure no overwrite of existing hints.
    if total_results == 0 {
        generate_zero_result_hints(index, args, &mut summary, ctx);
    }

    if index.parse_errors > 0 {
        summary["readErrors"] = json!(index.parse_errors);
    }
    if index.lossy_file_count > 0 {
        summary["lossyUtf8Files"] = json!(index.lossy_file_count);
    }
    if args.include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
        if total_body_lines_emitted < total_body_lines_available {
            summary["totalBodyLinesAvailable"] = json!(total_body_lines_available);
        }
    }
    if let Some(ref sort_field) = args.sort_by {
        summary["sortedBy"] = json!(sort_field);
    }
    if stats_info.applied {
        summary["statsFiltersApplied"] = json!(true);
        summary["afterStatsFilter"] = json!(total_results);
        summary["beforeStatsFilter"] = json!(stats_info.before_count);
    }
    if args.include_code_stats && index.code_stats.is_empty() {
        summary["codeStatsAvailable"] = json!(false);
    }
    if let Some(breakdown) = term_breakdown {
        summary["termBreakdown"] = breakdown.clone();
    }
    if let Some(missing) = compute_missing_terms(index, defs_json, args) {
        summary["missingTerms"] = missing;
    }
    inject_branch_warning(&mut summary, ctx);

    summary
}

// ─── Utility functions ───────────────────────────────────────────────

/// Extract a numeric value from CodeStats for sorting.
fn get_sort_value(stats: Option<&CodeStats>, def: &DefinitionEntry, field: &str) -> u32 {
    match field {
        "lines" => def.line_end.saturating_sub(def.line_start) + 1,
        _ => {
            let s = match stats {
                Some(s) => s,
                None => return 0,
            };
            match field {
                "cyclomaticComplexity" => s.cyclomatic_complexity as u32,
                "cognitiveComplexity" => s.cognitive_complexity as u32,
                "maxNestingDepth" => s.max_nesting_depth as u32,
                "paramCount" => s.param_count as u32,
                "returnCount" => s.return_count as u32,
                "callCount" => s.call_count as u32,
                "lambdaCount" => s.lambda_count as u32,
                _ => 0,
            }
        }
    }
}

/// Cross-validate definition index files against the file-list index on disk.
///
/// Loads the FileIndex from disk (adhoc, not kept in memory), then compares:
/// - Files in file-list but missing from definition index (filtered by def extensions)
/// - Files in definition index but missing from file-list
///
/// Returns a JSON object with the cross-validation results.
fn cross_validate_indexes(
    def_index: &crate::definitions::DefinitionIndex,
    server_dir: &str,
    index_base: &std::path::Path,
) -> serde_json::Value {
    // Try to load FileIndex from disk
    let file_index = match crate::index::load_index(server_dir, index_base) {
        Ok(fi) => fi,
        Err(_) => {
            return serde_json::json!({
                "status": "skipped",
                "reason": "File-list index not found on disk. Run xray_reindex or xray_fast first."
            });
        }
    };

    let def_extensions: std::collections::HashSet<String> = def_index.extensions.iter()
        .map(|e| e.to_lowercase())
        .collect();

    // Build set of definition index file paths (lowercased for comparison)
    let def_files: std::collections::HashSet<String> = def_index.files.iter()
        .map(|f| f.to_lowercase())
        .collect();

    // Files in file-list matching definition extensions but NOT in definition index
    let mut in_filelist_not_in_defs: Vec<String> = Vec::new();
    for entry in &file_index.entries {
        if entry.is_dir { continue; }
        let ext = std::path::Path::new(&entry.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !def_extensions.contains(&ext) { continue; }
        if !def_files.contains(&entry.path.to_lowercase()) {
            in_filelist_not_in_defs.push(entry.path.clone());
        }
    }

    // Files in definition index but NOT in file-list
    let filelist_paths: std::collections::HashSet<String> = file_index.entries.iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.path.to_lowercase())
        .collect();

    let mut in_defs_not_in_filelist: Vec<String> = Vec::new();
    for def_file in &def_index.files {
        if !filelist_paths.contains(&def_file.to_lowercase()) {
            in_defs_not_in_filelist.push(def_file.clone());
        }
    }

    // Cap results to avoid huge output
    let max_report = 50;
    let total_missing_from_defs = in_filelist_not_in_defs.len();
    let total_missing_from_filelist = in_defs_not_in_filelist.len();
    in_filelist_not_in_defs.truncate(max_report);
    in_defs_not_in_filelist.truncate(max_report);

    serde_json::json!({
        "status": "ok",
        "fileListFiles": file_index.entries.len(),
        "defIndexFiles": def_index.files.len(),
        "inFileListNotInDefIndex": total_missing_from_defs,
        "inDefIndexNotInFileList": total_missing_from_filelist,
        "sampleMissingFromDefIndex": in_filelist_not_in_defs,
        "sampleMissingFromFileList": in_defs_not_in_filelist,
    })
}

/// BFS traversal of the inheritance hierarchy to collect all definition indices
/// that transitively inherit from a given base type.
///
/// Starting from `base_type_name`, finds all classes/structs that directly inherit it,
/// then finds all classes that inherit from THOSE, etc., up to `MAX_BFS_DEPTH` levels.
///
/// Returns a combined Vec of definition indices from all levels.
///
/// Level 0 (seed): uses substring matching on base_type_index keys to support
/// generic types (e.g., "IAccessTable" matches "iaccesstable<model>").
///
/// Levels 1+: uses EXACT HashMap lookup (`base_type_index.get(&name)`) to prevent
/// cascade bugs where a short descendant name (e.g., "service") would substring-match
/// many unrelated base_type keys ("iservice", "webservice", "serviceprovider", etc.),
/// pulling thousands of unrelated definitions into the result set.
///
/// Known limitation: matching is by name only (no namespace resolution).
/// Classes with the same name in different namespaces will be conflated.
fn collect_transitive_base_type_indices(
    index: &crate::definitions::DefinitionIndex,
    base_type_name: &str,
) -> Vec<u32> {
    const MAX_BFS_DEPTH: usize = 10;

    let mut result: Vec<u32> = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    // Seed (level 0): substring match to support generic types
    visited.insert(base_type_name.to_string());
    for (key, indices) in &index.base_type_index {
        if key.contains(base_type_name) {
            result.extend(indices);
            for &def_idx in indices {
                if let Some(def) = index.definitions.get(def_idx as usize) {
                    let name = def.name.to_lowercase();
                    if visited.insert(name.clone()) {
                        queue.push_back(name);
                    }
                }
            }
        }
    }

    // BFS levels 1+: EXACT key matching — prevents cascade bug
    let mut depth = 1;
    while !queue.is_empty() && depth < MAX_BFS_DEPTH {
        let level_size = queue.len();
        for _ in 0..level_size {
            let current_type = queue.pop_front().unwrap();
            if let Some(indices) = index.base_type_index.get(&current_type) {
                result.extend(indices);
                for &def_idx in indices {
                    if let Some(def) = index.definitions.get(def_idx as usize) {
                        let name = def.name.to_lowercase();
                        if visited.insert(name.clone()) {
                            queue.push_back(name);
                        }
                    }
                }
            }
        }
        depth += 1;
    }

    // Deduplicate
    result.sort_unstable();
    result.dedup();
    result
}

// ─── Auto-correction for 0 results ───────────────────────────────────

/// Attempt to auto-correct the query when 0 results are found.
/// Tries two corrections in priority order:
/// A. Kind mismatch — remove kind filter, find the correct kind, re-run
/// B. Nearest name match — fix name to nearest match (≥85% similarity), re-run
///
/// Returns `Some(ToolCallResult)` with corrected results + `autoCorrection` metadata,
/// or `None` if no correction produced results.
fn attempt_auto_correction(
    index: &DefinitionIndex,
    original_args: &DefinitionSearchArgs,
    search_start: Instant,
    ctx: &HandlerContext,
) -> Option<ToolCallResult> {
    // A. Kind mismatch: kind filter is set + name/file filter exists
    if let Some(ref original_kind) = original_args.kind_filter {
        if original_args.name_filter.is_some() || original_args.file_filter.is_some() {
            if let Some(result) = try_kind_correction(index, original_args, original_kind, search_start, ctx) {
                return Some(result);
            }
        }
    }

    // B. Nearest name match (≥85% similarity)
    if let Some(ref original_name) = original_args.name_filter {
        if !original_args.use_regex {
            if let Some(result) = try_name_correction(index, original_args, original_name, search_start, ctx) {
                return Some(result);
            }
        }
    }

    None
}

/// Try auto-correcting a kind mismatch.
/// Removes the kind filter, collects candidates, finds the most common kind,
/// then re-runs the full pipeline with the corrected kind.
fn try_kind_correction(
    index: &DefinitionIndex,
    original_args: &DefinitionSearchArgs,
    original_kind: &str,
    search_start: Instant,
    ctx: &HandlerContext,
) -> Option<ToolCallResult> {
    let mut probe_args = original_args.clone();
    probe_args.kind_filter = None;

    let (candidates, _) = collect_candidates(index, &probe_args).ok()?;
    let filtered = apply_entry_filters(index, &candidates, &probe_args);

    if filtered.is_empty() {
        return None;
    }

    // Summarize available kinds for the autoCorrection message
    let mut kind_counts: HashMap<&str, usize> = HashMap::new();
    for (_, def) in &filtered {
        *kind_counts.entry(def.kind.as_str()).or_insert(0) += 1;
    }
    let mut kinds_sorted: Vec<(&&str, &usize)> = kind_counts.iter().collect();
    kinds_sorted.sort_by(|a, b| b.1.cmp(a.1));
    let kinds_str: Vec<String> = kinds_sorted.iter()
        .map(|(k, c)| format!("{} {}", c, k))
        .collect();

    // Return ALL results without kind filter (don't guess which kind the user wanted)
    let mut corrected_args = original_args.clone();
    corrected_args.kind_filter = None;

    run_corrected_search(index, &corrected_args, search_start, ctx, json!({
        "type": "kindCorrected",
        "original": { "kind": original_kind },
        "corrected": { "kind": null },
        "availableKinds": kinds_str.join(", "),
        "reason": format!(
            "kind='{}' returned 0 results. Removed kind filter — found {} definitions ({})",
            original_kind, filtered.len(), kinds_str.join(", ")
        ),
    }))
}

/// Try auto-correcting a name mismatch via nearest match (≥85% Jaro-Winkler).
/// Finds the closest name in the definition index and re-runs the search.
const AUTO_CORRECT_NAME_THRESHOLD: f64 = 0.80;
/// Minimum length ratio (shorter/longer) for auto-correction to fire.
/// Prevents partial-match corrections like "xray_definitions" → "search" (ratio 6/18 = 0.33).
const AUTO_CORRECT_MIN_LENGTH_RATIO: f64 = 0.6;

fn try_name_correction(
    index: &DefinitionIndex,
    original_args: &DefinitionSearchArgs,
    original_name: &str,
    search_start: Instant,
    ctx: &HandlerContext,
) -> Option<ToolCallResult> {
    let search_lower = original_name.to_lowercase();
    let mut best_name: Option<String> = None;
    let mut best_score: f64 = 0.0;

    for (index_name, _) in &index.name_index {
        let score = name_similarity(&search_lower, index_name);
        if score >= AUTO_CORRECT_NAME_THRESHOLD && score > best_score {
            // Guard: reject corrections where query and match differ too much in length.
            // Jaro-Winkler inflates similarity for shared prefixes (e.g., "xray_definitions" vs "search" = 87%)
            // but 6/18 = 33% length ratio reveals it's a partial match, not a typo.
            let length_ratio = search_lower.len().min(index_name.len()) as f64
                / search_lower.len().max(index_name.len()) as f64;
            if length_ratio < AUTO_CORRECT_MIN_LENGTH_RATIO {
                continue;
            }
            best_score = score;
            best_name = Some(index_name.clone());
        }
    }

    let corrected_name = best_name?;

    let mut corrected_args = original_args.clone();
    corrected_args.name_filter = Some(corrected_name.clone());

    run_corrected_search(index, &corrected_args, search_start, ctx, json!({
        "type": "nameCorrected",
        "original": { "name": original_name },
        "corrected": { "name": &corrected_name },
        "similarity": format!("{:.0}%", best_score * 100.0),
        "reason": format!(
            "name='{}' returned 0 results, auto-corrected to name='{}' ({:.0}% similarity)",
            original_name, corrected_name, best_score * 100.0
        ),
    }))
}

/// Execute the full search pipeline with corrected args and inject autoCorrection metadata.
/// Returns `Some(ToolCallResult)` if the corrected search produces results, `None` otherwise.
fn run_corrected_search(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
    search_start: Instant,
    ctx: &HandlerContext,
    auto_correction: Value,
) -> Option<ToolCallResult> {
    let (candidates, def_to_term) = collect_candidates(index, args).ok()?;
    let mut results = apply_entry_filters(index, &candidates, args);

    // Apply stats filters (ignore error — correction is best-effort)
    let stats_info = apply_stats_filters(index, &mut results, args).ok()?;

    let total_results = results.len();
    if total_results == 0 {
        return None; // Correction didn't produce results
    }

    let term_breakdown = compute_term_breakdown(&results, &def_to_term, args);
    sort_results(&mut results, index, args);

    if args.max_results > 0 && results.len() > args.max_results {
        results.truncate(args.max_results);
    }

    let search_elapsed = search_start.elapsed();

    let tool_result = format_search_output(
        index, &results, args, total_results, &stats_info,
        &term_breakdown, search_elapsed, ctx,
    );

    // Inject autoCorrection into the response summary
    if let Some(content) = tool_result.content.first() {
        if let Ok(mut output) = serde_json::from_str::<Value>(&content.text) {
            if let Some(summary) = output.get_mut("summary") {
                summary["autoCorrection"] = auto_correction;
            }
            return Some(ToolCallResult::success(json_to_string(&output)));
        }
    }

    Some(tool_result)
}

// ─── XML On-Demand Handler ───────────────────────────────────────────

/// Try handling a request for an XML file on-demand (without definition index).
///
/// Intercepts requests where `file` filter points to an XML extension file
/// AND either `containsLine` or `name` filter is active. These files are
/// NOT in the definition index — they are parsed on-the-fly via tree-sitter-xml.
///
/// Key feature: **Parent Promotion** — if the innermost element at `containsLine`
/// is a leaf (no child elements), the result is promoted to the parent block.
/// This returns the full configuration section instead of a trivial leaf element.
#[cfg(feature = "lang-xml")]
fn try_xml_on_demand(
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

    // Only handle containsLine or name filter queries
    if args.contains_line.is_none() && args.name_filter.is_none() {
        return None;
    }

    // Resolve file path: try absolute path first, then relative to workspace
    let file_path = resolve_xml_file_path(file_filter, ctx)?;

    // Check if path is a directory — XML on-demand requires a specific file
    if std::path::Path::new(&file_path).is_dir() {
        return Some(ToolCallResult::error(format!(
            "XML on-demand requires a file path, not a directory: '{}'. \
             Use xray_fast pattern='*.xml' dir='{}' to find specific XML files, \
             then pass the full file path to xray_definitions.",
            file_filter, file_filter
        )));
    }

    // Read file content
    let source = match std::fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(e) => {
            return Some(ToolCallResult::error(format!(
                "Failed to read XML file '{}': {}. Hint: check file path and permissions.",
                file_path, e
            )));
        }
    };

    // Parse XML on-demand
    let xml_defs = match parse_xml_on_demand(&source, &file_path) {
        Ok(defs) => defs,
        Err(e) => {
            return Some(ToolCallResult::error(format!(
                "Failed to parse XML file '{}': {}. The file may contain malformed XML.",
                file_path, e
            )));
        }
    };

    if xml_defs.is_empty() {
        return Some(ToolCallResult::success(json_to_string(&json!({
            "definitions": [],
            "summary": {
                "totalResults": 0,
                "onDemand": true,
                "hint": "XML file parsed but no elements found."
            }
        }))));
    }

    if let Some(line_num) = args.contains_line {
        Some(handle_xml_contains_line(&xml_defs, &source, &file_path, line_num, args, search_start, ctx))
    } else if let Some(ref name) = args.name_filter {
        Some(handle_xml_name_filter(&xml_defs, &source, &file_path, name, args, search_start, ctx))
    } else {
        None
    }
}

/// Handle XML containsLine with parent promotion.
#[cfg(feature = "lang-xml")]
fn handle_xml_contains_line(
    xml_defs: &[crate::definitions::parser_xml::XmlDefinition],
    source: &str,
    file_path: &str,
    line_num: u32,
    args: &DefinitionSearchArgs,
    search_start: Instant,
    _ctx: &HandlerContext,
) -> ToolCallResult {
    // Find all elements containing this line, sorted by range (smallest first)
    let mut containing: Vec<(usize, &crate::definitions::parser_xml::XmlDefinition)> = xml_defs.iter()
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
                "onDemand": true,
                "hint": format!("Line {} is not inside any XML element in '{}'.", line_num, file_path)
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

    // Include body if requested
    if args.include_body {
        let body = extract_body_from_source(source, result_def.entry.line_start, result_def.entry.line_end, args.max_body_lines);
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
            "onDemand": true,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        }
    });
    ToolCallResult::success(json_to_string(&output))
}

/// Handle XML name filter (search by element name).

#[cfg(feature = "lang-xml")]
fn handle_xml_name_filter(
    xml_defs: &[crate::definitions::parser_xml::XmlDefinition],
    source: &str,
    file_path: &str,
    name: &str,
    args: &DefinitionSearchArgs,
    search_start: Instant,
    _ctx: &HandlerContext,
) -> ToolCallResult {
    let name_lower = name.to_lowercase();
    let terms: Vec<&str> = name_lower.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // Phase 1: Classify matches — name (tag) vs textContent
    // Min-length guard: only search textContent for terms >= 3 chars
    let long_terms: Vec<&str> = terms.iter().filter(|t| t.len() >= 3).copied().collect();

    struct XmlMatch {
        def_index: usize,
        is_text_content: bool,
    }

    let mut matches: Vec<XmlMatch> = Vec::new();
    for (idx, def) in xml_defs.iter().enumerate() {
        let def_name_lower = def.entry.name.to_lowercase();
        if terms.iter().any(|t| def_name_lower.contains(t)) {
            matches.push(XmlMatch { def_index: idx, is_text_content: false });
            continue;
        }
        if !long_terms.is_empty() {
            if let Some(ref tc) = def.text_content {
                let tc_lower = tc.to_lowercase();
                if long_terms.iter().any(|t| tc_lower.contains(t)) {
                    matches.push(XmlMatch { def_index: idx, is_text_content: true });
                }
            }
        }
    }

    // Phase 2: Collect name-matched indices for de-duplication
    let name_matched_indices: std::collections::HashSet<usize> = matches.iter()
        .filter(|m| !m.is_text_content)
        .map(|m| m.def_index)
        .collect();

    // Phase 3: Build results with parent promotion and de-duplication
    struct PromotedGroup {
        parent_index: usize,
        matched_children: Vec<(String, u32)>, // (child_name, child_line_start)
    }

    let mut name_results: Vec<Value> = Vec::new();
    let mut promoted_groups: HashMap<usize, PromotedGroup> = HashMap::new();
    let mut text_content_direct: Vec<Value> = Vec::new();

    for m in &matches {
        let def = &xml_defs[m.def_index];

        if !m.is_text_content {
            // Name match — direct result, no promotion
            let mut obj = build_xml_def_json(def, file_path);
            obj["matchedBy"] = json!("name");
            if args.include_body {
                let body = extract_body_from_source(source, def.entry.line_start, def.entry.line_end, args.max_body_lines);
                obj["body"] = json!(body);
                obj["bodyStartLine"] = json!(def.entry.line_start);
            }
            name_results.push(obj);
        } else {
            // textContent match
            if !def.has_child_elements {
                // Leaf → promote to parent
                if let Some(parent_idx) = def.parent_index {
                    // Skip if parent already matched by name
                    if name_matched_indices.contains(&parent_idx) {
                        continue;
                    }
                    promoted_groups.entry(parent_idx)
                        .or_insert_with(|| PromotedGroup {
                            parent_index: parent_idx,
                            matched_children: Vec::new(),
                        })
                        .matched_children.push((def.entry.name.clone(), def.entry.line_start));
                } else {
                    // No parent (root leaf) — return as-is
                    let mut obj = build_xml_def_json(def, file_path);
                    obj["matchedBy"] = json!("textContent");
                    if args.include_body {
                        let body = extract_body_from_source(source, def.entry.line_start, def.entry.line_end, args.max_body_lines);
                        obj["body"] = json!(body);
                        obj["bodyStartLine"] = json!(def.entry.line_start);
                    }
                    text_content_direct.push(obj);
                }
            } else {
                // Block matched by textContent (unusual) — return as-is
                let mut obj = build_xml_def_json(def, file_path);
                obj["matchedBy"] = json!("textContent");
                if args.include_body {
                    let body = extract_body_from_source(source, def.entry.line_start, def.entry.line_end, args.max_body_lines);
                    obj["body"] = json!(body);
                    obj["bodyStartLine"] = json!(def.entry.line_start);
                }
                text_content_direct.push(obj);
            }
        }
    }

    // Phase 4: Build promoted results from groups
    let mut promoted_results: Vec<Value> = Vec::new();
    // Sort promoted groups by parent line_start for deterministic order
    let mut sorted_groups: Vec<&PromotedGroup> = promoted_groups.values().collect();
    sorted_groups.sort_by_key(|g| xml_defs[g.parent_index].entry.line_start);

    for group in sorted_groups {
        let parent_def = &xml_defs[group.parent_index];
        let mut obj = build_xml_def_json(parent_def, file_path);
        obj["matchedBy"] = json!("textContent");

        if group.matched_children.len() == 1 {
            obj["matchedChild"] = json!(group.matched_children[0].0);
            obj["matchedLine"] = json!(group.matched_children[0].1);
        } else {
            let children: Vec<Value> = group.matched_children.iter()
                .map(|(name, line)| json!({"name": name, "line": line}))
                .collect();
            obj["matchedChildren"] = json!(children);
        }

        if args.include_body {
            let body = extract_body_from_source(source, parent_def.entry.line_start, parent_def.entry.line_end, args.max_body_lines);
            obj["body"] = json!(body);
            obj["bodyStartLine"] = json!(parent_def.entry.line_start);
        }
        promoted_results.push(obj);
    }

    // Combine: name matches first, then promoted textContent, then direct textContent
    let max_results = if args.max_results > 0 { args.max_results } else { 100 };
    let total_results = name_results.len() + promoted_results.len() + text_content_direct.len();
    let defs_json: Vec<Value> = name_results.into_iter()
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
            "onDemand": true,
            "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        }
    });
    ToolCallResult::success(json_to_string(&output))
}

// ─── XML Helper Functions ───────────────────────────────────────────

/// Build a JSON object from a single XmlDefinition.
#[cfg(feature = "lang-xml")]
fn build_xml_def_json(
    def: &crate::definitions::parser_xml::XmlDefinition,
    file_path: &str,
) -> Value {
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

/// Build the parent chain for an XML definition (ancestors up to root).
#[cfg(feature = "lang-xml")]
fn build_parent_chain(
    xml_defs: &[crate::definitions::parser_xml::XmlDefinition],
    target: &crate::definitions::parser_xml::XmlDefinition,
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

/// Extract the body (source lines) for an XML element.
#[cfg(feature = "lang-xml")]
fn extract_body_from_source(
    source: &str,
    line_start: u32,
    line_end: u32,
    max_body_lines: usize,
) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = (line_start as usize).saturating_sub(1);
    let end = (line_end as usize).min(lines.len());
    let body_lines: Vec<String> = lines[start..end].iter().map(|s| s.to_string()).collect();

    if max_body_lines > 0 && body_lines.len() > max_body_lines {
        let mut truncated = body_lines[..max_body_lines].to_vec();
        truncated.push(format!("... ({} more lines)", body_lines.len() - max_body_lines));
        truncated
    } else {
        body_lines
    }
}

/// Extract file extension from a file filter string.
#[cfg(feature = "lang-xml")]
fn extract_file_extension(file_filter: &str) -> Option<String> {
    // Take the last term in comma-separated filter (most specific)
    let last_term = file_filter.split(',').last()?.trim();
    let dot_pos = last_term.rfind('.')?;
    let ext = &last_term[dot_pos + 1..];
    if ext.is_empty() { None } else { Some(ext.to_lowercase()) }
}

/// Resolve an XML file path from the file filter.
/// Supports absolute paths and paths relative to server workspace.
#[cfg(feature = "lang-xml")]
fn resolve_xml_file_path(file_filter: &str, ctx: &HandlerContext) -> Option<String> {
    let path = std::path::Path::new(file_filter);

    // Check absolute path first
    if path.is_absolute() && path.exists() {
        return Some(file_filter.to_string());
    }

    // Try relative to server dir
    let server_dir = ctx.server_dir();
    let full_path = std::path::Path::new(&server_dir).join(file_filter);
    if full_path.exists() {
        return Some(full_path.to_string_lossy().to_string());
    }

    // Try to find matching files in server dir by walking
    // (for substring-style file filters like 'web.config')
    let filter_lower = file_filter.replace('\\', "/").to_lowercase();
    if let Ok(entries) = std::fs::read_dir(&server_dir) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_file() {
                let entry_str = entry_path.to_string_lossy().replace('\\', "/").to_lowercase();
                if entry_str.contains(&filter_lower) {
                    return Some(entry_path.to_string_lossy().to_string());
                }
            }
        }
    }

    // For absolute paths that don't exist yet, still return them
    // (will fail with a clear error message at read time)
    if path.is_absolute() {
        return Some(file_filter.to_string());
    }

    // Last resort: return the filter as-is, prepend server dir
    Some(full_path.to_string_lossy().to_string())
}


// ─── Zero-result hint generation ─────────────────────────────────────

/// Generate contextual hints when xray_definitions returns 0 results.
/// Helps LLMs self-correct common mistakes (wrong kind, typos, wrong tool).
///
/// Hint priority (first matching wins, via `.or_else()` chain):
/// E. Unsupported extension — file filter has an extension not in the definition index
/// A. Wrong `kind` — definitions exist with same name/file but different kind
/// C. File has definitions — file matches but other filters (name/kind/parent) are too narrow
/// F. File fuzzy match — nearest file path when file filter returns 0 results
/// B. Nearest name — typo/wrong name, suggest closest match by edit distance
/// D. Name in content index — name exists as text but not as an AST definition name
/// Hint E: File filter extension not supported by definition index.
/// Must be checked FIRST — most actionable hint for unsupported file types.
fn hint_unsupported_extension(
    args: &DefinitionSearchArgs,
    ctx: &HandlerContext,
) -> Option<String> {
    if ctx.def_extensions.is_empty() {
        return None;
    }
    let ff = args.file_filter.as_ref()?;
    let terms: Vec<&str> = ff.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    for term in &terms {
        if let Some(dot_pos) = term.rfind('.') {
            let ext_lower = term[dot_pos + 1..].to_lowercase();

            // XML extensions are handled by on-demand parsing, not the definition index.
            // If we reach this hint, it means the on-demand handler didn't activate
            // (e.g., no containsLine or name filter was specified). Provide guidance.
            #[cfg(feature = "lang-xml")]
            if crate::definitions::parser_xml::is_xml_extension(&ext_lower) {
                return Some(format!(
                    "XML structural context is available for '.{}' files. \
                     Use xray_definitions file='<path.{}>' containsLine=<N> includeBody=true \
                     or name='<element>' to get XML structural context on-demand.",
                    ext_lower, ext_lower
                ));
            }

            if !ext_lower.is_empty()
                && !ctx.def_extensions.iter().any(|e| e == &ext_lower)
            {
                let def_supported = ctx.def_extensions.iter()
                    .map(|e| format!(".{}", e)).collect::<Vec<_>>().join(", ");

                let content_exts: Vec<&str> = ctx.server_ext.split(',')
                    .map(|s| s.trim()).collect();
                let in_content_index = content_exts.iter()
                    .any(|e| e.eq_ignore_ascii_case(&ext_lower));

                if in_content_index {
                    return Some(format!(
                        "Extension '.{}' is not in the definition index (AST parsing supports: {}). \
                         However, .{} files ARE indexed in the content index. \
                         Use xray_grep terms='<query>' ext='{}' showLines=true for content search.",
                        ext_lower, def_supported, ext_lower, ext_lower
                    ));
                } else {
                    return Some(format!(
                        "Extension '.{}' is not supported by any index \
                         (definition index supports: {}, content index supports: {}). \
                         Use read_file to access this file directly.",
                        ext_lower, def_supported, ctx.server_ext
                    ));
                }
            }
        }
    }
    None
}

/// Hint A: Wrong kind — find what kinds exist for matching name/file.
fn hint_wrong_kind(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) -> Option<String> {
    if args.kind_filter.is_none()
        || (args.name_filter.is_none() && args.file_filter.is_none())
    {
        return None;
    }
    let kind_str = args.kind_filter.as_ref().unwrap();
    let mut kind_counts: HashMap<&str, usize> = HashMap::new();

    if let Some(ref name) = args.name_filter {
        let terms: Vec<String> = name.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        for (n, indices) in &index.name_index {
            if terms.iter().any(|t| n.contains(t.as_str())) {
                for &idx in indices {
                    if let Some(def) = index.definitions.get(idx as usize) {
                        if let Some(ref ff) = args.file_filter {
                            if !file_matches_filter(index, def.file_id, ff) {
                                continue;
                            }
                        }
                        *kind_counts.entry(def.kind.as_str()).or_insert(0) += 1;
                    }
                }
            }
        }
    } else if let Some(ref ff) = args.file_filter {
        for (&file_id, def_indices) in &index.file_index {
            if file_matches_filter(index, file_id, ff) {
                for &idx in def_indices {
                    if let Some(def) = index.definitions.get(idx as usize) {
                        *kind_counts.entry(def.kind.as_str()).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    if kind_counts.is_empty() {
        return None;
    }
    let total: usize = kind_counts.values().sum();
    let mut kinds_sorted: Vec<(&&str, &usize)> = kind_counts.iter().collect();
    kinds_sorted.sort_by(|a, b| b.1.cmp(a.1));
    let kinds_str: Vec<String> = kinds_sorted.iter()
        .map(|(k, c)| format!("{} {}", c, k))
        .collect();
    let top_kind = kinds_sorted[0].0;
    Some(format!(
        "0 results with kind='{}'. Without kind filter: {} definitions found ({}). Did you mean kind='{}'?",
        kind_str, total, kinds_str.join(", "), top_kind
    ))
}

/// Hint C: File has definitions but other filters (name/kind/parent) are too narrow.
fn hint_file_has_defs_but_filters_narrow(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) -> Option<String> {
    let ff = args.file_filter.as_ref()?;
    let mut matching_file_defs = 0usize;
    let mut matching_kinds: HashMap<&str, usize> = HashMap::new();

    for (&file_id, def_indices) in &index.file_index {
        if file_matches_filter(index, file_id, ff) {
            for &idx in def_indices {
                if let Some(def) = index.definitions.get(idx as usize) {
                    matching_file_defs += 1;
                    *matching_kinds.entry(def.kind.as_str()).or_insert(0) += 1;
                }
            }
        }
    }

    if matching_file_defs == 0 {
        return None;
    }
    let mut kinds_sorted: Vec<(&&str, &usize)> = matching_kinds.iter().collect();
    kinds_sorted.sort_by(|a, b| b.1.cmp(a.1));
    let kinds_str: Vec<String> = kinds_sorted.iter()
        .map(|(k, c)| format!("{} {}", c, k))
        .collect();

    let mut extra = String::new();

    // Cross-file hint: check if name matches exist in OTHER files
    if let Some(ref name) = args.name_filter {
        let terms: Vec<String> = name.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let mut cross_file_paths: Vec<String> = Vec::new();
        for (index_name, def_indices) in &index.name_index {
            if terms.iter().any(|t| index_name.contains(t.as_str())) {
                for &def_idx in def_indices {
                    if let Some(def) = index.definitions.get(def_idx as usize) {
                        // Skip definitions that ARE in the file filter (already counted above)
                        if !file_matches_filter(index, def.file_id, ff) {
                            if let Some(path) = index.files.get(def.file_id as usize) {
                                // Extract just the filename for readability
                                let short = path.rsplit(['/', '\\']).next().unwrap_or(path);
                                if !cross_file_paths.contains(&short.to_string()) {
                                    cross_file_paths.push(short.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        cross_file_paths.sort();
        cross_file_paths.truncate(3);

        if !cross_file_paths.is_empty() {
            extra.push_str(&format!(
                " Found '{}' in {} — consider removing file filter or using file='{}'.",
                name, cross_file_paths.join(", "), cross_file_paths[0]
            ));
        } else {
            extra.push_str(" xray_definitions searches AST definition names, not string content. Use xray_grep for content search.");
        }
    }

    Some(format!(
        "File '{}' has {} definitions ({}), but none match your other filters (name/kind/parent).{}",
        ff, matching_file_defs, kinds_str.join(", "), extra
    ))
}

/// Hint F: File fuzzy-match — suggest nearest file path when file filter returns 0 results.
fn hint_file_fuzzy_match(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) -> Option<String> {
    let ff = args.file_filter.as_ref()?;
    let normalize = |s: &str| -> String {
        s.replace('\\', "").replace('/', "").replace('-', "").replace('_', "").to_lowercase()
    };
    let ff_normalized = normalize(ff);
    if ff_normalized.is_empty() {
        return None;
    }

    let mut best_match: Option<(String, usize)> = None;
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (file_id, def_indices) in &index.file_index {
        if let Some(file_path) = index.files.get(*file_id as usize) {
            let path_lower = file_path.replace('\\', "/").to_lowercase();
            let path_normalized = normalize(&path_lower);
            if path_normalized.contains(&ff_normalized) {
                if seen_paths.insert(path_lower.clone()) {
                    let segments: Vec<&str> = path_lower.split('/').collect();
                    for window_size in 1..=segments.len() {
                        for start in 0..=(segments.len() - window_size) {
                            let segment = segments[start..start + window_size].join("/");
                            let seg_normalized = normalize(&segment);
                            if seg_normalized.contains(&ff_normalized) || ff_normalized.contains(&seg_normalized) {
                                let count = def_indices.len();
                                match &best_match {
                                    None => { best_match = Some((segment, count)); }
                                    Some((_, prev_count)) => {
                                        if count > *prev_count {
                                            best_match = Some((segment, count));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let (nearest_path, def_count) = best_match?;
    Some(format!(
        "No definitions found for file='{}'. Nearest match: '{}' ({} definitions). Retry with file='{}'.",
        ff, nearest_path, def_count, nearest_path
    ))
}

/// Hint B: Nearest name match — suggest closest name by edit distance.
fn hint_nearest_name(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
) -> Option<String> {
    if args.use_regex {
        return None;
    }
    let search_name = args.name_filter.as_ref()?.to_lowercase();
    let mut best_match: Option<(String, usize)> = None;
    let mut best_score: f64 = 0.0;

    for (name, indices) in &index.name_index {
        let score = name_similarity(&search_name, name);
        if score > best_score && score > 0.8 {
            best_score = score;
            best_match = Some((name.clone(), indices.len()));
        }
    }

    let (name, count) = best_match?;
    Some(format!(
        "0 results for name='{}'. Nearest match: '{}' ({} definitions, similarity {:.0}%)",
        search_name, name, count, best_score * 100.0
    ))
}

/// Hint D: Name found in content index but not in definitions.
fn hint_name_in_content_not_defs(
    args: &DefinitionSearchArgs,
    ctx: &HandlerContext,
) -> Option<String> {
    let name = args.name_filter.as_ref()?;
    if !ctx.content_ready.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let content_idx = ctx.index.read().ok()?;
    let lower = name.to_lowercase();
    let postings = content_idx.index.get(&lower)?;
    let file_count = postings.len();

    // Phase 4: Count classes and methods in matched files via definition index
    let (class_count, method_count) = if let Some(ref def_arc) = ctx.def_index {
        if let Ok(def_idx) = def_arc.read() {
            let matched_files: std::collections::HashSet<&str> = postings.iter()
                .filter_map(|p| content_idx.files.get(p.file_id as usize))
                .map(|s| s.as_str())
                .collect();
            let mut classes = 0usize;
            let mut methods = 0usize;
            for def in &def_idx.definitions {
                if let Some(file_path) = def_idx.files.get(def.file_id as usize) {
                    if matched_files.contains(file_path.as_str()) {
                        match def.kind {
                            DefinitionKind::Class | DefinitionKind::Interface
                            | DefinitionKind::Struct | DefinitionKind::Record => classes += 1,
                            DefinitionKind::Method | DefinitionKind::Function
                            | DefinitionKind::Constructor => methods += 1,
                            _ => {}
                        }
                    }
                }
            }
            (classes, methods)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let counts_info = if class_count > 0 || method_count > 0 {
        format!(
            " The files contain {} classes and {} methods total. \
             Tip: use parent='<ClassName>' kind='method' to list all methods of a specific class.",
            class_count, method_count
        )
    } else {
        String::new()
    };

    Some(format!(
        "'{}' not found as an AST definition name, but appears in {} files as text content.{} \
         Use xray_grep terms='{}' for content search.",
        name, file_count, counts_info, name
    ))
}

/// Phase 2: Suggest shorter CamelCase fragments when a long name query returns 0 results.
fn hint_suggest_shorter_fragments(
    args: &DefinitionSearchArgs,
) -> Option<String> {
    let name = args.name_filter.as_ref()?;
    if name.len() <= 15 || args.use_regex {
        return None;
    }
    // Split by CamelCase boundaries: find sequences starting with uppercase
    let mut fragments: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !current.is_empty() {
            if current.len() >= 3 {
                fragments.push(current.clone());
            }
            current.clear();
        }
        current.push(ch);
    }
    if current.len() >= 3 {
        fragments.push(current);
    }
    if fragments.len() < 2 {
        return None; // Not enough fragments to be useful
    }
    let fragment_list = fragments.iter()
        .map(|f| format!("name='{}'", f))
        .collect::<Vec<_>>()
        .join(" or ");
    let csv = fragments.join(",");
    Some(format!(
        "No definitions match name='{}'. \
         Try shorter fragments: {}. \
         Or use comma-separated: name='{}' for multi-term OR.",
        name, fragment_list, csv
    ))
}

/// Orchestrator: generate zero-result hints with priority chain.
fn generate_zero_result_hints(
    index: &DefinitionIndex,
    args: &DefinitionSearchArgs,
    summary: &mut Value,
    ctx: &HandlerContext,
) {
    let hint = hint_unsupported_extension(args, ctx)
        .or_else(|| hint_wrong_kind(index, args))
        .or_else(|| hint_file_has_defs_but_filters_narrow(index, args))
        .or_else(|| hint_file_fuzzy_match(index, args))
        .or_else(|| hint_nearest_name(index, args))
        .or_else(|| hint_name_in_content_not_defs(args, ctx))
        .or_else(|| hint_suggest_shorter_fragments(args));

    if let Some(hint_text) = hint {
        summary["hint"] = json!(hint_text);
    }
}

// ─── Auto-summary for broad queries ──────────────────────────────────

/// Container kinds eligible for topDefinitions (classes, interfaces, etc.)
const CONTAINER_KINDS: &[DefinitionKind] = &[
    DefinitionKind::Class,
    DefinitionKind::Interface,
    DefinitionKind::Struct,
    DefinitionKind::Enum,
    DefinitionKind::Record,
];

/// Check whether auto-summary should activate instead of returning truncated entries.
/// Conditions: broad query (no name filter, no body) with more results than maxResults.
fn should_auto_summary(args: &DefinitionSearchArgs, total_results: usize) -> bool {
    args.max_results > 0
        && total_results > args.max_results
        && args.name_filter.is_none()
        && !args.include_body
        && args.sort_by.is_none() // sortBy means user wants ranked individual results, not summary
}

/// Internal accumulator for grouping definitions by directory.
/// Kinds eligible for memberNames in autoSummary (methods, functions)
const MEMBER_KINDS: &[DefinitionKind] = &[
    DefinitionKind::Method,
    DefinitionKind::Function,
    DefinitionKind::Constructor,
];

#[derive(Default)]
struct AutoSummaryGroup {
    counts: HashMap<String, usize>,
    /// (name, line_count) for container kinds — used to pick topDefinitions
    containers: Vec<(String, u32)>,
    /// (parent_name, member_name) for method/function kinds — used for memberNames
    member_names: Vec<(String, String)>,
}

impl AutoSummaryGroup {
    fn add(&mut self, def: &DefinitionEntry) {
        let kind_name = def.kind.as_str().to_string();
        *self.counts.entry(kind_name).or_insert(0) += 1;
        if CONTAINER_KINDS.contains(&def.kind) {
            let line_count = def.line_end.saturating_sub(def.line_start) + 1;
            self.containers.push((def.name.clone(), line_count));
        }
        if MEMBER_KINDS.contains(&def.kind) {
            let parent = def.parent.as_deref().unwrap_or("").to_string();
            self.member_names.push((parent, def.name.clone()));
        }
    }

    fn total(&self) -> usize {
        self.counts.values().sum()
    }

    fn top_definitions(&self, n: usize) -> Vec<String> {
        let mut sorted = self.containers.clone();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(n);
        sorted.into_iter().map(|(name, _)| name).collect()
    }
}

/// Build a directory-grouped summary instead of returning truncated entries.
/// Called when should_auto_summary() returns true.
fn build_auto_summary(
    index: &DefinitionIndex,
    results: &[(u32, &DefinitionEntry)],
    args: &DefinitionSearchArgs,
    total_results: usize,
    search_start: Instant,
    ctx: &HandlerContext,
) -> ToolCallResult {
    let file_filter_base = args.file_filter.as_deref().unwrap_or("");
    let mut groups: HashMap<String, AutoSummaryGroup> = HashMap::new();

    for (_, def) in results {
        let file_path = match index.files.get(def.file_id as usize) {
            Some(p) => p.as_str(),
            None => continue,
        };
        let group_key = extract_group_directory(file_path, file_filter_base);
        groups.entry(group_key).or_default().add(def);
    }

    // Sort groups by total count desc
    let mut sorted_groups: Vec<(String, AutoSummaryGroup)> = groups.into_iter().collect();
    sorted_groups.sort_by(|a, b| b.1.total().cmp(&a.1.total()));

    // Collect all member names across groups, dedup, sort, cap at 50
    let mut all_members: Vec<String> = Vec::new();
    for (_, data) in &sorted_groups {
        for (parent, name) in &data.member_names {
            let formatted = if parent.is_empty() {
                name.clone()
            } else {
                format!("{}.{}", parent, name)
            };
            all_members.push(formatted);
        }
    }
    // Dedup by name (removes overloads)
    all_members.sort();
    all_members.dedup();
    let global_member_cap = 50;
    all_members.truncate(global_member_cap);

    // Format JSON groups
    let groups_json: Vec<Value> = sorted_groups.iter().map(|(dir, data)| {
        let top = data.top_definitions(3);
        let mut group = json!({
            "directory": dir,
            "total": data.total(),
            "counts": data.counts,
            "topDefinitions": top,
        });
        if !all_members.is_empty() {
            group["memberNames"] = json!(all_members);
        }
        group
    }).collect();

    // Build contextual hint
    let hint = if let Some((largest_dir, largest_data)) = sorted_groups.first() {
        let top_name = largest_data.containers.iter()
            .max_by_key(|(_, lc)| *lc)
            .map(|(n, _)| n.as_str())
            .unwrap_or("...");
        format!(
            "Use file='{}' to explore the largest group, or name='{}' for a specific class",
            largest_dir, top_name
        )
    } else {
        "Narrow with file or name filter".to_string()
    };

    let active_definitions: usize = index.file_index.values().map(|v| v.len()).sum();
    let mut summary = json!({
        "totalResults": total_results,
        "returned": 0,
        "autoSummaryMode": true,
        "searchTimeMs": search_start.elapsed().as_secs_f64() * 1000.0,
        "indexFiles": index.files.len(),
        "totalDefinitions": active_definitions,
    });
    inject_branch_warning(&mut summary, ctx);

    let output = json!({
        "autoSummary": {
            "groups": groups_json,
            "totalDefinitions": total_results,
            "groupCount": sorted_groups.len(),
            "hint": hint,
        },
        "summary": summary,
    });

    ToolCallResult::success(json_to_string(&output))
}

/// Extract the group directory from a file path relative to a file_filter base.
///
/// If file_filter="Services/", file_path=".../Services/Auth/UserService.cs" → "Auth"
/// If the file is directly in base: ".../Services/Helpers.cs" → "(root)"
/// Without file_filter: groups by first path component.
fn extract_group_directory(file_path: &str, file_filter_base: &str) -> String {
    let normalized = file_path.replace('\\', "/");

    // Find where the base ends in the file path (case-insensitive)
    let relative = if !file_filter_base.is_empty() {
        let base_norm = file_filter_base.replace('\\', "/").to_lowercase();
        let lower = normalized.to_lowercase();
        if let Some(pos) = lower.find(&base_norm) {
            &normalized[pos + base_norm.len()..]
        } else {
            &normalized
        }
    } else {
        &normalized
    };

    // Strip leading slash
    let relative = relative.strip_prefix('/').unwrap_or(relative);

    // Take first path component as group
    if let Some(sep_pos) = relative.find('/') {
        relative[..sep_pos].to_string()
    } else {
        "(root)".to_string()
    }
}

/// Check if a file (by file_id) matches a comma-separated file filter string.
fn file_matches_filter(index: &DefinitionIndex, file_id: u32, filter: &str) -> bool {
    let path = match index.files.get(file_id as usize) {
        Some(p) => p,
        None => return false,
    };
    let path_lower = path.replace('\\', "/").to_lowercase();
    let terms: Vec<String> = filter.split(',')
        .map(|s| s.trim().replace('\\', "/").to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    terms.iter().any(|t| path_lower.contains(t.as_str()))
}


#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
