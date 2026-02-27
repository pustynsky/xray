//! search_definitions handler: AST-based code definition search.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::definitions::{DefinitionEntry, DefinitionKind, CodeStats};

use super::utils::{inject_body_into_obj, inject_branch_warning, best_match_tier};
use super::HandlerContext;

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

    let name_filter = args.get("name").and_then(|v| v.as_str())
        .and_then(|s| if s.is_empty() { None } else { Some(s) });
    let kind_filter = args.get("kind").and_then(|v| v.as_str());
    let attribute_filter = args.get("attribute").and_then(|v| v.as_str());
    let base_type_filter = args.get("baseType").and_then(|v| v.as_str())
        .and_then(|s| if s.is_empty() { None } else { Some(s) });
    let base_type_transitive = args.get("baseTypeTransitive").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_filter = args.get("file").and_then(|v| v.as_str());
    let parent_filter = args.get("parent").and_then(|v| v.as_str());
    let contains_line = match args.get("containsLine") {
        Some(v) if v.is_i64() || v.is_u64() => {
            match v.as_i64() {
                Some(n) if n < 1 => return ToolCallResult::error(
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

    // Code stats parameters
    let sort_by = args.get("sortBy").and_then(|v| v.as_str());
    let min_complexity = args.get("minComplexity").and_then(|v| v.as_u64()).map(|v| v as u16);
    let min_cognitive = args.get("minCognitive").and_then(|v| v.as_u64()).map(|v| v as u16);
    let min_nesting = args.get("minNesting").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_params = args.get("minParams").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_returns = args.get("minReturns").and_then(|v| v.as_u64()).map(|v| v as u8);
    let min_calls = args.get("minCalls").and_then(|v| v.as_u64()).map(|v| v as u16);

    let has_stats_filter = sort_by.is_some()
        || min_complexity.is_some()
        || min_cognitive.is_some()
        || min_nesting.is_some()
        || min_params.is_some()
        || min_returns.is_some()
        || min_calls.is_some();

    // sortBy and min* imply includeCodeStats=true
    let include_code_stats = args.get("includeCodeStats").and_then(|v| v.as_bool()).unwrap_or(false)
        || has_stats_filter;

    // Validate sortBy value
    if let Some(sort_field) = sort_by {
        let valid = ["cyclomaticComplexity", "cognitiveComplexity", "maxNestingDepth",
                     "paramCount", "returnCount", "callCount", "lambdaCount", "lines"];
        if !valid.contains(&sort_field) {
            return ToolCallResult::error(format!(
                "Invalid sortBy value '{}'. Valid values: {}",
                sort_field, valid.join(", ")
            ));
        }
    }

    // --- audit mode: return index coverage report ---
    if audit {
        let suspicious_threshold = args.get("auditMinBytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as u64;
        let cross_validate = args.get("crossValidate").and_then(|v| v.as_bool()).unwrap_or(false);

        let files_with_defs = index.file_index.len();
        let total_files = index.files.len();
        let files_without_defs = index.empty_file_ids.len();

        let suspicious: Vec<Value> = index.empty_file_ids.iter()
            .filter(|(_, size)| *size > suspicious_threshold)
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
                "suspiciousThresholdBytes": suspicious_threshold,
            },
            "suspiciousFiles": suspicious,
        });

        // Cross-index validation: compare definition index files with file-list index
        if cross_validate {
            let cross = cross_validate_indexes(&index, &ctx.server_dir, &ctx.index_base);
            output["crossValidation"] = cross;
        }

        return ToolCallResult::success(serde_json::to_string(&output).unwrap());
    }

    // --- containsLine: find containing method/class by line number ---
    if let Some(line_num) = contains_line {
        if file_filter.is_none() {
            return ToolCallResult::error(
                "containsLine requires 'file' parameter to identify the file.".to_string()
            );
        }
        let file_substr = file_filter.unwrap().replace('\\', "/").to_lowercase();

        // Find matching file(s)
        let mut containing_defs: Vec<Value> = Vec::new();
        let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
        let mut total_body_lines_emitted: usize = 0;
        for (file_id, file_path) in index.files.iter().enumerate() {
            if !file_path.replace('\\', "/").to_lowercase().contains(&file_substr) {
                continue;
            }
            // Get all definitions in this file
            if let Some(def_indices) = index.file_index.get(&(file_id as u32)) {
                // Find all definitions that contain this line, sorted by specificity
                // (innermost first = smallest line range)
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
                    if include_body {
                        inject_body_into_obj(
                            &mut obj, file_path, def.line_start, def.line_end,
                            &mut file_cache, &mut total_body_lines_emitted,
                            max_body_lines, max_total_body_lines,
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
        if include_body {
            summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
        }
        inject_branch_warning(&mut summary, ctx);
        let output = json!({
            "containingDefinitions": containing_defs,
            "query": {
                "file": file_filter.unwrap(),
                "line": line_num,
            },
            "summary": summary,
        });
        return ToolCallResult::success(serde_json::to_string(&output).unwrap());
    }

    // Start with candidate indices
    let mut candidate_indices: Option<Vec<u32>> = None;

    // Filter by kind first (most selective usually)
    if let Some(kind_str) = kind_filter {
        match kind_str.parse::<DefinitionKind>() {
            Ok(kind) => {
                if let Some(indices) = index.kind_index.get(&kind) {
                    candidate_indices = Some(indices.clone());
                } else {
                    candidate_indices = Some(Vec::new());
                }
            }
            Err(e) => {
                return ToolCallResult::error(e);
            }
        }
    }

    // Filter by attribute
    if let Some(attr) = attribute_filter {
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
    // Uses substring matching to support generic types: baseType="IAccessTable"
    // matches IAccessTable<Model>, IAccessTable<Report>, etc.
    // Fast path: try exact HashMap lookup first (O(1)), fall back to substring scan
    // only when exact match returns nothing.
    if let Some(bt) = base_type_filter {
        let bt_lower = bt.to_lowercase();
        let matching_indices = if base_type_transitive {
            // BFS: collect all types transitively inheriting from bt (substring match)
            collect_transitive_base_type_indices(&index, &bt_lower)
        } else {
            // Substring match: find all base_type_index keys containing bt_lower.
            // Supports generic types: "IAccessTable" matches "iaccesstable<model>",
            // "iaccesstable<report>", etc.
            // O(N) scan over base_type_index keys (~1.4ms for 50K entries).
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

    // Track which term matched each definition (for termBreakdown in multi-term name queries)
    let mut def_to_term: HashMap<u32, usize> = HashMap::new();

    // Filter by name
    if let Some(name) = name_filter {
        if use_regex {
            // Regex match against all names in the index
            let re = match regex::Regex::new(&format!("(?i){}", name)) {
                Ok(r) => r,
                Err(e) => return ToolCallResult::error(format!("Invalid regex '{}': {}", name, e)),
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
                // Find the first matching term for termBreakdown tracking
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

    // If no filters applied, return all ACTIVE definitions (up to max).
    // Use file_index to exclude tombstoned entries that are no longer referenced.
    let mut candidates = candidate_indices.unwrap_or_else(|| {
        index.file_index.values().flat_map(|v| v.iter().copied()).collect()
    });

    // Deduplicate candidate indices (a definition may appear multiple times
    // if e.g. multiple attributes normalize to the same name)
    candidates.sort_unstable();
    candidates.dedup();

    // Apply remaining filters (file, parent, excludeDir) on actual entries
    // Track (def_idx, &DefinitionEntry) for code_stats lookup
    let mut results: Vec<(u32, &DefinitionEntry)> = candidates.iter()
        .filter_map(|&idx| {
            let def = index.definitions.get(idx as usize)?;
            let file_path = index.files.get(def.file_id as usize)?;

            // File filter: comma-separated OR with substring matching
            // (e.g., "UserService.cs,OrderService.cs" matches files containing ANY term)
            if let Some(ff) = file_filter {
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
            // (e.g., "UserService,OrderService" matches members of ANY class)
            if let Some(pf) = parent_filter {
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
            if exclude_dir.iter().any(|excl| {
                file_path.to_lowercase().contains(&excl.to_lowercase())
            }) {
                return None;
            }

            Some((idx, def))
        })
        .collect();

    // ── Stats error check & filtering ──
    let mut stats_filters_applied = false;
    let before_stats_count = results.len();
    if has_stats_filter {
        // sortBy='lines' works without code_stats
        let needs_code_stats = sort_by != Some("lines");

        if needs_code_stats && index.code_stats.is_empty() {
            return ToolCallResult::error(
                "Code stats not available for this index. Run search_reindex_definitions to compute metrics.".to_string()
            );
        }

        if needs_code_stats {
            // Filter to only definitions with code_stats, apply min* thresholds
            results.retain(|(def_idx, _def)| {
                let stats = match index.code_stats.get(def_idx) {
                    Some(s) => s,
                    None => return false, // skip classes, fields, etc.
                };

                if let Some(min) = min_complexity {
                    if stats.cyclomatic_complexity < min { return false; }
                }
                if let Some(min) = min_cognitive {
                    if stats.cognitive_complexity < min { return false; }
                }
                if let Some(min) = min_nesting {
                    if stats.max_nesting_depth < min { return false; }
                }
                if let Some(min) = min_params {
                    if stats.param_count < min { return false; }
                }
                if let Some(min) = min_returns {
                    if stats.return_count < min { return false; }
                }
                if let Some(min) = min_calls {
                    if stats.call_count < min { return false; }
                }
                true
            });
            stats_filters_applied = true;
        }
    }

    let total_results = results.len();

    // Build termBreakdown for multi-term name queries (computed from full result set,
    // before truncation, so LLM sees the true distribution across terms)
    let term_breakdown: Option<Value> = if !use_regex {
        if let Some(name) = name_filter {
            let terms: Vec<String> = name.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            if terms.len() >= 2 {
                let mut breakdown = serde_json::Map::new();
                for (i, term) in terms.iter().enumerate() {
                    let count = results.iter()
                        .filter(|(idx, _)| def_to_term.get(idx) == Some(&i))
                        .count();
                    breakdown.insert(term.clone(), json!(count));
                }
                Some(json!(breakdown))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // ── Sorting ──
    if let Some(sort_field) = sort_by {
        // Sort by metric (descending — worst first)
        results.sort_by(|(idx_a, def_a), (idx_b, def_b)| {
            let va = get_sort_value(index.code_stats.get(idx_a), def_a, sort_field);
            let vb = get_sort_value(index.code_stats.get(idx_b), def_b, sort_field);
            vb.cmp(&va) // descending — worst first
        });
    } else if (name_filter.is_some() && !use_regex) || parent_filter.is_some() {
        // Relevance ranking when name or parent filter is active (not regex)
        // - Name terms: exact name > prefix > substring
        // - Parent terms: exact parent > prefix > substring
        let name_terms: Vec<String> = name_filter
            .map(|n| n.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();

        let parent_terms: Vec<String> = parent_filter
            .map(|p| p.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();

        results.sort_by(|(_, a), (_, b)| {
            // Parent relevance first (when parent filter is set, exact parent match is critical)
            let parent_tier_a = if !parent_terms.is_empty() {
                a.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
            } else { 0 };
            let parent_tier_b = if !parent_terms.is_empty() {
                b.parent.as_deref().map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3)
            } else { 0 };

            parent_tier_a.cmp(&parent_tier_b)
                .then_with(|| {
                    // Name relevance as secondary sort
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

    // Apply max results
    if max_results > 0 && results.len() > max_results {
        results.truncate(max_results);
    }

    let search_elapsed = search_start.elapsed();

    // Build output JSON
    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total_body_lines_emitted: usize = 0;
    let defs_json: Vec<Value> = results.iter().map(|(def_idx_value, def)| {
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
        // Add Angular template metadata
        if let Some(children) = index.template_children.get(&(*def_idx_value as u32)) {
            obj["templateChildren"] = json!(children);
        }
        for (selector, sel_indices) in &index.selector_index {
            if sel_indices.contains(&(*def_idx_value as u32)) {
                obj["selector"] = json!(selector);
                break;
            }
        }

        if include_body {
            inject_body_into_obj(
                &mut obj, file_path, def.line_start, def.line_end,
                &mut file_cache, &mut total_body_lines_emitted,
                max_body_lines, max_total_body_lines,
            );
        }

        // Inject codeStats if requested
        if include_code_stats {
            if let Some(stats) = index.code_stats.get(def_idx_value) {
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
    }).collect();

    let active_definitions: usize = index.file_index.values().map(|v| v.len()).sum();
    let mut summary = json!({
        "totalResults": total_results,
        "returned": defs_json.len(),
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        "indexFiles": index.files.len(),
        "totalDefinitions": active_definitions,
    });
    // Hint for large transitive hierarchies
    if base_type_transitive && total_results > 5000 {
        if let Some(bt) = base_type_filter {
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
    if include_body {
        summary["totalBodyLinesReturned"] = json!(total_body_lines_emitted);
    }
    if let Some(sort_field) = sort_by {
        summary["sortedBy"] = json!(sort_field);
    }
    if stats_filters_applied {
        summary["statsFiltersApplied"] = json!(true);
        summary["afterStatsFilter"] = json!(total_results);
        summary["beforeStatsFilter"] = json!(before_stats_count);
    }
    if include_code_stats && index.code_stats.is_empty() {
        summary["codeStatsAvailable"] = json!(false);
    }
    if let Some(ref breakdown) = term_breakdown {
        summary["termBreakdown"] = breakdown.clone();
    }
    inject_branch_warning(&mut summary, ctx);
    let output = json!({
        "definitions": defs_json,
        "summary": summary,
    });

    ToolCallResult::success(serde_json::to_string(&output).unwrap())
}

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
    // e.g., "iaccesstable" matches "iaccesstable<model>", "iaccesstable<report>"
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
    // O(1) HashMap lookup instead of O(N) scan over all keys
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

    // Deduplicate (a definition could be found via multiple paths)
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
        // When parent_filter="UserService", exact parent "UserService" should rank
        // before substring parent "UserServiceMock"
        let parent_terms = vec!["userservice".to_string()];
        let _name_terms: Vec<String> = vec![];

        let def_exact = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
        let def_substring = make_def("GetUser", Some("UserServiceMock"), DefinitionKind::Method);

        let tier_exact = def_exact.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let tier_substring = def_substring.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        // Exact parent (tier 0) should sort before prefix/substring parent (tier 1 or 2)
        assert!(tier_exact < tier_substring,
            "Exact parent tier {} should be less than substring parent tier {}",
            tier_exact, tier_substring);
        assert_eq!(tier_exact, 0, "Exact parent match should be tier 0");
    }

    #[test]
    fn test_parent_ranking_prefix_parent_before_contains_parent() {
        // "UserServiceFactory" (prefix) should rank before "IUserService" (contains)
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
        // Even if name is exact match, parent tier should dominate
        let parent_terms = vec!["userservice".to_string()];
        let name_terms = vec!["getuser".to_string()];

        // def_a: exact name match + substring parent (tier 2)
        let def_a = make_def("GetUser", Some("MockUserServiceWrapper"), DefinitionKind::Method);
        // def_b: no exact name match + exact parent (tier 0)
        let def_b = make_def("FetchData", Some("UserService"), DefinitionKind::Method);

        let parent_tier_a = def_a.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);
        let parent_tier_b = def_b.parent.as_deref()
            .map(|p| best_match_tier(p, &parent_terms)).unwrap_or(3);

        let name_tier_a = best_match_tier(&def_a.name, &name_terms);
        let name_tier_b = best_match_tier(&def_b.name, &name_terms);

        // def_a has better name tier but worse parent tier
        assert!(name_tier_a < name_tier_b, "def_a should have better name tier");
        // def_b has better parent tier — it should sort first
        assert!(parent_tier_b < parent_tier_a, "def_b should have better parent tier");

        // Simulate the sort comparison
        let cmp = parent_tier_a.cmp(&parent_tier_b)
            .then_with(|| name_tier_a.cmp(&name_tier_b));
        assert_eq!(cmp, std::cmp::Ordering::Greater,
            "def_a should sort AFTER def_b because parent tier is primary");
    }

    #[test]
    fn test_parent_ranking_no_parent_sorts_last() {
        // Definitions without a parent should sort after those with a matching parent
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
        // When parent_filter is None, parent_terms is empty,
        // so parent tier should be 0 for all (no effect on sorting)
        let parent_terms: Vec<String> = vec![];

        let def_a = make_def("GetUser", Some("UserService"), DefinitionKind::Method);
        let def_b = make_def("FetchData", Some("OrderService"), DefinitionKind::Method);

        // When parent_terms is empty, both should get tier 0 (neutral)
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
        // file="ResilientClient.cs,ProxyClient.cs" should match defs from both files
        let ctx = super::super::handlers_test_utils::make_ctx_with_defs();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "file": "ResilientClient.cs,ProxyClient.cs",
            "kind": "method"
        }));
        assert!(!result.is_error, "should not error: {:?}", result.content[0].text);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        // Should find ExecuteQueryAsync in ResilientClient AND ExecuteQueryAsync in ProxyClient
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
        // Single file value (no comma) should work as before
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
        // parent="ResilientClient,ProxyClient" should match methods from both classes
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
        // When no file-list index exists on disk, crossValidate should return status: "skipped"
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
        // Create a temp dir, build a file-list index, then cross-validate
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        // Create some .cs files
        { let mut f = std::fs::File::create(project_dir.join("FileA.cs")).unwrap();
          writeln!(f, "class FileA {{ }}").unwrap(); }
        { let mut f = std::fs::File::create(project_dir.join("FileB.cs")).unwrap();
          writeln!(f, "class FileB {{ }}").unwrap(); }

        let project_str = crate::clean_path(&project_dir.to_string_lossy());
        let idx_base = tmp.path().join("indexes");
        std::fs::create_dir_all(&idx_base).unwrap();

        // Build and save a file-list index
        let file_index = crate::build_index(&crate::IndexArgs {
            dir: project_str.clone(),
            max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
        });
        crate::save_index(&file_index, &idx_base).unwrap();

        // Build a definition index
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
            // idx 0: BaseService (root, no base types)
            DefinitionEntry {
                name: "BaseService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            // idx 1: MiddleService : BaseService
            DefinitionEntry {
                name: "MiddleService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["BaseService".to_string()],
            },
            // idx 2: ConcreteService : MiddleService
            DefinitionEntry {
                name: "ConcreteService".to_string(),
                kind: DefinitionKind::Class,
                file_id: 1, line_start: 1, line_end: 80,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["MiddleService".to_string()],
            },
            // idx 3: UnrelatedService : SomethingElse
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
        // baseType=BaseService + baseTypeTransitive=true should find:
        // - MiddleService (directly inherits BaseService)
        // - ConcreteService (inherits MiddleService which inherits BaseService)
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
        // baseType=BaseService without transitive should find only MiddleService
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
        // baseType="" should behave like no baseType filter (return all definitions)
        // Regression: substring scan with contains("") matches ALL keys
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
        // baseType="BaseService" should match via fast-path (exact HashMap lookup)
        // but if the base_type_index has generic keys like "baseservice<t>",
        // searching with "baseservice" should find them via substring fallback
        use crate::definitions::*;

        let definitions = vec![
            // GenericImpl : IRepository<Model>
            DefinitionEntry {
                name: "GenericImpl".to_string(),
                kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Model>".to_string()],
            },
            // AnotherImpl : IRepository<Report>
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

        // Substring search: "IRepository" should find both GenericImpl and AnotherImpl
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "IRepository"
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let defs = v["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 2, "baseType='IRepository' should find both IRepository<Model> and IRepository<Report> via substring. Got: {:?}",
            defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>());

        // Exact search: "IRepository<Model>" should find only GenericImpl (fast path)
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
        // Regression test for B-1: BFS should NOT cascade when a descendant's name
        // is a substring of unrelated base_type_index keys.
        //
        // Setup:
        // - "BaseBlock" is the root type
        // - "ServiceBlock" inherits from "BaseBlock" ← level 0 finds this
        // - Unrelated entries: "iservice" → [UnrelatedA], "webservicebase" → [UnrelatedB]
        // - Without the fix, BFS level 1 would search for "serviceblock" via substring,
        //   and while "serviceblock" wouldn't match "iservice", a shorter name like "service"
        //   WOULD match. So we test with a descendant named exactly "Service" to trigger the cascade.
        //
        // Hierarchy:
        //   BaseBlock → Service (idx 1)
        //   Unrelated: iservice → UnrelatedA (idx 2), webservicebase → UnrelatedB (idx 3)
        //
        // Expected: transitive search for "BaseBlock" finds Service only, NOT UnrelatedA or UnrelatedB
        use crate::definitions::*;

        let definitions = vec![
            // idx 0: BaseBlock (root)
            DefinitionEntry {
                name: "BaseBlock".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![], base_types: vec![],
            },
            // idx 1: Service : BaseBlock  ← descendant with a "dangerous" short name
            DefinitionEntry {
                name: "Service".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["BaseBlock".to_string()],
            },
            // idx 2: UnrelatedA : IService  ← unrelated, but "iservice" contains "service"
            DefinitionEntry {
                name: "UnrelatedA".to_string(), kind: DefinitionKind::Class,
                file_id: 1, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IService".to_string()],
            },
            // idx 3: UnrelatedB : WebServiceBase  ← unrelated, "webservicebase" contains "service"
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

        // Service should be found (direct descendant of BaseBlock)
        assert!(names.contains(&"Service"),
            "Should find Service (direct descendant). Got: {:?}", names);

        // UnrelatedA and UnrelatedB should NOT be found
        // Without the fix, BFS level 1 would search for "service" via substring,
        // matching "iservice" and "webservicebase" → pulling in UnrelatedA and UnrelatedB
        assert!(!names.contains(&"UnrelatedA"),
            "Should NOT find UnrelatedA (unrelated, inherits IService not BaseBlock). Got: {:?}", names);
        assert!(!names.contains(&"UnrelatedB"),
            "Should NOT find UnrelatedB (unrelated, inherits WebServiceBase not BaseBlock). Got: {:?}", names);
    }

    #[test]
    fn test_base_type_transitive_generics_still_work_at_seed_level() {
        // Verify that substring matching at level 0 (seed) still works for generics
        // e.g., baseType="IRepository" should match "irepository<model>" and "irepository<report>"
        use crate::definitions::*;

        let definitions = vec![
            // idx 0: GenericImpl : IRepository<Model>
            DefinitionEntry {
                name: "GenericImpl".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 1, line_end: 50,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Model>".to_string()],
            },
            // idx 1: AnotherImpl : IRepository<Report>
            DefinitionEntry {
                name: "AnotherImpl".to_string(), kind: DefinitionKind::Class,
                file_id: 0, line_start: 52, line_end: 100,
                signature: None, parent: None, modifiers: vec![],
                attributes: vec![],
                base_types: vec!["IRepository<Report>".to_string()],
            },
            // idx 2: SubImpl : GenericImpl (transitive — should be found at level 1 via exact match)
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

        // Transitive search from IRepository should find GenericImpl, AnotherImpl, and SubImpl
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
        // When baseTypeTransitive=true and totalResults > 5000, a hint should appear
        // We can't easily create 5000+ definitions in a unit test, so we test the
        // negative case: no hint when results are small
        let ctx = make_transitive_inheritance_ctx();
        let result = handle_search_definitions(&ctx, &serde_json::json!({
            "baseType": "BaseService",
            "baseTypeTransitive": true
        }));
        assert!(!result.is_error);
        let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        // With only 2 results, no hint should be present
        assert!(v["summary"].get("hint").is_none(),
            "No hint expected for small result set (< 5000)");
    }

    #[test]
    fn test_parent_filter_comma_with_spaces_trimmed() {
        // Spaces around comma-separated terms should be trimmed
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
        // name="QueryService,ResilientClient" should show breakdown with counts for each term
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
        // Both should have > 0 results
        assert!(breakdown["queryservice"].as_u64().unwrap() > 0,
            "queryservice should have results");
        assert!(breakdown["resilientclient"].as_u64().unwrap() > 0,
            "resilientclient should have results");
    }

    #[test]
    fn test_term_breakdown_single_term_not_present() {
        // Single-term name query should NOT include termBreakdown
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
        // Regex name query should NOT include termBreakdown
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
        // No name filter at all should NOT include termBreakdown
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
        // One term matches, another doesn't — breakdown should show 0 for missing term
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
        // termBreakdown counts reflect the full result set before maxResults truncation
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
        // returned should be <= maxResults
        let returned = v["summary"]["returned"].as_u64().unwrap();
        assert!(returned <= 1, "returned should be <= maxResults=1, got {}", returned);
    }
}