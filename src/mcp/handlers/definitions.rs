//! search_definitions handler: AST-based code definition search.
//!
//! The main entry point is [`handle_search_definitions`], which orchestrates:
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

use super::utils::{inject_body_into_obj, inject_branch_warning, best_match_tier, json_to_string};
use super::HandlerContext;

// ─── Parsed arguments struct ─────────────────────────────────────────

/// Parsed and validated arguments for the search_definitions tool.
/// Extracted from raw JSON [`Value`] by [`parse_definition_args`].
#[derive(Debug)]
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
    pub exclude_dir: Vec<String>,
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
        exclude_dir,
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

pub(crate) fn handle_search_definitions(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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

    // 7. Compute term breakdown (before truncation)
    let term_breakdown = compute_term_breakdown(&results, &def_to_term, &parsed);

    // 8. Sort results
    sort_results(&mut results, &index, &parsed);

    // 9. Apply max results
    if parsed.max_results > 0 && results.len() > parsed.max_results {
        results.truncate(parsed.max_results);
    }

    let search_elapsed = search_start.elapsed();

    // 10. Format output
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
        let cross = cross_validate_indexes(index, &ctx.server_dir, &ctx.index_base);
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

    for (file_id, file_path) in index.files.iter().enumerate() {
        if !file_path.replace('\\', "/").to_lowercase().contains(&file_substr) {
            continue;
        }
        if let Some(def_indices) = index.file_index.get(&(file_id as u32)) {
            let mut matching: Vec<&DefinitionEntry> = def_indices.iter()
                .filter_map(|&di| index.definitions.get(di as usize))
                .filter(|d| d.line_start <= line_num && d.line_end >= line_num)
                .collect();

            // Sort by range size (smallest first = most specific)
            matching.sort_by_key(|d| d.line_end - d.line_start);

            for def in &matching {
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
                    inject_body_into_obj(
                        &mut obj, file_path, def.line_start, def.line_end,
                        &mut file_cache, &mut total_body_lines_emitted,
                        args.max_body_lines, args.max_total_body_lines,
                        args.include_doc_comments,
                        args.body_line_start, args.body_line_end,
                    );
                }
                containing_defs.push(obj);
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
        match kind_str.parse::<DefinitionKind>() {
            Ok(kind) => {
                if let Some(indices) = index.kind_index.get(&kind) {
                    candidate_indices = Some(indices.clone());
                } else {
                    candidate_indices = Some(Vec::new());
                }
            }
            Err(e) => {
                return Err(e);
            }
        }
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

            // File filter: comma-separated OR with substring matching
            if let Some(ref ff) = args.file_filter {
                let file_lower = file_path.replace('\\', "/").to_lowercase();
                let file_terms: Vec<String> = ff.split(',')
                    .map(|s| s.trim().replace('\\', "/").to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !file_terms.iter().any(|t| file_lower.contains(t)) {
                    return None;
                }
            }

            // Parent filter: comma-separated OR with substring matching
            if let Some(ref pf) = args.parent_filter {
                let parent_terms: Vec<String> = pf.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                match &def.parent {
                    Some(parent) => {
                        let parent_lower = parent.to_lowercase();
                        if !parent_terms.iter().any(|t| parent_lower.contains(t)) {
                            return None;
                        }
                    }
                    None => return None,
                }
            }

            // Exclude dir
            if args.exclude_dir.iter().any(|excl| {
                file_path.to_lowercase().contains(&excl.to_lowercase())
            }) {
                return None;
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

    // sortBy='lines' works without code_stats
    let needs_code_stats = args.sort_by.as_deref() != Some("lines");

    if needs_code_stats && index.code_stats.is_empty() {
        return Err(
            "Code stats not available for this index. Run search_reindex_definitions to compute metrics.".to_string()
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

    let defs_json: Vec<Value> = results.iter().map(|(def_idx_value, def)| {
        format_definition_entry(
            index, *def_idx_value, def, args,
            &mut file_cache, &mut total_body_lines_emitted,
        )
    }).collect();

    let summary = build_search_summary(
        index, &defs_json, args, total_results,
        stats_info, term_breakdown, total_body_lines_emitted,
        search_elapsed, ctx,
    );

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
    if let Some(ref parent) = def.parent {
        obj["parent"] = json!(parent);
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
    if index.parse_errors > 0 {
        summary["readErrors"] = json!(index.parse_errors);
    }
    if index.lossy_file_count > 0 {
        summary["lossyUtf8Files"] = json!(index.lossy_file_count);
    }
    if args.include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
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
                "reason": "File-list index not found on disk. Run search_reindex or search_fast first."
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

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
