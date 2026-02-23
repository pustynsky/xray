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
    let base_type_filter = args.get("baseType").and_then(|v| v.as_str());
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

        let output = json!({
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

    // Filter by base type
    if let Some(bt) = base_type_filter {
        let bt_lower = bt.to_lowercase();
        if let Some(indices) = index.base_type_index.get(&bt_lower) {
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
                if terms.iter().any(|t| n.contains(t)) {
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

    // If no filters applied, return all definitions (up to max)
    let mut candidates = candidate_indices.unwrap_or_else(|| {
        (0..index.definitions.len() as u32).collect()
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

            // File filter (normalize separators for cross-platform matching)
            if let Some(ff) = file_filter
                && !file_path.replace('\\', "/").to_lowercase().contains(&ff.replace('\\', "/").to_lowercase()) {
                    return None;
                }

            // Parent filter
            if let Some(pf) = parent_filter {
                match &def.parent {
                    Some(parent) => {
                        if !parent.to_lowercase().contains(&pf.to_lowercase()) {
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
            .map(|p| vec![p.to_lowercase()])
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

    let mut summary = json!({
        "totalResults": total_results,
        "returned": defs_json.len(),
        "searchTimeMs": search_elapsed.as_secs_f64() * 1000.0,
        "indexFiles": index.files.len(),
        "totalDefinitions": index.definitions.len(),
    });
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
}