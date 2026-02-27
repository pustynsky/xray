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
    pub max_body_lines: usize,
    pub max_total_body_lines: usize,
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
    let include_body = args.get("includeBody").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_body_lines = args.get("maxBodyLines").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let max_total_body_lines = args.get("maxTotalBodyLines").and_then(|v| v.as_u64()).unwrap_or(500) as usize;
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
        max_body_lines,
        max_total_body_lines,
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

            if let Some(min) = args.min_complexity {
                if stats.cyclomatic_complexity < min { return false; }
            }
            if let Some(min) = args.min_cognitive {
                if stats.cognitive_complexity < min { return false; }
            }
            if let Some(min) = args.min_nesting {
                if stats.max_nesting_depth < min { return false; }
            }
            if let Some(min) = args.min_params {
                if stats.param_count < min { return false; }
            }
            if let Some(min) = args.min_returns {
                if stats.return_count < min { return false; }
            }
            if let Some(min) = args.min_calls {
                if stats.call_count < min { return false; }
            }
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
        );
    }

    // Inject codeStats if requested
    if args.include_code_stats {
        if let Some(stats) = index.code_stats.get(&def_idx) {
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
    }

    obj
}

/// Build the summary JSON object for the search response.
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
    if total_results == 0 {
        if let Some(ref kind_str) = args.kind_filter {
            if kind_str.eq_ignore_ascii_case("property") {
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
        }
    }

    // Hint for large transitive hierarchies
    if args.base_type_transitive && total_results > 5000 {
        if let Some(ref bt) = args.base_type_filter {
            summary["hint"] = json!(format!(
                "Hierarchy of '{}' has {} transitive descendants. Consider adding 'kind' or 'file' filters to narrow results.",
                bt, total_results
            ));
        }
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
mod tests {
    use super::*;
    use crate::definitions::DefinitionKind;

    // ─── kind_priority tests ─────────────────────────────────────────

    #[test]
    fn test_kind_priority_class_returns_0() {
        assert_eq!(kind_priority(&DefinitionKind::Class), 0);
    }

    #[test]
    fn test_kind_priority_interface_returns_0() {
        assert_eq!(kind_priority(&DefinitionKind::Interface), 0);
    }

    #[test]
    fn test_kind_priority_enum_returns_0() {
        assert_eq!(kind_priority(&DefinitionKind::Enum), 0);
    }

    #[test]
    fn test_kind_priority_struct_returns_0() {
        assert_eq!(kind_priority(&DefinitionKind::Struct), 0);
    }

    #[test]
    fn test_kind_priority_record_returns_0() {
        assert_eq!(kind_priority(&DefinitionKind::Record), 0);
    }

    #[test]
    fn test_kind_priority_method_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Method), 1);
    }

    #[test]
    fn test_kind_priority_function_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Function), 1);
    }

    #[test]
    fn test_kind_priority_property_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Property), 1);
    }

    #[test]
    fn test_kind_priority_field_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Field), 1);
    }

    #[test]
    fn test_kind_priority_constructor_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Constructor), 1);
    }

    #[test]
    fn test_kind_priority_delegate_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Delegate), 1);
    }

    #[test]
    fn test_kind_priority_event_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Event), 1);
    }

    #[test]
    fn test_kind_priority_enum_member_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::EnumMember), 1);
    }

    #[test]
    fn test_kind_priority_type_alias_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::TypeAlias), 1);
    }

    #[test]
    fn test_kind_priority_variable_returns_1() {
        assert_eq!(kind_priority(&DefinitionKind::Variable), 1);
    }

    #[test]
    fn test_kind_priority_type_level_before_members() {
        // Verify that type-level definitions sort before member-level definitions
        assert!(kind_priority(&DefinitionKind::Class) < kind_priority(&DefinitionKind::Method));
        assert!(kind_priority(&DefinitionKind::Interface) < kind_priority(&DefinitionKind::Property));
        assert!(kind_priority(&DefinitionKind::Enum) < kind_priority(&DefinitionKind::Field));
        assert!(kind_priority(&DefinitionKind::Struct) < kind_priority(&DefinitionKind::Function));
        assert!(kind_priority(&DefinitionKind::Record) < kind_priority(&DefinitionKind::Constructor));
    }

    // ─── Parent relevance ranking tests ──────────────────────────────

    /// Helper to create a DefinitionEntry with specific name, parent, and kind
    fn make_def(name: &str, parent: Option<&str>, kind: DefinitionKind) -> DefinitionEntry {
        DefinitionEntry {
            name: name.to_string(),
            kind,
            file_id: 0,
            line_start: 1,
            line_end: 10,
            signature: None,
            parent: parent.map(|s| s.to_string()),
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        }
    }

    #[test]
    fn test_parent_ranking_exact_parent_before_substring_parent() {
        let parent_terms = vec!["userservice".to_string()];
        let _name_terms: Vec<String> = vec![];

        let def_exact = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
        let def_substring = make_def("GetUser", Some("UserServiceMock"), DefinitionKind::Method);

        let tier_exact = def_exact.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let tier_substring = def_substring.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        assert!(tier_exact < tier_substring,
            "Exact parent tier {} should be less than substring parent tier {}",
            tier_exact, tier_substring);
        assert_eq!(tier_exact, 0, "Exact parent match should be tier 0");
    }

    #[test]
    fn test_parent_ranking_prefix_parent_before_contains_parent() {
        let parent_terms = vec!["userservice".to_string()];

        let def_prefix = make_def("Create", Some("UserServiceFactory"), DefinitionKind::Method);
        let def_contains = make_def("Validate", Some("IUserService"), DefinitionKind::Method);

        let tier_prefix = def_prefix.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let tier_contains = def_contains.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        assert!(tier_prefix < tier_contains,
            "Prefix parent tier {} should be less than contains parent tier {}",
            tier_prefix, tier_contains);
        assert_eq!(tier_prefix, 1, "Prefix parent match should be tier 1");
        assert_eq!(tier_contains, 2, "Contains parent match should be tier 2");
    }

    #[test]
    fn test_parent_ranking_takes_precedence_over_name_ranking() {
        let parent_terms = vec!["userservice".to_string()];
        let name_terms = vec!["getuser".to_string()];

        let def_a = make_def("GetUser", Some("MockUserServiceWrapper"), DefinitionKind::Method);
        let def_b = make_def("FetchData", Some("UserService"), DefinitionKind::Method);

        let parent_tier_a = def_a.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let parent_tier_b = def_b.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        let name_tier_a = best_match_tier(&def_a.name, &name_terms);
        let name_tier_b = best_match_tier(&def_b.name, &name_terms);

        assert!(name_tier_a < name_tier_b, "def_a should have better name tier");
        assert!(parent_tier_b < parent_tier_a, "def_b should have better parent tier");

        let cmp = parent_tier_a.cmp(&parent_tier_b)
            .then_with(|| name_tier_a.cmp(&name_tier_b));
        assert_eq!(cmp, std::cmp::Ordering::Greater,
            "def_a should sort AFTER def_b because parent tier is primary");
    }

    #[test]
    fn test_parent_ranking_no_parent_sorts_last() {
        let parent_terms = vec!["userservice".to_string()];

        let def_with_parent = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
        let def_no_parent = make_def("GetUser", None, DefinitionKind::Method);

        let tier_with = def_with_parent.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let tier_without = def_no_parent.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        assert_eq!(tier_with, 0, "Exact parent should be tier 0");
        assert_eq!(tier_without, 3, "No parent should be tier 3 (worst)");
        assert!(tier_with < tier_without);
    }

    #[test]
    fn test_parent_ranking_only_active_with_parent_filter() {
        let parent_terms: Vec<String> = vec![];

        let def_a = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
        let def_b = make_def("FetchData", Some("OrderService"), DefinitionKind::Method);

        let tier_a = if !parent_terms.is_empty() {
            def_a.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
        } else { 0 };
        let tier_b = if !parent_terms.is_empty() {
            def_b.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
        } else { 0 };

        assert_eq!(tier_a, 0);
        assert_eq!(tier_b, 0);
        assert_eq!(tier_a.cmp(&tier_b), std::cmp::Ordering::Equal,
            "Without parent filter, parent tier should be equal for all");
    }

    // ─── Comma-separated file filter tests ───────────────────────────

    #[test]
    fn test_file_filter_comma_separated_matches_multiple_files() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "file": "ResilientClient.cs,ProxyClient.cs",
            "kind": "method"
        }));
        assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 2, "expected >= 2 methods from two files, got {}", defs.len());
        let files: Vec<&str> = defs.iter()
            .map(|d| d["file"].as_str().unwrap())
            .collect();
        assert!(files.iter().any(|f| f.contains("ResilientClient")),
            "should include ResilientClient");
        assert!(files.iter().any(|f| f.contains("ProxyClient")),
            "should include ProxyClient");
    }

    #[test]
    fn test_file_filter_single_value_still_works() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "file": "QueryService.cs",
            "kind": "method"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 3, "expected >= 3 methods in QueryService, got {}", defs.len());
        for d in defs {
            assert!(d["file"].as_str().unwrap().contains("QueryService"),
                "all results should be from QueryService");
        }
    }

    #[test]
    fn test_file_filter_comma_separated_no_match_returns_empty() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "file": "NonExistent.cs,AlsoMissing.cs"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 0, "no files match, should return 0 results");
    }

    // ─── Comma-separated parent filter tests ─────────────────────────

    #[test]
    fn test_parent_filter_comma_separated_matches_multiple_classes() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "parent": "ResilientClient,ProxyClient",
            "kind": "method"
        }));
        assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 2, "expected >= 2 methods from two classes, got {}", defs.len());
        let parents: Vec<&str> = defs.iter()
            .map(|d| d["parent"].as_str().unwrap())
            .collect();
        assert!(parents.iter().any(|p| *p == "ResilientClient"),
            "should include ResilientClient methods");
        assert!(parents.iter().any(|p| *p == "ProxyClient"),
            "should include ProxyClient methods");
    }

    #[test]
    fn test_parent_filter_single_value_still_works() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "parent": "QueryService",
            "kind": "method"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 3, "expected >= 3 methods in QueryService, got {}", defs.len());
        for d in defs {
            assert_eq!(d["parent"].as_str().unwrap(), "QueryService",
                "all results should have parent QueryService");
        }
    }

    #[test]
    fn test_parent_filter_comma_separated_no_match_returns_empty() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "parent": "NonExistentClass,AlsoMissing"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 0, "no parents match, should return 0 results");
    }

    // ─── crossValidate audit tests ────────────────────────────────────

    #[test]
    fn test_audit_cross_validate_no_file_index_returns_skipped() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "audit": true,
            "crossValidate": true
        }));
        assert!(!result.is_error, "audit+crossValidate should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["crossValidation"].is_object(), "Should have crossValidation object");
        assert_eq!(v["crossValidation"]["status"], "skipped",
            "Should be 'skipped' when file-list index not found");
    }

    #[test]
    fn test_audit_without_cross_validate_has_no_cross_validation() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "audit": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v.get("crossValidation").is_none(),
            "Without crossValidate=true, should NOT have crossValidation in output");
    }

    #[test]
    fn test_audit_cross_validate_with_file_index() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        { let mut f = std::fs::File::create(project_dir.join("FileA.cs")).unwrap();
          writeln!(f, "class FileA {{ }}").unwrap(); }
        { let mut f = std::fs::File::create(project_dir.join("FileB.cs")).unwrap();
          writeln!(f, "class FileB {{ }}").unwrap(); }

        let project_str = crate::clean_path(&project_dir.to_string_lossy());
        let idx_base = tmp.path().join("indexes");
        std::fs::create_dir_all(&idx_base).unwrap();

        let file_index = crate::build_index(&crate::IndexArgs {
            dir: project_str.clone(),
            max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
        });
        crate::save_index(&file_index, &idx_base).unwrap();

        let def_index = crate::definitions::build_definition_index(
            &crate::definitions::DefIndexArgs {
                dir: project_str.clone(),
                ext: "cs".to_string(),
                threads: 1,
            }
        );

        let content_index = crate::ContentIndex {
            root: project_str.clone(),
            extensions: vec!["cs".to_string()],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
            server_dir: project_str,
            index_base: idx_base,
            ..Default::default()
        };

        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "audit": true,
            "crossValidate": true
        }));
        assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(v["crossValidation"]["status"], "ok",
            "Cross-validation should succeed when file-list index exists");
        assert!(v["crossValidation"]["fileListFiles"].as_u64().unwrap() > 0,
            "Should report file-list file count");
        assert!(v["crossValidation"]["defIndexFiles"].as_u64().unwrap() > 0,
            "Should report def index file count");
    }

    // ─── baseTypeTransitive tests ─────────────────────────────────────

    /// Helper to create a context with a 3-level inheritance chain:
    /// BaseService → MiddleService → ConcreteService
    fn make_transitive_inheritance_ctx() -> HandlerContext {
        use crate::definitions::*;

        let definitions = vec![
            DefinitionEntry {
                name: "BaseService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                name: "MiddleService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["BaseService".to_string()],
            },
            DefinitionEntry {
                name: "ConcreteService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 1, line_start: 1, line_end: 80,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["MiddleService".to_string()],
            },
            DefinitionEntry {
                name: "UnrelatedService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 1, line_start: 82, line_end: 120,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["SomethingElse".to_string()],
            },
        ];

        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
            for bt in &def.base_types {
                base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
            }
        }

        let def_index = DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec![
                "C:\\src\\Services.cs".to_string(),
                "C:\\src\\Concrete.cs".to_string(),
            ],
            definitions, name_index, kind_index,
            attribute_index: HashMap::new(), base_type_index,
            file_index,
            path_to_id: HashMap::new(),
            method_calls: HashMap::new(),
            ..Default::default()
        };

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            extensions: vec!["cs".to_string()],
            ..Default::default()
        };

        super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
            ..Default::default()
        }
    }

    #[test]
    fn test_base_type_transitive_finds_indirect_descendants() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BaseService",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"MiddleService"), "Should find MiddleService (direct child)");
        assert!(names.contains(&"ConcreteService"), "Should find ConcreteService (grandchild via transitive BFS)");
        assert!(!names.contains(&"UnrelatedService"), "Should NOT find UnrelatedService");
        assert!(!names.contains(&"BaseService"), "Should NOT find BaseService itself (it doesn't inherit from itself)");
    }

    #[test]
    fn test_base_type_non_transitive_finds_only_direct() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BaseService"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"MiddleService"), "Should find MiddleService (direct child)");
        assert!(!names.contains(&"ConcreteService"), "Should NOT find ConcreteService (indirect, transitive=false)");
    }

    #[test]
    fn test_base_type_transitive_no_match_returns_empty() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "NonExistentType",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.is_empty(), "Non-existent base type should return 0 results");
    }

    #[test]
    fn test_base_type_empty_string_treated_as_no_filter() {
        let ctx = make_transitive_inheritance_ctx();
        let result_empty = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": ""
        }));
        assert!(!result_empty.is_error);
        let v_empty: serde_json::Value = serde_json::from_str(&result_empty.content[0].text).unwrap();
        let defs_empty = v_empty["definitions"].as_array().unwrap();

        let result_no_filter = handle_search_definitions(&ctx, &serde_json::json!({}));
        let v_no_filter: serde_json::Value = serde_json::from_str(&result_no_filter.content[0].text).unwrap();
        let defs_no_filter = v_no_filter["definitions"].as_array().unwrap();

        assert_eq!(defs_empty.len(), defs_no_filter.len(),
            "baseType='' should return same results as no baseType filter. Got {} vs {}",
            defs_empty.len(), defs_no_filter.len());
    }

    #[test]
    fn test_base_type_substring_matches_generic_interface() {
        use crate::definitions::*;

        let definitions = vec![
            DefinitionEntry {
                name: "GenericImpl".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Model>".to_string()],
            },
            DefinitionEntry {
                name: "AnotherImpl".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Report>".to_string()],
            },
        ];

        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
            for bt in &def.base_types {
                base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
            }
        }

        let def_index = DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec!["C:\\src\\Impls.cs".to_string()],
            definitions, name_index, kind_index,
            attribute_index: HashMap::new(), base_type_index,
            file_index, path_to_id: HashMap::new(),
            method_calls: HashMap::new(), ..Default::default()
        };

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            extensions: vec!["cs".to_string()],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
            ..Default::default()
        };

        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "IRepository"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 2, "baseType='IRepository' should find both IRepository<Model> and IRepository<Report> via substring. Got: {:?}",
            defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>());

        let result2 = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "IRepository<Model>"
        }));
        assert!(!result2.is_error);
        let v2: serde_json::Value = serde_json::from_str(&result2.content[0].text).unwrap();
        let defs2 = v2["definitions"].as_array().unwrap();
        assert_eq!(defs2.len(), 1, "baseType='IRepository<Model>' should find only GenericImpl via exact match");
        assert_eq!(defs2[0]["name"], "GenericImpl");
    }

    #[test]
    fn test_base_type_transitive_case_insensitive() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BASESERVICE",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 2, "Case-insensitive transitive should find both descendants");
    }

    // ─── B-1 BFS cascade prevention test ──────────────────────────────

    #[test]
    fn test_base_type_transitive_no_cascade_with_dangerous_names() {
        use crate::definitions::*;

        let definitions = vec![
            DefinitionEntry {
                name: "BaseBlock".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                name: "Service".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["BaseBlock".to_string()],
            },
            DefinitionEntry {
                name: "UnrelatedA".to_string(), kind: DefinitionKind::Class,
                file_id: 1, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IService".to_string()],
            },
            DefinitionEntry {
                name: "UnrelatedB".to_string(), kind: DefinitionKind::Class,
                file_id: 1, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["WebServiceBase".to_string()],
            },
        ];

        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
            for bt in &def.base_types {
                base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
            }
        }

        let def_index = DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec![
                "C:\\src\\Blocks.cs".to_string(),
                "C:\\src\\Services.cs".to_string(),
            ],
            definitions, name_index, kind_index,
            attribute_index: HashMap::new(), base_type_index,
            file_index, path_to_id: HashMap::new(),
            method_calls: HashMap::new(), ..Default::default()
        };

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            extensions: vec!["cs".to_string()],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
            ..Default::default()
        };

        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BaseBlock",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error, "Should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

        assert!(names.contains(&"Service"),
            "Should find Service (direct descendant). Got: {:?}", names);
        assert!(!names.contains(&"UnrelatedA"),
            "Should NOT find UnrelatedA (unrelated, inherits IService not BaseBlock). Got: {:?}", names);
        assert!(!names.contains(&"UnrelatedB"),
            "Should NOT find UnrelatedB (unrelated, inherits WebServiceBase not BaseBlock). Got: {:?}", names);
    }

    #[test]
    fn test_base_type_transitive_generics_still_work_at_seed_level() {
        use crate::definitions::*;

        let definitions = vec![
            DefinitionEntry {
                name: "GenericImpl".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Model>".to_string()],
            },
            DefinitionEntry {
                name: "AnotherImpl".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Report>".to_string()],
            },
            DefinitionEntry {
                name: "SubImpl".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 102, line_end: 150,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["GenericImpl".to_string()],
            },
        ];

        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
            for bt in &def.base_types {
                base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
            }
        }

        let def_index = DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec!["C:\\src\\Impls.cs".to_string()],
            definitions, name_index, kind_index,
            attribute_index: HashMap::new(), base_type_index,
            file_index, path_to_id: HashMap::new(),
            method_calls: HashMap::new(), ..Default::default()
        };

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            extensions: vec!["cs".to_string()],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(def_index))),
            ..Default::default()
        };

        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "IRepository",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

        assert!(names.contains(&"GenericImpl"),
            "Should find GenericImpl (IRepository<Model> matched via seed substring). Got: {:?}", names);
        assert!(names.contains(&"AnotherImpl"),
            "Should find AnotherImpl (IRepository<Report> matched via seed substring). Got: {:?}", names);
        assert!(names.contains(&"SubImpl"),
            "Should find SubImpl (inherits GenericImpl, found via level 1 exact match). Got: {:?}", names);
    }

    // ─── F-2 Hint for large transitive hierarchy test ─────────────────

    #[test]
    fn test_base_type_transitive_hint_for_large_hierarchy() {
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BaseService",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["summary"].get("hint").is_none(),
            "No hint expected for small result set (< 5000)");
    }

    #[test]
    fn test_parent_filter_comma_with_spaces_trimmed() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "parent": " ResilientClient , ProxyClient ",
            "kind": "method"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert!(defs.len() >= 2, "spaces should be trimmed, still match both classes");
    }

    // ─── termBreakdown tests ──────────────────────────────────────────

    #[test]
    fn test_term_breakdown_multi_term_shows_per_term_counts() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "name": "QueryService,ResilientClient"
        }));
        assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let summary = &v["summary"];
        assert!(summary.get("termBreakdown").is_some(),
            "Multi-term name query should have termBreakdown in summary");
        let breakdown = summary["termBreakdown"].as_object().unwrap();
        assert!(breakdown.contains_key("queryservice"),
            "termBreakdown should have key for 'queryservice'");
        assert!(breakdown.contains_key("resilientclient"),
            "termBreakdown should have key for 'resilientclient'");
        assert!(breakdown["queryservice"].as_u64().unwrap() > 0,
            "queryservice should have results");
        assert!(breakdown["resilientclient"].as_u64().unwrap() > 0,
            "resilientclient should have results");
    }

    #[test]
    fn test_term_breakdown_single_term_not_present() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "name": "QueryService"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["summary"].get("termBreakdown").is_none(),
            "Single-term query should NOT have termBreakdown");
    }

    #[test]
    fn test_term_breakdown_regex_not_present() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "name": "Query.*",
            "regex": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["summary"].get("termBreakdown").is_none(),
            "Regex query should NOT have termBreakdown");
    }

    #[test]
    fn test_term_breakdown_no_name_filter_not_present() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "kind": "class"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["summary"].get("termBreakdown").is_none(),
            "Query without name filter should NOT have termBreakdown");
    }

    #[test]
    fn test_term_breakdown_with_zero_match_term() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "name": "QueryService,NonExistentXyzZzz"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let breakdown = v["summary"]["termBreakdown"].as_object().unwrap();
        assert!(breakdown["queryservice"].as_u64().unwrap() > 0,
            "queryservice should have results");
        assert_eq!(breakdown["nonexistentxyzzzz"].as_u64().unwrap(), 0,
            "nonexistent term should have 0 results");
    }

    #[test]
    fn test_term_breakdown_counts_are_pre_truncation() {
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "name": "QueryService,ResilientClient",
            "maxResults": 1
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let breakdown = v["summary"]["termBreakdown"].as_object().unwrap();
        let total_breakdown: u64 = breakdown.values()
            .filter_map(|v| v.as_u64())
            .sum();
        let total_results = v["summary"]["totalResults"].as_u64().unwrap();
        assert_eq!(total_breakdown, total_results,
            "Sum of termBreakdown counts ({}) should equal totalResults ({})",
            total_breakdown, total_results);
        let returned = v["summary"]["returned"].as_u64().unwrap();
        assert!(returned <= 1, "returned should be <= maxResults=1, got {}", returned);
    }

    // ═══════════════════════════════════════════════════════════════════
    // NEW TESTS — for extracted functions
    // ═══════════════════════════════════════════════════════════════════

    // ─── parse_definition_args tests ─────────────────────────────────

    #[test]
    fn test_parse_args_empty_returns_defaults() {
        let args = json!({});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.name_filter.is_none());
        assert!(parsed.kind_filter.is_none());
        assert!(parsed.file_filter.is_none());
        assert!(parsed.parent_filter.is_none());
        assert!(parsed.contains_line.is_none());
        assert!(!parsed.use_regex);
        assert_eq!(parsed.max_results, 100);
        assert!(parsed.exclude_dir.is_empty());
        assert!(!parsed.include_body);
        assert_eq!(parsed.max_body_lines, 100);
        assert_eq!(parsed.max_total_body_lines, 500);
        assert!(!parsed.audit);
        assert!(!parsed.include_code_stats);
        assert!(parsed.sort_by.is_none());
        assert!(!parsed.has_stats_filter());
    }

    #[test]
    fn test_parse_args_name_filter_empty_string_is_none() {
        let args = json!({"name": ""});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.name_filter.is_none(), "empty name should be treated as None");
    }

    #[test]
    fn test_parse_args_name_filter_non_empty() {
        let args = json!({"name": "UserService"});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.name_filter, Some("UserService".to_string()));
    }

    #[test]
    fn test_parse_args_base_type_empty_string_is_none() {
        let args = json!({"baseType": ""});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.base_type_filter.is_none(), "empty baseType should be treated as None");
    }

    #[test]
    fn test_parse_args_contains_line_zero_rejected() {
        let args = json!({"containsLine": 0});
        let result = parse_definition_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be >= 1"));
    }

    #[test]
    fn test_parse_args_contains_line_negative_rejected() {
        let args = json!({"containsLine": -5});
        let result = parse_definition_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be >= 1"));
    }

    #[test]
    fn test_parse_args_contains_line_valid() {
        let args = json!({"containsLine": 42});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.contains_line, Some(42));
    }

    #[test]
    fn test_parse_args_sort_by_valid_values() {
        for field in &["cyclomaticComplexity", "cognitiveComplexity", "maxNestingDepth",
                       "paramCount", "returnCount", "callCount", "lambdaCount", "lines"] {
            let args = json!({"sortBy": field});
            let parsed = parse_definition_args(&args);
            assert!(parsed.is_ok(), "sortBy='{}' should be valid", field);
            assert_eq!(parsed.unwrap().sort_by, Some(field.to_string()));
        }
    }

    #[test]
    fn test_parse_args_sort_by_invalid_rejected() {
        let args = json!({"sortBy": "invalidField"});
        let result = parse_definition_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid sortBy"));
    }

    #[test]
    fn test_parse_args_has_stats_filter_with_min() {
        let args = json!({"minComplexity": 10});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.has_stats_filter());
        assert!(parsed.include_code_stats, "min* implies includeCodeStats");
    }

    #[test]
    fn test_parse_args_has_stats_filter_with_sort_by() {
        let args = json!({"sortBy": "lines"});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.has_stats_filter());
        assert!(parsed.include_code_stats);
    }

    #[test]
    fn test_parse_args_include_code_stats_explicit() {
        let args = json!({"includeCodeStats": true});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.include_code_stats);
        assert!(!parsed.has_stats_filter(), "explicit includeCodeStats doesn't set has_stats_filter");
    }

    #[test]
    fn test_parse_args_exclude_dir() {
        let args = json!({"excludeDir": ["node_modules", "bin"]});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.exclude_dir, vec!["node_modules".to_string(), "bin".to_string()]);
    }

    #[test]
    fn test_parse_args_all_code_stats_filters() {
        let args = json!({
            "minComplexity": 5,
            "minCognitive": 10,
            "minNesting": 3,
            "minParams": 4,
            "minReturns": 2,
            "minCalls": 8
        });
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.min_complexity, Some(5u16));
        assert_eq!(parsed.min_cognitive, Some(10u16));
        assert_eq!(parsed.min_nesting, Some(3u8));
        assert_eq!(parsed.min_params, Some(4u8));
        assert_eq!(parsed.min_returns, Some(2u8));
        assert_eq!(parsed.min_calls, Some(8u16));
        assert!(parsed.has_stats_filter());
    }

    // ─── collect_candidates tests ────────────────────────────────────

    /// Helper to create a DefinitionIndex for collect_candidates tests.
    fn make_test_def_index() -> DefinitionIndex {
        let definitions = vec![
            DefinitionEntry {
                name: "UserService".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec!["injectable".to_string()], base_types: vec![],
            },
            DefinitionEntry {
                name: "GetUser".to_string(), kind: DefinitionKind::Method,
                file_id: 0, line_start: 10, line_end: 30,
                signature: None, parent: Some("UserService".to_string()), modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                name: "OrderService".to_string(), kind: DefinitionKind::Class,
                file_id: 1, line_start: 1, line_end: 80,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                name: "GetOrder".to_string(), kind: DefinitionKind::Method,
                file_id: 1, line_start: 10, line_end: 25,
                signature: None, parent: Some("OrderService".to_string()), modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
        ];

        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut attribute_index: HashMap<String, Vec<u32>> = HashMap::new();

        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
            for attr in &def.attributes {
                attribute_index.entry(attr.to_lowercase()).or_default().push(idx);
            }
        }

        DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec![
                "C:\\src\\UserService.cs".to_string(),
                "C:\\src\\OrderService.cs".to_string(),
            ],
            definitions, name_index, kind_index,
            attribute_index, base_type_index: HashMap::new(),
            file_index, path_to_id: HashMap::new(),
            method_calls: HashMap::new(), ..Default::default()
        }
    }

    #[test]
    fn test_collect_candidates_no_filters_returns_all() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 4, "No filters → all 4 definitions");
    }

    #[test]
    fn test_collect_candidates_kind_filter() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"kind": "class"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 2, "kind=class → 2 classes");
        for &idx in &candidates {
            assert_eq!(index.definitions[idx as usize].kind, DefinitionKind::Class);
        }
    }

    #[test]
    fn test_collect_candidates_name_substring() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"name": "Service"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 2, "name=Service → UserService + OrderService");
    }

    #[test]
    fn test_collect_candidates_name_multi_term() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"name": "UserService,GetOrder"})).unwrap();
        let (candidates, def_to_term) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 2, "name=UserService,GetOrder → 2 matches");
        // Check term mapping
        assert!(!def_to_term.is_empty(), "def_to_term should be populated for multi-term");
    }

    #[test]
    fn test_collect_candidates_kind_and_name_intersection() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"kind": "method", "name": "GetUser"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 1, "kind=method + name=GetUser → 1 match");
        assert_eq!(index.definitions[candidates[0] as usize].name, "GetUser");
    }

    #[test]
    fn test_collect_candidates_invalid_kind_returns_error() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"kind": "nonexistent"})).unwrap();
        let result = collect_candidates(&index, &args);
        assert!(result.is_err(), "Invalid kind should return error");
    }

    #[test]
    fn test_collect_candidates_attribute_filter() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"attribute": "Injectable"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 1, "attribute=Injectable → 1 match");
        assert_eq!(index.definitions[candidates[0] as usize].name, "UserService");
    }

    #[test]
    fn test_collect_candidates_regex_name() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"name": "Get.*", "regex": true})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert_eq!(candidates.len(), 2, "regex Get.* → GetUser + GetOrder");
    }

    // ─── apply_entry_filters tests ───────────────────────────────────

    #[test]
    fn test_apply_entry_filters_file_filter() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        let args = parse_definition_args(&json!({"file": "UserService.cs"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 2, "file=UserService.cs → 2 defs in that file");
        for (_, def) in &results {
            assert_eq!(def.file_id, 0);
        }
    }

    #[test]
    fn test_apply_entry_filters_parent_filter() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        let args = parse_definition_args(&json!({"parent": "UserService"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 1, "parent=UserService → 1 method");
        assert_eq!(results[0].1.name, "GetUser");
    }

    #[test]
    fn test_apply_entry_filters_exclude_dir() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        let args = parse_definition_args(&json!({"excludeDir": ["OrderService"]})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        // OrderService.cs is excluded → only UserService.cs defs remain
        assert_eq!(results.len(), 2, "excludeDir OrderService → 2 defs from UserService.cs");
    }

    #[test]
    fn test_apply_entry_filters_comma_separated_file() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        let args = parse_definition_args(&json!({"file": "UserService.cs,OrderService.cs"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 4, "both files → all 4 defs");
    }

    #[test]
    fn test_apply_entry_filters_parent_no_match_returns_empty() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        let args = parse_definition_args(&json!({"parent": "NonExistentClass"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 0);
    }

    // ─── apply_stats_filters tests ───────────────────────────────────

    #[test]
    fn test_apply_stats_filters_no_filter_passthrough() {
        let index = make_test_def_index();
        let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let mut results = all_defs;
        let args = parse_definition_args(&json!({})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert!(!info.applied, "No stats filter → not applied");
        assert_eq!(results.len(), 4, "All 4 should remain");
    }

    #[test]
    fn test_apply_stats_filters_error_when_no_stats() {
        let index = make_test_def_index(); // no code_stats populated
        let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let mut results = all_defs;
        let args = parse_definition_args(&json!({"minComplexity": 5})).unwrap();
        let result = apply_stats_filters(&index, &mut results, &args);
        assert!(result.is_err(), "Should error when code_stats is empty");
        assert!(result.unwrap_err().contains("Code stats not available"));
    }

    #[test]
    fn test_apply_stats_filters_sort_by_lines_no_stats_needed() {
        let index = make_test_def_index(); // no code_stats — but sortBy=lines doesn't need them
        let all_defs: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let mut results = all_defs;
        let args = parse_definition_args(&json!({"sortBy": "lines"})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert!(!info.applied, "sortBy=lines doesn't filter, just sorts");
        assert_eq!(results.len(), 4, "All 4 should remain");
    }

    // ─── compute_term_breakdown tests ────────────────────────────────

    #[test]
    fn test_compute_term_breakdown_single_term_returns_none() {
        let results: Vec<(u32, &DefinitionEntry)> = vec![];
        let def_to_term = HashMap::new();
        let args = parse_definition_args(&json!({"name": "UserService"})).unwrap();
        let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
        assert!(breakdown.is_none(), "Single term → no breakdown");
    }

    #[test]
    fn test_compute_term_breakdown_no_name_returns_none() {
        let results: Vec<(u32, &DefinitionEntry)> = vec![];
        let def_to_term = HashMap::new();
        let args = parse_definition_args(&json!({})).unwrap();
        let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
        assert!(breakdown.is_none(), "No name filter → no breakdown");
    }

    #[test]
    fn test_compute_term_breakdown_regex_returns_none() {
        let results: Vec<(u32, &DefinitionEntry)> = vec![];
        let def_to_term = HashMap::new();
        let args = parse_definition_args(&json!({"name": "Get.*", "regex": true})).unwrap();
        let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
        assert!(breakdown.is_none(), "Regex → no breakdown");
    }

    #[test]
    fn test_compute_term_breakdown_multi_term() {
        let index = make_test_def_index();
        let mut def_to_term: HashMap<u32, usize> = HashMap::new();
        def_to_term.insert(0, 0); // UserService → term 0
        def_to_term.insert(3, 1); // GetOrder → term 1

        let results: Vec<(u32, &DefinitionEntry)> = vec![
            (0, &index.definitions[0]),
            (3, &index.definitions[3]),
        ];
        let args = parse_definition_args(&json!({"name": "UserService,GetOrder"})).unwrap();
        let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
        assert!(breakdown.is_some(), "Multi-term → should have breakdown");
        let bd = breakdown.unwrap();
        assert_eq!(bd["userservice"], 1);
        assert_eq!(bd["getorder"], 1);
    }

    // ─── sort_results tests ──────────────────────────────────────────

    #[test]
    fn test_sort_results_by_lines_descending() {
        let index = make_test_def_index();
        let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let args = parse_definition_args(&json!({"sortBy": "lines"})).unwrap();
        sort_results(&mut results, &index, &args);
        // Should be sorted by line count descending
        let line_counts: Vec<u32> = results.iter()
            .map(|(_, d)| d.line_end - d.line_start + 1)
            .collect();
        for i in 0..line_counts.len() - 1 {
            assert!(line_counts[i] >= line_counts[i + 1],
                "Should be descending: {} >= {}", line_counts[i], line_counts[i + 1]);
        }
    }

    #[test]
    fn test_sort_results_relevance_exact_before_prefix() {
        let index = make_test_def_index();
        // Search for "userservice" — UserService (exact) should come before GetUser (no match)
        let mut results: Vec<(u32, &DefinitionEntry)> = vec![
            (1, &index.definitions[1]), // GetUser
            (0, &index.definitions[0]), // UserService
        ];
        let args = parse_definition_args(&json!({"name": "userservice"})).unwrap();
        sort_results(&mut results, &index, &args);
        assert_eq!(results[0].1.name, "UserService", "Exact match should sort first");
    }

    #[test]
    fn test_sort_results_no_filter_no_sort() {
        let index = make_test_def_index();
        let original_order: Vec<u32> = (0..4).collect();
        let mut results: Vec<(u32, &DefinitionEntry)> = original_order.iter()
            .map(|&i| (i, &index.definitions[i as usize]))
            .collect();
        let args = parse_definition_args(&json!({})).unwrap();
        sort_results(&mut results, &index, &args);
        // Without name/parent filter, no sorting happens — order preserved
        let result_indices: Vec<u32> = results.iter().map(|(i, _)| *i).collect();
        assert_eq!(result_indices, original_order, "No filter → original order preserved");
    }

    // ─── get_sort_value tests ────────────────────────────────────────

    #[test]
    fn test_get_sort_value_lines() {
        let def = make_def("Test", None, DefinitionKind::Method);
        // line_start=1, line_end=10 → 10 lines
        assert_eq!(get_sort_value(None, &def, "lines"), 10);
    }

    #[test]
    fn test_get_sort_value_no_stats_returns_zero() {
        let def = make_def("Test", None, DefinitionKind::Method);
        assert_eq!(get_sort_value(None, &def, "cyclomaticComplexity"), 0);
    }

    #[test]
    fn test_get_sort_value_with_stats() {
        let def = make_def("Test", None, DefinitionKind::Method);
        let stats = CodeStats {
            cyclomatic_complexity: 15,
            cognitive_complexity: 25,
            max_nesting_depth: 4,
            param_count: 3,
            return_count: 2,
            call_count: 10,
            lambda_count: 1,
        };
        assert_eq!(get_sort_value(Some(&stats), &def, "cyclomaticComplexity"), 15);
        assert_eq!(get_sort_value(Some(&stats), &def, "cognitiveComplexity"), 25);
        assert_eq!(get_sort_value(Some(&stats), &def, "maxNestingDepth"), 4);
        assert_eq!(get_sort_value(Some(&stats), &def, "paramCount"), 3);
        assert_eq!(get_sort_value(Some(&stats), &def, "returnCount"), 2);
        assert_eq!(get_sort_value(Some(&stats), &def, "callCount"), 10);
        assert_eq!(get_sort_value(Some(&stats), &def, "lambdaCount"), 1);
    }

    #[test]
    fn test_get_sort_value_unknown_field_returns_zero() {
        let def = make_def("Test", None, DefinitionKind::Method);
        let stats = CodeStats {
            cyclomatic_complexity: 15,
            cognitive_complexity: 25,
            max_nesting_depth: 4,
            param_count: 3,
            return_count: 2,
            call_count: 10,
            lambda_count: 1,
        };
        assert_eq!(get_sort_value(Some(&stats), &def, "unknownField"), 0);
    }

    // ═══════════════════════════════════════════════════════════════════
    // ADDITIONAL TESTS — covering remaining gaps
    // ═══════════════════════════════════════════════════════════════════

    // ─── parse_definition_args: remaining field coverage ─────────────

    #[test]
    fn test_parse_args_audit_and_cross_validate() {
        let args = json!({"audit": true, "crossValidate": true, "auditMinBytes": 1000});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.audit);
        assert!(parsed.cross_validate);
        assert_eq!(parsed.audit_min_bytes, 1000);
    }

    #[test]
    fn test_parse_args_audit_defaults() {
        let args = json!({"audit": true});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.audit);
        assert!(!parsed.cross_validate);
        assert_eq!(parsed.audit_min_bytes, 500, "default auditMinBytes should be 500");
    }

    #[test]
    fn test_parse_args_body_params() {
        let args = json!({"includeBody": true, "maxBodyLines": 50, "maxTotalBodyLines": 200});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.include_body);
        assert_eq!(parsed.max_body_lines, 50);
        assert_eq!(parsed.max_total_body_lines, 200);
    }

    #[test]
    fn test_parse_args_use_regex() {
        let args = json!({"regex": true});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.use_regex);
    }

    #[test]
    fn test_parse_args_base_type_transitive() {
        let args = json!({"baseType": "IService", "baseTypeTransitive": true});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.base_type_filter, Some("IService".to_string()));
        assert!(parsed.base_type_transitive);
    }

    #[test]
    fn test_parse_args_file_and_parent_filter() {
        let args = json!({"file": "UserService.cs", "parent": "UserService"});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.file_filter, Some("UserService.cs".to_string()));
        assert_eq!(parsed.parent_filter, Some("UserService".to_string()));
    }

    #[test]
    fn test_parse_args_max_results_custom() {
        let args = json!({"maxResults": 50});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.max_results, 50);
    }

    #[test]
    fn test_parse_args_max_results_zero_means_unlimited() {
        let args = json!({"maxResults": 0});
        let parsed = parse_definition_args(&args).unwrap();
        assert_eq!(parsed.max_results, 0);
    }

    #[test]
    fn test_parse_args_contains_line_non_numeric_ignored() {
        let args = json!({"containsLine": "abc"});
        let parsed = parse_definition_args(&args).unwrap();
        assert!(parsed.contains_line.is_none(), "non-numeric containsLine should be None");
    }

    // ─── collect_candidates: additional edge cases ───────────────────

    #[test]
    fn test_collect_candidates_invalid_regex_returns_error() {
        let index = make_test_def_index();
        let args = parse_definition_args(&json!({"name": "[invalid(", "regex": true})).unwrap();
        let result = collect_candidates(&index, &args);
        assert!(result.is_err(), "Invalid regex should return error");
        assert!(result.unwrap_err().contains("Invalid regex"));
    }

    #[test]
    fn test_collect_candidates_kind_no_matches_returns_empty() {
        let index = make_test_def_index();
        // "property" kind exists in the enum but no definitions have it
        let args = parse_definition_args(&json!({"kind": "property"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert!(candidates.is_empty(), "No properties exist → empty result");
    }

    #[test]
    fn test_collect_candidates_attribute_and_kind_intersection() {
        let index = make_test_def_index();
        // Injectable attribute is on UserService (class). kind=method should yield empty intersection
        let args = parse_definition_args(&json!({"attribute": "Injectable", "kind": "method"})).unwrap();
        let (candidates, _) = collect_candidates(&index, &args).unwrap();
        assert!(candidates.is_empty(), "Injectable + method → empty (Injectable is on a class)");
    }

    // ─── apply_entry_filters: additional edge cases ──────────────────

    #[test]
    fn test_apply_entry_filters_combined_file_and_parent() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        // Both file and parent filter — intersection
        let args = parse_definition_args(&json!({
            "file": "UserService.cs",
            "parent": "UserService"
        })).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 1, "file=UserService.cs + parent=UserService → only GetUser");
        assert_eq!(results[0].1.name, "GetUser");
    }

    #[test]
    fn test_apply_entry_filters_case_insensitive_file() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        // Uppercase file filter still matches
        let args = parse_definition_args(&json!({"file": "USERSERVICE.CS"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        assert_eq!(results.len(), 2, "case-insensitive file filter should match");
    }

    #[test]
    fn test_apply_entry_filters_parent_null_excluded_when_parent_filter_set() {
        let index = make_test_def_index();
        let candidates: Vec<u32> = (0..4).collect();
        // UserService (idx 0) and OrderService (idx 2) have parent=None → excluded
        let args = parse_definition_args(&json!({"parent": "UserService,OrderService"})).unwrap();
        let results = apply_entry_filters(&index, &candidates, &args);
        // Only methods (which have parents) should be returned
        for (_, def) in &results {
            assert!(def.parent.is_some(), "When parent filter is set, defs without parent are excluded");
        }
    }

    // ─── apply_stats_filters: actual filtering with populated code_stats ──

    /// Helper: create a DefinitionIndex with populated code_stats
    fn make_index_with_stats() -> DefinitionIndex {
        let mut index = make_test_def_index();
        // Add code_stats for methods (idx 1: GetUser, idx 3: GetOrder)
        index.code_stats.insert(1, CodeStats {
            cyclomatic_complexity: 15,
            cognitive_complexity: 25,
            max_nesting_depth: 4,
            param_count: 3,
            return_count: 2,
            call_count: 10,
            lambda_count: 1,
        });
        index.code_stats.insert(3, CodeStats {
            cyclomatic_complexity: 5,
            cognitive_complexity: 8,
            max_nesting_depth: 2,
            param_count: 1,
            return_count: 1,
            call_count: 3,
            lambda_count: 0,
        });
        index
    }

    #[test]
    fn test_apply_stats_filters_min_complexity_filters() {
        let index = make_index_with_stats();
        let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let args = parse_definition_args(&json!({"minComplexity": 10})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert!(info.applied);
        assert_eq!(info.before_count, 4);
        // Only GetUser (complexity=15) should pass; GetOrder (5) filtered out;
        // UserService and OrderService have no stats → filtered out
        assert_eq!(results.len(), 1, "Only GetUser passes minComplexity=10");
        assert_eq!(results[0].1.name, "GetUser");
    }

    #[test]
    fn test_apply_stats_filters_min_params_filters() {
        let index = make_index_with_stats();
        let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let args = parse_definition_args(&json!({"minParams": 2})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert!(info.applied);
        // GetUser has param_count=3, GetOrder has param_count=1
        assert_eq!(results.len(), 1, "Only GetUser passes minParams=2");
        assert_eq!(results[0].1.name, "GetUser");
    }

    #[test]
    fn test_apply_stats_filters_multiple_min_filters_and_logic() {
        let index = make_index_with_stats();
        let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        // GetUser: complexity=15, nesting=4. GetOrder: complexity=5, nesting=2
        // minComplexity=3 AND minNesting=3 → only GetUser passes both
        let args = parse_definition_args(&json!({"minComplexity": 3, "minNesting": 3})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert!(info.applied);
        assert_eq!(results.len(), 1, "AND logic: only GetUser passes both thresholds");
        assert_eq!(results[0].1.name, "GetUser");
    }

    #[test]
    fn test_apply_stats_filters_before_count_correct() {
        let index = make_index_with_stats();
        let mut results: Vec<(u32, &DefinitionEntry)> = index.definitions.iter()
            .enumerate()
            .map(|(i, d)| (i as u32, d))
            .collect();
        let args = parse_definition_args(&json!({"minComplexity": 100})).unwrap();
        let info = apply_stats_filters(&index, &mut results, &args).unwrap();
        assert_eq!(info.before_count, 4, "before_count should capture pre-filter count");
        assert_eq!(results.len(), 0, "No results pass minComplexity=100");
    }

    // ─── format_definition_entry: direct tests ───────────────────────

    #[test]
    fn test_format_definition_entry_basic_fields() {
        let index = make_test_def_index();
        let def = &index.definitions[1]; // GetUser method
        let args = parse_definition_args(&json!({})).unwrap();
        let mut cache = HashMap::new();
        let mut body_lines = 0usize;
        let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
        assert_eq!(obj["name"], "GetUser");
        assert_eq!(obj["kind"], "method");
        assert!(obj["file"].as_str().unwrap().contains("UserService"));
        assert_eq!(obj["lines"], "10-30");
        assert_eq!(obj["parent"], "UserService");
        // No body by default
        assert!(obj.get("body").is_none());
        assert!(obj.get("codeStats").is_none());
    }

    #[test]
    fn test_format_definition_entry_optional_fields_absent_when_empty() {
        let index = make_test_def_index();
        let def = &index.definitions[0]; // UserService class — no signature, no parent
        let args = parse_definition_args(&json!({})).unwrap();
        let mut cache = HashMap::new();
        let mut body_lines = 0usize;
        let obj = format_definition_entry(&index, 0, def, &args, &mut cache, &mut body_lines);
        assert!(obj.get("parent").is_none(), "no parent → field absent");
        assert!(obj.get("signature").is_none(), "no signature → field absent");
        // modifiers is empty → should be absent
        assert!(obj.get("modifiers").is_none(), "empty modifiers → field absent");
    }

    #[test]
    fn test_format_definition_entry_with_code_stats() {
        let index = make_index_with_stats();
        let def = &index.definitions[1]; // GetUser — has code_stats
        let args = parse_definition_args(&json!({"includeCodeStats": true})).unwrap();
        let mut cache = HashMap::new();
        let mut body_lines = 0usize;
        let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
        assert!(obj.get("codeStats").is_some(), "includeCodeStats=true → codeStats present");
        let stats = &obj["codeStats"];
        assert_eq!(stats["cyclomaticComplexity"], 15);
        assert_eq!(stats["cognitiveComplexity"], 25);
        assert_eq!(stats["maxNestingDepth"], 4);
        assert_eq!(stats["paramCount"], 3);
        assert_eq!(stats["returnCount"], 2);
        assert_eq!(stats["callCount"], 10);
        assert_eq!(stats["lambdaCount"], 1);
        assert_eq!(stats["lines"], 21); // 30 - 10 + 1
    }

    #[test]
    fn test_format_definition_entry_no_code_stats_when_not_requested() {
        let index = make_index_with_stats();
        let def = &index.definitions[1]; // GetUser — has code_stats
        let args = parse_definition_args(&json!({})).unwrap(); // includeCodeStats defaults false
        let mut cache = HashMap::new();
        let mut body_lines = 0usize;
        let obj = format_definition_entry(&index, 1, def, &args, &mut cache, &mut body_lines);
        assert!(obj.get("codeStats").is_none(), "includeCodeStats=false → no codeStats");
    }

    #[test]
    fn test_format_definition_entry_with_attributes() {
        let index = make_test_def_index();
        let def = &index.definitions[0]; // UserService — has attributes: ["injectable"]
        let args = parse_definition_args(&json!({})).unwrap();
        let mut cache = HashMap::new();
        let mut body_lines = 0usize;
        let obj = format_definition_entry(&index, 0, def, &args, &mut cache, &mut body_lines);
        assert!(obj.get("attributes").is_some(), "UserService has attributes");
        let attrs = obj["attributes"].as_array().unwrap();
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0], "injectable");
    }

    // ─── build_search_summary: direct tests ──────────────────────────

    #[test]
    fn test_build_search_summary_basic_fields() {
        let index = make_test_def_index();
        let defs_json = vec![json!({"name": "a"}), json!({"name": "b"})];
        let args = parse_definition_args(&json!({})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 2 };
        let elapsed = std::time::Duration::from_millis(5);
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 10, &stats_info, &None, 0, elapsed, &ctx);
        assert_eq!(summary["totalResults"], 10);
        assert_eq!(summary["returned"], 2);
        assert_eq!(summary["indexFiles"], 2); // 2 files in make_test_def_index
        assert!(summary["searchTimeMs"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn test_build_search_summary_sorted_by_field() {
        let index = make_test_def_index();
        let defs_json = vec![];
        let args = parse_definition_args(&json!({"sortBy": "cognitiveComplexity"})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 0, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert_eq!(summary["sortedBy"], "cognitiveComplexity");
    }

    #[test]
    fn test_build_search_summary_stats_filters_applied() {
        let index = make_test_def_index();
        let defs_json = vec![json!({"name": "a"})];
        let args = parse_definition_args(&json!({"minComplexity": 5})).unwrap();
        let stats_info = StatsFilterInfo { applied: true, before_count: 10 };
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 1, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert_eq!(summary["statsFiltersApplied"], true);
        assert_eq!(summary["beforeStatsFilter"], 10);
        assert_eq!(summary["afterStatsFilter"], 1);
    }

    #[test]
    fn test_build_search_summary_code_stats_unavailable() {
        let index = make_test_def_index(); // empty code_stats
        let defs_json = vec![];
        let args = parse_definition_args(&json!({"includeCodeStats": true})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 0, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert_eq!(summary["codeStatsAvailable"], false);
    }

    #[test]
    fn test_build_search_summary_body_lines_reported() {
        let index = make_test_def_index();
        let defs_json = vec![];
        let args = parse_definition_args(&json!({"includeBody": true})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 0, &stats_info, &None, 42,
            std::time::Duration::ZERO, &ctx);
        assert_eq!(summary["totalBodyLinesReturned"], 42);
    }

    #[test]
    fn test_build_search_summary_no_body_lines_when_not_requested() {
        let index = make_test_def_index();
        let defs_json = vec![];
        let args = parse_definition_args(&json!({})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();
        let summary = build_search_summary(
            &index, &defs_json, &args, 0, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert!(summary.get("totalBodyLinesReturned").is_none(),
            "No body → no totalBodyLinesReturned");
    }

    #[test]
    fn test_build_search_summary_read_errors_only_when_nonzero() {
        let mut index = make_test_def_index();
        let args = parse_definition_args(&json!({})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();

        // No errors → field absent
        let summary = build_search_summary(
            &index, &[], &args, 0, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert!(summary.get("readErrors").is_none());

        // With errors → field present
        index.parse_errors = 3;
        let summary2 = build_search_summary(
            &index, &[], &args, 0, &stats_info, &None, 0,
            std::time::Duration::ZERO, &ctx);
        assert_eq!(summary2["readErrors"], 3);
    }

    #[test]
    fn test_build_search_summary_term_breakdown_injected() {
        let index = make_test_def_index();
        let defs_json = vec![];
        let args = parse_definition_args(&json!({})).unwrap();
        let stats_info = StatsFilterInfo { applied: false, before_count: 0 };
        let ctx = HandlerContext::default();
        let breakdown = Some(json!({"term1": 5, "term2": 3}));
        let summary = build_search_summary(
            &index, &defs_json, &args, 0, &stats_info, &breakdown, 0,
            std::time::Duration::ZERO, &ctx);
        assert!(summary.get("termBreakdown").is_some());
        assert_eq!(summary["termBreakdown"]["term1"], 5);
    }

    // ─── sort_results: additional coverage ───────────────────────────

    #[test]
    fn test_sort_results_kind_priority_class_before_method() {
        let index = make_test_def_index();
        // Search for "service" — UserService (class) and GetUser (method containing "service" via parent)
        // Actually, let's search for "user" which matches: UserService (class) and GetUser (method)
        let mut results: Vec<(u32, &DefinitionEntry)> = vec![
            (1, &index.definitions[1]), // GetUser (method)
            (0, &index.definitions[0]), // UserService (class)
        ];
        let args = parse_definition_args(&json!({"name": "user"})).unwrap();
        sort_results(&mut results, &index, &args);
        // Both contain "user" — class (kind_priority=0) should come before method (kind_priority=1)
        assert_eq!(results[0].1.kind, DefinitionKind::Class,
            "Class should sort before method (kind priority tiebreaker)");
    }

    #[test]
    fn test_sort_results_parent_filter_exact_parent_first() {
        let index = make_test_def_index();
        let mut results: Vec<(u32, &DefinitionEntry)> = vec![
            (3, &index.definitions[3]), // GetOrder (parent: OrderService)
            (1, &index.definitions[1]), // GetUser (parent: UserService)
        ];
        // parent=UserService — exact match should sort first
        let args = parse_definition_args(&json!({"parent": "UserService"})).unwrap();
        sort_results(&mut results, &index, &args);
        assert_eq!(results[0].1.name, "GetUser",
            "Exact parent match 'UserService' should sort first");
    }

    #[test]
    fn test_sort_results_name_length_tiebreaker() {
        // Two definitions with same kind, both exact matches — shorter name first
        let defs = vec![
            make_def("AB", None, DefinitionKind::Class),   // shorter
            make_def("ABC", None, DefinitionKind::Class),  // longer
        ];
        let index = DefinitionIndex {
            root: ".".to_string(), created_at: 0,
            extensions: vec!["cs".to_string()],
            files: vec!["C:\\src\\file.cs".to_string()],
            definitions: defs,
            name_index: {
                let mut m = HashMap::new();
                m.insert("ab".to_string(), vec![0]);
                m.insert("abc".to_string(), vec![1]);
                m
            },
            kind_index: {
                let mut m = HashMap::new();
                m.entry(DefinitionKind::Class).or_insert_with(Vec::new).extend([0u32, 1]);
                m
            },
            file_index: {
                let mut m = HashMap::new();
                m.insert(0u32, vec![0, 1]);
                m
            },
            ..Default::default()
        };
        let mut results: Vec<(u32, &DefinitionEntry)> = vec![
            (1, &index.definitions[1]), // ABC
            (0, &index.definitions[0]), // AB
        ];
        // Both contain "ab" — tiebreak by name length
        let args = parse_definition_args(&json!({"name": "ab"})).unwrap();
        sort_results(&mut results, &index, &args);
        assert_eq!(results[0].1.name, "AB", "Shorter name should sort first as tiebreaker");
        assert_eq!(results[1].1.name, "ABC");
    }

    // ─── compute_term_breakdown: edge case ───────────────────────────

    #[test]
    fn test_compute_term_breakdown_comma_only_returns_none() {
        let results: Vec<(u32, &DefinitionEntry)> = vec![];
        let def_to_term = HashMap::new();
        let args = parse_definition_args(&json!({"name": ",,,"})).unwrap();
        // name=",,," → after filtering empty strings, terms is empty → name_filter is Some(",,,") but terms.len() < 2
        // Actually, ",,," splits into ["", "", "", ""], filter empty → empty vec, len() = 0 < 2 → None
        let breakdown = compute_term_breakdown(&results, &def_to_term, &args);
        assert!(breakdown.is_none(), "Comma-only name → no usable terms → no breakdown");
    }

    // ─── property→field hint tests ────────────────────────────────────

    #[test]
    fn test_kind_property_hint_when_fields_exist() {
        // Create an index with Field definitions but no Property definitions
        let mut index = make_test_def_index();
        // Add a Field definition explicitly
        let field_def = DefinitionEntry {
            name: "title".to_string(), kind: DefinitionKind::Field,
            file_id: 0, line_start: 5, line_end: 5,
            signature: Some("title: string".to_string()),
            parent: Some("UserService".to_string()),
            modifiers: vec!["private".to_string()],
            attributes: vec![], base_types: vec![],
        };
        let field_idx = index.definitions.len() as u32;
        index.definitions.push(field_def);
        index.kind_index.entry(DefinitionKind::Field).or_default().push(field_idx);
        index.name_index.entry("title".to_string()).or_default().push(field_idx);
        index.file_index.entry(0).or_default().push(field_idx);

        let content_index = crate::ContentIndex {
            root: ".".to_string(),
            extensions: vec!["ts".to_string()],
            ..Default::default()
        };

        let ctx = super::HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(content_index)),
            def_index: Some(std::sync::Arc::new(std::sync::RwLock::new(index))),
            ..Default::default()
        };

        // Search with kind="property" and parent="UserService" — should return 0 results + hint
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "kind": "property",
            "parent": "UserService"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 0, "kind=property should return 0 for TS class fields");

        // Should have a hint suggesting kind='field'
        let hint = v["summary"]["hint"].as_str();
        assert!(hint.is_some(), "Should have hint when kind=property returns 0 but fields exist");
        assert!(hint.unwrap().contains("kind='field'"),
            "Hint should suggest kind='field'. Got: {}", hint.unwrap());
    }

    #[test]
    fn test_kind_property_no_hint_when_results_exist() {
        // If kind="property" returns results, no hint needed
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "kind": "class"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(v["summary"].get("hint").is_none(),
            "No hint when results are returned");
    }
}