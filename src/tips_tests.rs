use super::*;

#[test]
fn test_tips_not_empty() {
    assert!(!tips(&[]).is_empty());
}

#[test]
fn test_performance_tiers_not_empty() {
    assert!(!performance_tiers().is_empty());
}

#[test]
fn test_tool_priority_not_empty() {
    assert!(!tool_priority(&[]).is_empty());
}

#[test]
fn test_render_cli_contains_all_tips() {
    let exts = vec!["rs".to_string()];
    let output = render_cli(&exts);
    for tip in tips(&exts) {
        assert!(output.contains(&*tip.rule), "CLI output missing tip: {}", tip.rule);
    }
}

#[test]
fn test_strategies_not_empty() {
    assert!(!strategies().is_empty());
}

#[test]
fn test_render_json_has_best_practices() {
    let exts = vec!["rs".to_string()];
    let json = render_json(&exts);
    let practices = json["bestPractices"].as_array().unwrap();
    assert_eq!(practices.len(), tips(&exts).len());
}

#[test]
fn test_render_json_has_strategy_recipes() {
    let json = render_json(&[]);
    let recipes = json["strategyRecipes"].as_array().unwrap();
    assert_eq!(recipes.len(), strategies().len());
    // Each recipe has required fields
    for recipe in recipes {
        assert!(recipe["name"].is_string(), "recipe must have name");
        assert!(recipe["when"].is_string(), "recipe must have when");
        assert!(recipe["steps"].is_array(), "recipe must have steps");
        assert!(recipe["antiPatterns"].is_array(), "recipe must have antiPatterns");
    }
}

#[cfg(all(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-sql"))]
#[test]
fn test_render_instructions_contains_key_terms() {
    let text = render_instructions(crate::definitions::definition_extensions());
    // Core tools mentioned (via INTENT -> TOOL MAPPING and CRITICAL OVERRIDE)
    assert!(text.contains("xray_fast"), "instructions should mention xray_fast");
    assert!(text.contains("xray_callers"), "instructions should mention xray_callers");
    assert!(text.contains("xray_definitions"), "instructions should mention xray_definitions");
    assert!(text.contains("xray_grep"), "instructions should mention xray_grep");
    assert!(text.contains("xray_edit"), "instructions should mention xray_edit");
    // INTENT -> TOOL MAPPING replaces the older TASK ROUTING table
    assert!(text.contains("INTENT -> TOOL MAPPING"),
        "instructions should have INTENT -> TOOL MAPPING (replaces TASK ROUTING)");
    assert!(text.contains("consult BEFORE choosing any tool"),
        "INTENT -> TOOL MAPPING should have the directive telling the LLM to consult it first");
    assert!(!text.contains("TASK ROUTING"),
        "instructions should NOT have TASK ROUTING header (removed as duplicate of INTENT -> TOOL MAPPING)");
    // Key features mentioned in strategy recipes / DECISION TRIGGERs
    assert!(text.contains("includeBody"), "instructions should mention includeBody");
    // xray_help reference (soft, not urgent)
    assert!(text.contains("xray_help"), "instructions should mention xray_help");
    assert!(!text.contains("IMPORTANT: Call xray_help first"), "instructions should NOT have urgent xray_help prompt");
    // Strategy recipes trimmed: reference + query budget are inlined, detailed
    // recipe bodies live only in xray_help.
    assert!(text.contains("STRATEGY RECIPES"),
        "instructions should keep the STRATEGY RECIPES reference line");
    assert!(!text.contains("[Architecture Exploration]"),
        "Architecture Exploration recipe body should be xray_help-only");
    assert!(text.contains("<=3 search calls"), "instructions should mention query budget");
    // Dynamic extension list: verify all definition extensions appear in the NEVER READ rule
    for ext in crate::definitions::definition_extensions() {
        assert!(text.contains(&format!(".{}", ext)),
            "instructions should mention .{} extension in NEVER READ rule", ext);
    }
    assert!(text.contains("NEVER READ"), "instructions should have absolute prohibition on reading indexed files");
    assert!(text.contains("FILES DIRECTLY"), "instructions should have FILES DIRECTLY in prohibition");
    assert!(text.contains("DECISION TRIGGER"), "instructions should have decision trigger");
    assert!(text.contains("ONLY exception for"), "instructions should list exceptions for indexed file reading");
    // Response hints auto-follow rule (renamed from ZERO-RESULT HINTS)
    assert!(text.contains("RESPONSE HINTS"), "instructions should have RESPONSE HINTS auto-follow rule");
    assert!(!text.contains("ZERO-RESULT HINTS"), "instructions should NOT have old ZERO-RESULT HINTS (renamed to RESPONSE HINTS)");
    assert!(text.contains("AUTOMATICALLY follow the hint"), "instructions should tell LLM to auto-follow hints");
    assert!(text.contains("NEAREST MATCH"), "instructions should have NEAREST MATCH auto-follow rule");
    assert!(text.contains("KIND MISMATCH"), "instructions should have KIND MISMATCH auto-follow rule");
    assert!(text.contains("NEVER ask the user whether to follow a hint"), "instructions should prohibit asking user about hints");
    // Error recovery rule
    assert!(text.contains("ERROR RECOVERY"), "instructions should have ERROR RECOVERY rule");
    assert!(text.contains("NEVER fall back to built-in tools"), "ERROR RECOVERY should prohibit fallback");
    // Built-in tools should be explicitly mentioned as prohibited somewhere in the instructions
    // (MANDATORY PRE-FLIGHT CHECK block lists all of them; NEVER USE blocks repeat the ban).
    assert!(text.contains("list_files"), "instructions should mention list_files as prohibited");
    assert!(text.contains("list_directory"), "instructions should mention list_directory as prohibited");
    assert!(text.contains("directory_tree"), "instructions should mention directory_tree as prohibited");
    assert!(text.contains("apply_diff"), "instructions should mention apply_diff as prohibited");
    assert!(text.contains("search_and_replace"), "instructions should mention search_and_replace as prohibited");
    assert!(text.contains("insert_content"), "instructions should mention insert_content as prohibited");
    // INTENT -> TOOL MAPPING must include directory-listing routing
    assert!(text.contains("list files or subdirectories"),
        "INTENT -> TOOL MAPPING should include directory-listing intent");
    // Removed sections should NOT be present
    assert!(!text.contains("Quick Reference"), "instructions should NOT have Quick Reference (replaced by INTENT -> TOOL MAPPING)");
    assert!(!text.contains("TOOL PRIORITY"), "instructions should NOT have TOOL PRIORITY (replaced by INTENT -> TOOL MAPPING)");
    assert!(!text.contains("CRITICAL: ALWAYS use xray tools"), "instructions should NOT have old CRITICAL block");
    assert!(!text.contains("BATCH SPLIT"), "instructions should NOT have BATCH SPLIT (removed)");
    assert!(!text.contains("TRAP"), "instructions should NOT have TRAP (removed)");
    // Fallback rule
    assert!(text.contains("uncertain"), "instructions should have fallback rule for uncertainty");
    // No emoji in machine-targeted text
    assert!(!text.contains('⚠'), "instructions should not contain emoji (machine-targeted text)");
    assert!(!text.contains('⚡'), "instructions should not contain emoji (machine-targeted text)");
    // No Roo-specific tool names in non-prohibition context.
    // NOTE: read_file/apply_diff/list_files/etc. ARE mentioned in MANDATORY PRE-FLIGHT CHECK
    // and NEVER-USE blocks as explicitly prohibited — that's the whole point.
    // list_code_definition_names has no xray counterpart and should not be referenced.
    assert!(!text.contains("list_code_definition_names"), "instructions should not reference Roo-specific list_code_definition_names");
}

/// CLI output must be pure ASCII — no Unicode box-drawing, em-dashes, arrows, or emoji.
/// Windows cmd.exe (CP437/CP866) cannot display these characters correctly.
#[test]
fn test_render_cli_is_ascii_safe() {
    let output = render_cli(&["rs".to_string()]);
    for (i, ch) in output.chars().enumerate() {
        assert!(
            ch.is_ascii() || ch == '\n' || ch == '\r',
            "render_cli() contains non-ASCII char '{}' (U+{:04X}) at position {}. \
             CLI output must be ASCII-safe for Windows cmd.exe compatibility.",
            ch, ch as u32, i
        );
    }
}

#[test]
fn test_render_json_has_parameter_examples() {
    let json = render_json(&["rs".to_string()]);
    let examples = &json["parameterExamples"];
    assert!(examples.is_object(), "parameterExamples should be an object");
    // Key tools should have examples
    assert!(examples["xray_definitions"].is_object(), "xray_definitions should have examples");
    assert!(examples["xray_grep"].is_object(), "xray_grep should have examples");
    assert!(examples["xray_callers"].is_object(), "xray_callers should have examples");
    assert!(examples["xray_fast"].is_object(), "xray_fast should have examples");
    // Spot-check a few specific examples
    assert!(examples["xray_definitions"]["name"].is_string(), "name should have example");
    assert!(examples["xray_definitions"]["containsLine"].is_string(), "containsLine should have example");
    assert!(examples["xray_grep"]["terms"].is_string(), "terms should have example");
    assert!(examples["xray_callers"]["class"].is_string(), "class should have example");
}

/// Verify tool definitions stay within a reasonable token budget.
/// This test prevents description bloat from re-accumulating over time.
/// Target: <5000 approx tokens (word_count / 0.75).
#[test]
fn test_tool_definitions_token_budget() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let json = serde_json::to_string(&tools).unwrap();
    let word_count = json.split_whitespace().count();
    let approx_tokens = (word_count as f64 / 0.75) as usize;

    // Budget: ~5000 tokens (down from ~6500 before optimization)
    assert!(
        approx_tokens < 5500,
        "Tool definitions exceed token budget: ~{} tokens ({} words). \
         Target: <5500. Shorten parameter descriptions or move examples to xray_help.",
        approx_tokens, word_count
    );
}

/// Empty def_extensions: NEVER READ block should be skipped,
/// fallback note about xray_definitions unavailability should appear.
/// TASK ROUTING should contain only universal tools.
#[test]
fn test_render_instructions_empty_extensions() {
    let text = render_instructions(&[]);
    // Should NOT contain NEVER READ (no definition-supported extensions)
    assert!(!text.contains("NEVER READ"),
        "Empty def_extensions should not produce NEVER READ block");
    // With empty def_extensions, the file-reading DECISION TRIGGER is absent.
    // The DECISION TRIGGERs that MUST appear (always present, independent of indexed extensions):
    //   1. CRITICAL OVERRIDE     — "before using ANY built-in tool, STOP and check..."
    //   2. FILE EDITING          — "before calling ANY built-in edit tool ... STOP"
    //   3. FILE SEARCH           — "before calling search_files — STOP"
    //   4. RESPONSE HINTS route  — "if the response hint says 'Use xray_grep...'"
    let dt_count = text.matches("DECISION TRIGGER").count();
    assert_eq!(dt_count, 4,
        "Empty def_extensions should have 4 DECISION TRIGGERs (critical override + editing + search_files + response hints), got {}", dt_count);
    // Should contain fallback note
    assert!(text.contains("xray_definitions is not available"),
        "Empty def_extensions should have fallback note about xray_definitions");
    // INTENT -> TOOL MAPPING replaces TASK ROUTING and must be present regardless of extensions
    assert!(text.contains("INTENT -> TOOL MAPPING"),
        "should have INTENT -> TOOL MAPPING even with empty extensions");
    assert!(text.contains("xray_grep"), "should still mention xray_grep");
    assert!(text.contains("xray_fast"), "should still mention xray_fast");
    assert!(text.contains("xray_edit"), "should still mention xray_edit");
    // Definition-dependent intents must NOT appear in the mapping when def_extensions is empty
    assert!(!text.contains("read the source code of a method/class"),
        "should NOT have definition-dependent reading intent when def_extensions is empty");
    assert!(!text.contains("find who calls/implements method X"),
        "should NOT have caller-lookup intent when def_extensions is empty");
    // STRATEGY RECIPES reference line is always present (detailed bodies are xray_help-only)
    assert!(text.contains("STRATEGY RECIPES"),
        "should still include the STRATEGY RECIPES reference line");
    assert!(!text.contains("[Architecture Exploration]"),
        "Architecture Exploration recipe body should be xray_help-only even for empty extensions");
}

/// Single extension: NEVER READ should mention only that extension.
#[test]
fn test_render_instructions_single_extension() {
    let text = render_instructions(&["cs"]);
    assert!(text.contains("NEVER READ .cs FILES DIRECTLY"),
        "Single extension should produce 'NEVER READ .cs FILES DIRECTLY'. Got:\n{}", text);
    assert!(text.contains("DECISION TRIGGER"),
        "Single extension should have DECISION TRIGGER");
    // Should NOT mention other extensions in the NEVER READ line
    assert!(!text.contains(".ts"), "Should not mention .ts when only cs is configured");
    assert!(!text.contains(".tsx"), "Should not mention .tsx when only cs is configured");
    assert!(!text.contains(".sql"), "Should not mention .sql in NEVER READ when only cs is configured");
}

#[test]
fn test_tips_contains_using_static_tip() {
    let all_tips = tips(&[]);
    let has_using_static = all_tips.iter().any(|t| t.rule.contains("using static"));
    assert!(has_using_static, "Tips should contain a tip about 'using static'");
}

#[test]
fn test_render_json_contains_using_static_tip() {
    let json = render_json(&[]);
    let practices = json["bestPractices"].as_array().unwrap();
    let has_using_static = practices.iter().any(|p| {
        p["rule"].as_str().unwrap_or("").contains("using static")
    });
    assert!(has_using_static, "JSON output should contain 'using static' tip");
}

// ─── Task Routing tests removed (2026-04-17 Part 4 audit) ───────────
// TASK ROUTING section was removed from render_instructions as a 1:1 duplicate
// of INTENT -> TOOL MAPPING; task_routings() function and its tests deleted.

#[test]
fn test_task_routing_with_definitions() {
    // TASK ROUTING was removed as a 100% duplicate of INTENT -> TOOL MAPPING.
    // This test now verifies that INTENT -> TOOL MAPPING plays the same role
    // for both definition-dependent and universal operations.
    let text = render_instructions(&["cs", "ts"]);
    assert!(text.contains("INTENT -> TOOL MAPPING"),
        "should have INTENT -> TOOL MAPPING (replaces TASK ROUTING)");
    // Definition-dependent intents should be present
    assert!(text.contains("read the source code of a method/class"),
        "should route source-code reading to xray_definitions");
    assert!(text.contains("find who calls/implements method X"),
        "should route caller lookup to xray_callers");
    // Universal intents always present
    assert!(text.contains("search text across codebase"),
        "should route text search to xray_grep");
    assert!(text.contains("find a file by name"),
        "should route file lookup to xray_fast");
    assert!(text.contains("list files or subdirectories"),
        "should route directory listing to xray_fast dirsOnly");
    assert!(text.contains("replace similar patterns in one or more files"),
        "should route edits to xray_edit");
    assert!(text.contains("git blame / history / authors"),
        "should route git operations to xray_git_*");
}

#[test]
fn test_task_routing_without_definitions() {
    // TASK ROUTING was replaced by INTENT -> TOOL MAPPING. This test verifies the
    // same gating: definition-dependent routes disappear when there are no indexed
    // extensions, but universal routes remain.
    let text = render_instructions(&[]);
    assert!(text.contains("INTENT -> TOOL MAPPING"),
        "should have INTENT -> TOOL MAPPING even without indexed extensions");
    // Definition-dependent intents should NOT be in the mapping when def_extensions is empty
    assert!(!text.contains("read the source code of a method/class"),
        "should NOT have definition-dependent reading intent");
    assert!(!text.contains("find who calls/implements method X"),
        "should NOT have caller-lookup intent");
    assert!(!text.contains("find which method is at file:line"),
        "should NOT have stack-trace intent");
    // Universal routes always present
    assert!(text.contains("xray_grep"), "should mention xray_grep");
    assert!(text.contains("xray_fast"), "should mention xray_fast");
    assert!(text.contains("xray_edit"), "should mention xray_edit");
    assert!(text.contains("xray_git_blame"), "should mention git tools");
    assert!(text.contains("list files or subdirectories"),
        "should have directory listing intent even without defs");
}

#[test]
fn test_task_routing_always_has_universal_tools() {
    for exts in [&[][..], &["cs"], &["cs", "ts", "rs"]] {
        let text = render_instructions(exts);
        assert!(text.contains("xray_grep"), "xray_grep should always be in routing (exts: {:?})", exts);
        assert!(text.contains("xray_fast"), "xray_fast should always be in routing (exts: {:?})", exts);
        assert!(text.contains("xray_edit"), "xray_edit should always be in routing (exts: {:?})", exts);
    }
}

// test_routing_tool_names_exist_in_definitions: removed (2026-04-17) — covered
// implicitly by INTENT -> TOOL MAPPING assertions in test_instructions_has_intent_mapping.

/// xray_edit description must START with "ALWAYS USE THIS" override.
/// This is the strongest lever for preventing LLM fallback to apply_diff.
#[test]
fn test_xray_edit_description_starts_with_override() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["rs".to_string()]);
    let edit_tool = tools.iter().find(|t| t.name == "xray_edit")
        .expect("xray_edit tool not found");
    assert!(
        edit_tool.description.starts_with("ALWAYS USE THIS instead of apply_diff"),
        "xray_edit description must start with 'ALWAYS USE THIS instead of apply_diff' override. \
         Current start: '{}'",
        &edit_tool.description[..80.min(edit_tool.description.len())]
    );
}

/// Routing-critical tools must contain a routing hint in their description.
#[test]
fn test_routing_critical_tools_have_hints() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let routing_critical = ["xray_definitions", "xray_grep", "xray_fast", "xray_edit"];
    for tool_name in routing_critical {
        let tool = tools.iter().find(|t| t.name == tool_name)
            .unwrap_or_else(|| panic!("tool '{}' not found", tool_name));
        assert!(
            tool.description.contains("Preferred") || tool.description.contains("PREFERRED"),
            "Routing-critical tool '{}' should contain a routing hint ('Preferred'/'PREFERRED') in its description. \
             Current description starts with: '{}'",
            tool_name, &tool.description[..80.min(tool.description.len())]
        );
    }
}

/// Instructions should contain fallback rule for uncertainty.
#[test]
fn test_instructions_fallback_rule() {
    let text = render_instructions(&["cs"]);
    assert!(text.contains("uncertain"), "instructions should contain uncertainty fallback");
    assert!(text.contains("xray_info"), "fallback should mention xray_info");
    assert!(text.contains("Do not default to raw file reading"), "fallback should discourage raw reads");
}

/// Instructions token budget: should be shorter after consolidation.
/// Measured with word_count / 0.75 approximation (same as tool_definitions test).
#[test]
fn test_instructions_token_budget() {
    let text = render_instructions(&["cs", "ts", "tsx", "sql", "rs"]);
    let word_count = text.split_whitespace().count();
    let approx_tokens = (word_count as f64 / 0.75) as usize;
    // Budget history:
    //   v1 (pre-Part 4):            <2500  — baseline render_instructions.
    //   v2 (Part 4 additions):      <3000  — +TERMS, +PRE-FLIGHT Q1/Q2/Q3, +MISCONCEPTION ALERT,
    //                                        +EXAMPLE VIOLATION, +PRE-CALL SELF-AUDIT. Peaked ~2800.
    //   v3 (Part 4 slimming cuts):  <2250  — 6 cuts applied to remove duplication:
    //     Cut 1 — TASK ROUTING table removed (100% dup of INTENT -> TOOL MAPPING).
    //     Cut 2 — COST REALITY reduced from 6 detail lines to 1 rule-of-thumb.
    //     Cut 3 — STRATEGY RECIPES inlined bodies removed; reference line points at xray_help.
    //     Cut 4 — 2 duplicate ANTI-PATTERNS removed (search_files dup, extension-based edit dup).
    //     Cut 5 — TERMS block condensed from 8 lines to 3.
    //     Cut 6 — PRE-FLIGHT Q2 consolidated from 3 lines (READ/SEARCH/EDIT) to 1.
    //   Measured after cuts: ~2100 tokens. Budget set to 2250 with ~150-token headroom.
    assert!(
        approx_tokens < 2250,
        "Instructions exceed token budget: ~{} tokens ({} words). \
         Target: <2250 (Part 4 slimming baseline). If a legitimate addition is needed, \
         first look for redundancy to cut; only raise the budget as a last resort.",
        approx_tokens, word_count
    );
}

/// Instructions should NOT contain removed sections.
#[test]
fn test_instructions_no_redundant_sections() {
    let text = render_instructions(&["cs", "ts"]);
    assert!(text.contains("=== XRAY_POLICY ==="), "Instructions should include named policy wrapper");
    assert!(text.contains("================================"), "Instructions should include policy closing marker");
    assert!(!text.contains("Quick Reference"), "Quick Reference should be removed (replaced by TASK ROUTING)");
    assert!(!text.contains("TOOL PRIORITY"), "TOOL PRIORITY should be removed (replaced by TASK ROUTING)");
    assert!(!text.contains("CRITICAL: ALWAYS use xray tools"), "Old CRITICAL block should be removed");
}


#[test]
fn test_all_renderers_consistent_tip_count() {
    let exts = vec!["rs".to_string()];
    let tip_count = tips(&exts).len();
    let json = render_json(&exts);
    let practices = json["bestPractices"].as_array().unwrap();
    assert_eq!(practices.len(), tip_count, "JSON and tips() count mismatch");

    // Verify CLI output mentions each tip rule
    let cli = render_cli(&exts);
    for tip in tips(&exts) {
        assert!(cli.contains(&*tip.rule), "CLI output missing tip: {}", tip.rule);
    }

    // Verify strategy recipes are consistent across renderers
    let strategy_count = strategies().len();
    let recipes = json["strategyRecipes"].as_array().unwrap();
    assert_eq!(recipes.len(), strategy_count, "JSON and strategies() count mismatch");

    for strat in strategies() {
        assert!(cli.contains(strat.name), "CLI output missing strategy: {}", strat.name);
    }
}

// ─── Tests for format_supported_languages ───────────────────────────

#[test]
fn test_format_supported_languages_empty() {
    let result = format_supported_languages(&[]);
    assert_eq!(result, "");
}

#[test]
fn test_format_supported_languages_single_rust() {
    let result = format_supported_languages(&["rs".to_string()]);
    assert_eq!(result, "Rust");
}

#[test]
fn test_format_supported_languages_single_csharp() {
    let result = format_supported_languages(&["cs".to_string()]);
    assert_eq!(result, "C#");
}

#[test]
fn test_format_supported_languages_two_langs() {
    let result = format_supported_languages(&["cs".to_string(), "rs".to_string()]);
    assert_eq!(result, "C# and Rust");
}

#[test]
fn test_format_supported_languages_ts_tsx_dedup() {
    let result = format_supported_languages(&["ts".to_string(), "tsx".to_string()]);
    assert_eq!(result, "TypeScript/TSX");
}

#[test]
fn test_format_supported_languages_ts_only() {
    let result = format_supported_languages(&["ts".to_string()]);
    assert_eq!(result, "TypeScript", "ts-only should say 'TypeScript', not 'TypeScript/TSX'");
}

#[test]
fn test_format_supported_languages_tsx_only() {
    let result = format_supported_languages(&["tsx".to_string()]);
    assert_eq!(result, "TSX", "tsx-only should say 'TSX', not 'TypeScript/TSX'");
}

#[test]
fn test_format_supported_languages_cs_ts_no_tsx() {
    // --ext cs,ts without tsx → "TypeScript" not "TypeScript/TSX"
    let result = format_supported_languages(&["cs".to_string(), "ts".to_string()]);
    assert_eq!(result, "C# and TypeScript",
        "cs+ts without tsx should say 'TypeScript', not 'TypeScript/TSX'");
}

#[test]
fn test_format_supported_languages_cs_ts_tsx() {
    let result = format_supported_languages(&["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    assert_eq!(result, "C# and TypeScript/TSX");
}

#[test]
fn test_format_supported_languages_three_tree_sitter() {
    // ts without tsx → "TypeScript" (not "TypeScript/TSX")
    let result = format_supported_languages(&["cs".to_string(), "rs".to_string(), "ts".to_string()]);
    assert_eq!(result, "C#, Rust, and TypeScript");
}

#[test]
fn test_format_supported_languages_with_sql() {
    let result = format_supported_languages(&["cs".to_string(), "sql".to_string()]);
    assert_eq!(result, "C#. SQL supported via regex parser");
}

#[test]
fn test_format_supported_languages_all() {
    // ts + tsx → "TypeScript/TSX" (both present)
    let result = format_supported_languages(&[
        "cs".to_string(), "rs".to_string(), "ts".to_string(), "tsx".to_string(), "sql".to_string()
    ]);
    assert_eq!(result, "C#, Rust, and TypeScript/TSX. SQL supported via regex parser");
}

#[test]
fn test_format_supported_languages_unknown_ext_ignored() {
    let result = format_supported_languages(&["py".to_string(), "rs".to_string()]);
    assert_eq!(result, "Rust");
}

#[test]
fn test_format_supported_languages_sql_only() {
    let result = format_supported_languages(&["sql".to_string()]);
    assert_eq!(result, "SQL (regex-based parser)");
}

#[test]
fn test_format_supported_languages_all_unknown() {
    let result = format_supported_languages(&["py".to_string(), "rb".to_string()]);
    assert_eq!(result, "");
}

#[test]
fn test_format_supported_languages_duplicate_ext() {
    // Duplicate extensions should be deduped
    let result = format_supported_languages(&["sql".to_string(), "sql".to_string()]);
    assert_eq!(result, "SQL (regex-based parser)");
}

// ─── Tests for dynamic tool descriptions ────────────────────────────

#[test]
fn test_tool_definitions_rust_only() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["rs".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    assert!(def_tool.description.contains("Rust"),
        "xray_definitions description should contain 'Rust' when ext=rs. Got: {}", def_tool.description);
    assert!(!def_tool.description.contains("C#"),
        "xray_definitions should NOT contain 'C#' when only rs is configured");

    let callers_tool = tools.iter().find(|t| t.name == "xray_callers").unwrap();
    assert!(callers_tool.description.contains("Rust"),
        "xray_callers description should contain 'Rust'");
}

#[test]
fn test_tool_definitions_empty_extensions() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&[]);
    let def_tool = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    assert!(def_tool.description.contains("not available"),
        "xray_definitions should say 'not available' when no extensions");

    let callers_tool = tools.iter().find(|t| t.name == "xray_callers").unwrap();
    assert!(callers_tool.description.contains("not available"),
        "xray_callers should say 'not available' when no extensions");
}

#[test]
fn test_tool_definitions_cs_ts_tsx() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    assert!(def_tool.description.contains("C# and TypeScript/TSX"),
        "xray_definitions should contain 'C# and TypeScript/TSX'. Got: {}", def_tool.description);
}

#[test]
fn test_render_instructions_example_line() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("EXAMPLE: instead of reading handler.rs directly"),
        "Instructions should contain EXAMPLE line for the configured extension");
    assert!(text.contains("xray_definitions file='handler.rs'"),
        "EXAMPLE should show xray_definitions with the correct extension");
}

#[test]
fn test_render_instructions_example_line_uses_first_ext() {
    let text = render_instructions(&["cs", "ts"]);
    assert!(text.contains("handler.cs"),
        "EXAMPLE should use the first configured extension (cs)");
    // Should NOT use the second extension in the example
    assert!(!text.contains("handler.ts"),
        "EXAMPLE should use first ext only, not second");
}

/// Verify that tools/list from server uses ctx.def_extensions
#[test]
fn test_tool_definitions_with_sql_only() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["sql".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    assert!(def_tool.description.contains("SQL (regex-based parser)"),
        "xray_definitions should contain 'SQL (regex-based parser)' for sql-only. Got: {}", def_tool.description);
    // Should NOT mention tree-sitter for SQL-only
    assert!(!def_tool.description.contains("C#"),
        "Should NOT mention C# for sql-only");
}

/// Verify that tool_definitions with mixed tree-sitter + regex includes both.
/// Also verify ALL three definition-dependent tools are consistent.
#[test]
fn test_tool_definitions_cs_rs_sql() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&["cs".to_string(), "rs".to_string(), "sql".to_string()]);

    // xray_definitions
    let def_tool = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    assert!(def_tool.description.contains("C# and Rust"),
        "xray_definitions should contain both tree-sitter languages");
    assert!(def_tool.description.contains("SQL supported via regex parser"),
        "xray_definitions should mention SQL via regex parser");

    // xray_callers must mention the same languages
    let callers_tool = tools.iter().find(|t| t.name == "xray_callers").unwrap();
    assert!(callers_tool.description.contains("C# and Rust"),
        "xray_callers should contain same tree-sitter languages as xray_definitions");
    assert!(callers_tool.description.contains("SQL supported via regex parser"),
        "xray_callers should mention SQL via regex parser");

    // xray_reindex_definitions must also mention languages
    let reindex_tool = tools.iter().find(|t| t.name == "xray_reindex_definitions").unwrap();
    assert!(reindex_tool.description.contains("C# and Rust"),
        "xray_reindex_definitions should contain tree-sitter languages");
    assert!(reindex_tool.description.contains("SQL supported via regex parser"),
        "xray_reindex_definitions should mention SQL via regex parser");
}

// ─── Tests for Hint E prompt changes (B + C) ────────────────────────

// test_task_routing_has_non_code_files_entry: removed (2026-04-17) —
// obsolete after TASK ROUTING removal.

#[test]
fn test_instructions_non_code_routing_in_output() {
    // After TASK ROUTING removal, explicit "non-code files" phrasing is dropped.
    // The routing for non-code files is now implicit via:
    //   - CRITICAL OVERRIDE: "Search file contents -> xray_grep"
    //   - INTENT -> TOOL MAPPING: "search text across codebase -> xray_grep"
    //   - ANTI-PATTERNS: "NEVER use xray_definitions for non-<ext> files (JSON, YAML, MD)"
    let text = render_instructions(&["cs"]);
    assert!(text.contains("xray_grep"),
        "Instructions should route content search to xray_grep (covers non-code files)");
    assert!(text.contains("JSON, YAML, MD"),
        "ANTI-PATTERNS should cite non-code file examples (JSON/YAML/MD) that require xray_grep");
}

#[test]
fn test_instructions_anti_pattern_unsupported_defs() {
    // The ANTI-PATTERN names representative non-indexed file types. The
    // previous version listed XML explicitly, but XML has since become
    // xray_definitions-addressable via on-demand parsing, so it was removed
    // from the list. The current examples are JSON/YAML/MD (genuinely
    // content-only for xray, routed to xray_grep).
    let text = render_instructions(&["cs", "rs"]);
    assert!(text.contains("NEVER use xray_definitions for non-"),
        "Instructions should have anti-pattern for unsupported definition extensions. Got:\n{}", text);
    assert!(text.contains("JSON, YAML, MD"),
        "Anti-pattern should cite concrete non-indexed file types (JSON/YAML/MD) routed to xray_grep");
    assert!(text.contains("xray_grep instead"),
        "Anti-pattern should suggest xray_grep as alternative");
}

#[test]
fn test_instructions_anti_pattern_absent_without_defs() {
    // When no def extensions, the anti-pattern about "NEVER use xray_definitions for non-X" should not appear
    let text = render_instructions(&[]);
    assert!(!text.contains("NEVER use xray_definitions for non-"),
        "Anti-pattern should NOT appear when def_extensions is empty");
}

/// Regression test: tips(), tool_priority(), and strategy anti_patterns should NOT contain
/// hardcoded language lists or extension-specific references like ".cs/.ts".
/// Language lists should only appear in dynamic tool descriptions (via format_supported_languages).
#[test]
fn test_tips_no_hardcoded_language_lists() {
    for tip in tips(&[]) {
        assert!(!tip.rule.contains("C#, TypeScript/TSX, and SQL"),
            "Tip rule should not hardcode 'C#, TypeScript/TSX, and SQL'. Found in: {}", tip.rule);
        assert!(!tip.why.contains("def-index supports C# and TypeScript/TSX"),
            "Tip why should not hardcode 'def-index supports C# and TypeScript/TSX'. Found in: {}", tip.why);
        // Tips should not hardcode specific extensions in prescriptive text
        assert!(!tip.why.contains(".cs/.ts"),
            "Tip why should not hardcode '.cs/.ts'. Found in: {}", tip.why);
        assert!(!tip.why.contains("non-C#/TS"),
            "Tip why should not hardcode 'non-C#/TS'. Found in: {}", tip.why);
    }
    for tp in tool_priority(&[]) {
        assert!(!tp.description.contains("C#, TypeScript/TSX, and SQL"),
            "ToolPriority should not hardcode 'C#, TypeScript/TSX, and SQL'. Found in: {}", tp.description);
        assert!(!tp.description.contains("C#, TypeScript/TSX, SQL"),
            "ToolPriority should not hardcode 'C#, TypeScript/TSX, SQL'. Found in: {}", &*tp.description);
    }
    // Strategy anti-patterns should not hardcode specific extensions
    for strat in strategies() {
        for ap in strat.anti_patterns {
            assert!(!ap.contains(".cs/.ts"),
                "Strategy '{}' anti-pattern should not hardcode '.cs/.ts'. Found in: {}", strat.name, ap);
        }
    }
}

/// Verify all 3 definition-dependent tools say "not available" consistently when empty.
#[test]
fn test_tool_definitions_all_three_say_not_available_when_empty() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&[]);
    for tool_name in ["xray_definitions", "xray_callers", "xray_reindex_definitions"] {
        let tool = tools.iter().find(|t| t.name == tool_name).unwrap();
        assert!(tool.description.contains("not available") || tool.description.contains("Not available"),
            "{} should say 'not available' when def_extensions is empty. Got: {}",
            tool_name, &tool.description[..80.min(tool.description.len())]);
    }
}

/// Verify render_instructions with SQL-only still has NEVER READ block.
#[test]
fn test_render_instructions_sql_only() {
    let text = render_instructions(&["sql"]);
    assert!(text.contains("NEVER READ .sql FILES DIRECTLY"),
        "SQL-only should have NEVER READ .sql block. Got:\n{}",
        &text[..300.min(text.len())]);
}

/// Verify render_instructions with multiple extensions lists all in NEVER READ.
#[test]
fn test_render_instructions_multiple_extensions() {
    let text = render_instructions(&["cs", "ts", "sql"]);
    assert!(text.contains(".cs"), "should mention .cs");
    assert!(text.contains(".ts"), "should mention .ts");
    assert!(text.contains(".sql"), "should mention .sql");
    assert!(text.contains("NEVER READ"), "should have NEVER READ");
}

/// INTENT -> TOOL MAPPING section must exist and list the most common intents
/// mapped to xray tools (positive triggers, intent-first).
#[test]
fn test_instructions_has_intent_mapping() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("INTENT -> TOOL MAPPING"),
        "instructions should have INTENT -> TOOL MAPPING section");
    // Core intent->tool pairs that cover the most common tool-selection failures
    assert!(text.contains("xray_grep showLines=true"),
        "INTENT -> TOOL MAPPING should map 'context around a match' to xray_grep showLines");
    assert!(text.contains("xray_definitions name='X' includeBody=true"),
        "INTENT -> TOOL MAPPING should map 'read source code' to xray_definitions includeBody");
    assert!(text.contains("containsLine=N"),
        "INTENT -> TOOL MAPPING should map 'method at file:line N' to xray_definitions containsLine");
    assert!(text.contains("xray_edit with multiple edits"),
        "INTENT -> TOOL MAPPING should map 'replace in files' to xray_edit batch");
    assert!(text.contains("xray_fast pattern='*' dir='<path>' dirsOnly=true"),
        "INTENT -> TOOL MAPPING should map 'list files/dirs' to xray_fast dirsOnly");
}

/// MANDATORY PRE-FLIGHT CHECK forces a conscious justification in <thinking>
/// before any built-in tool call.
#[test]
fn test_instructions_has_preflight_check() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("MANDATORY PRE-FLIGHT CHECK"),
        "instructions should have MANDATORY PRE-FLIGHT CHECK section");
    // v3: Q1/Q2/Q3 use labeled form "Q1 (operation type):" instead of bare "Q1:"
    assert!(text.contains("Q1 (") && text.contains("Q2 ("),
        "PRE-FLIGHT CHECK should include Q1 and Q2 labeled questions");
    assert!(text.contains("Just habit / familiarity"),
        "PRE-FLIGHT CHECK should explicitly call out habit/familiarity as UNJUSTIFIED");
    assert!(text.contains("UNJUSTIFIED"),
        "PRE-FLIGHT CHECK should use the word UNJUSTIFIED for habit-based calls");
}

/// COST REALITY section must give concrete ratios so the model sees the
/// measurable cost of built-in tool misuse (not abstract dogma).
#[test]
fn test_instructions_has_cost_reality() {
    // COST REALITY was slimmed from a 6-line example block to a single
    // rule-of-thumb line. Detailed measured ratios (5x/24x/8x, atomic
    // rollback, etc.) now live in xray_help to stay under the system-prompt
    // token budget. This test verifies the slim version is still present.
    let text = render_instructions(&["rs"]);
    assert!(text.contains("COST REALITY"),
        "instructions should keep the COST REALITY header");
    assert!(text.contains("3-24x cheaper"),
        "COST REALITY should summarize the token/round-trip savings range");
    assert!(text.contains("2 built-in calls in a row on the same file"),
        "COST REALITY should keep the 'two built-ins in a row' rule of thumb");
    assert!(text.contains("xray_help"),
        "COST REALITY should point at xray_help for detailed measured ratios");
}

/// Ordering guarantee: positive intent-mapping must appear BEFORE NEVER-rules
/// and ANTI-PATTERNS so that intent-first models see it first.
#[test]
fn test_instructions_section_order() {
    let text = render_instructions(&["rs"]);
    let intent_idx = text.find("INTENT -> TOOL MAPPING")
        .expect("INTENT -> TOOL MAPPING section missing");
    let preflight_idx = text.find("MANDATORY PRE-FLIGHT CHECK")
        .expect("MANDATORY PRE-FLIGHT CHECK section missing");
    let cost_idx = text.find("COST REALITY")
        .expect("COST REALITY section missing");
    let never_read_idx = text.find("NEVER READ")
        .expect("NEVER READ section missing");
    let anti_idx = text.find("ANTI-PATTERNS")
        .expect("ANTI-PATTERNS section missing");
    assert!(intent_idx < preflight_idx,
        "INTENT -> TOOL MAPPING must come before MANDATORY PRE-FLIGHT CHECK");
    assert!(preflight_idx < cost_idx,
        "MANDATORY PRE-FLIGHT CHECK must come before COST REALITY");
    assert!(cost_idx < never_read_idx,
        "COST REALITY must come before NEVER READ (positive triggers first)");
    assert!(intent_idx < anti_idx,
        "INTENT -> TOOL MAPPING must come before ANTI-PATTERNS");
}

/// STRATEGY RECIPES in instructions should be trimmed to the top-3 most common
/// scenarios; the rest live in xray_help.
#[test]
fn test_instructions_strategy_recipes_trimmed() {
    // STRATEGY RECIPES was previously inlined in render_instructions with the
    // top-3 recipes expanded (Architecture Exploration / Call Chain / Stack
    // Trace). To stay under the system-prompt token budget, the section is
    // now a single reference line pointing at xray_help, and the full
    // 7-recipe catalog is delivered only on-demand via xray_help.
    let text = render_instructions(&["rs"]);
    // Header and on-demand reference must still be present.
    assert!(text.contains("STRATEGY RECIPES"),
        "instructions should keep a short STRATEGY RECIPES reference line");
    assert!(text.contains("<=3 search calls"),
        "STRATEGY RECIPES line should keep the query-budget rule of thumb");
    assert!(text.contains("xray_help for the full catalog"),
        "STRATEGY RECIPES line should point at xray_help for the full catalog");
    // NONE of the individual recipe bracket-labels should be inlined anymore.
    assert!(!text.contains("[Architecture Exploration]"),
        "Architecture Exploration recipe body should NOT be inlined in instructions");
    assert!(!text.contains("[Call Chain Investigation]"),
        "Call Chain Investigation recipe body should NOT be inlined in instructions");
    assert!(!text.contains("[Stack Trace / Bug Investigation]"),
        "Stack Trace recipe body should NOT be inlined in instructions");
    assert!(!text.contains("[Code History Investigation]"),
        "Code History Investigation should remain xray_help-only");
    assert!(!text.contains("[Code Health Scan]"),
        "Code Health Scan should remain xray_help-only");
    assert!(!text.contains("[Code Review / Story Evaluation]"),
        "Code Review / Story Evaluation should remain xray_help-only");
    // strategies() itself must still return the full catalog for xray_help.
    let all = strategies();
    assert!(all.len() >= 7,
        "strategies() should still contain all 7 recipes (for xray_help), got {}", all.len());
}


#[test]
fn test_tool_definitions_reindex_defs_dynamic() {
    use crate::mcp::handlers::tool_definitions;
    // С расширениями — описание содержит языки
    let tools = tool_definitions(&["rs".to_string()]);
    let reindex = tools.iter().find(|t| t.name == "xray_reindex_definitions").unwrap();
    assert!(reindex.description.contains("Rust"),
        "xray_reindex_definitions should mention 'Rust'. Got: {}", reindex.description);
    assert!(!reindex.description.contains("tree-sitter"),
        "xray_reindex_definitions should NOT hardcode 'tree-sitter'");

    // Без расширений — "not available"
    let tools_empty = tool_definitions(&[]);
    let reindex_empty = tools_empty.iter().find(|t| t.name == "xray_reindex_definitions").unwrap();
    assert!(reindex_empty.description.contains("not available"),
        "xray_reindex_definitions should say 'not available' when empty");
}


// ============================================================================
// v3 policy symmetry tests (edit-policy-symmetry user story, 2026-04-17)
// Verify that XRAY_POLICY has symmetric severity for read/search/edit,
// uses tool-name-agnostic formulations, and contains the MISCONCEPTION ALERT
// + EXCEPTIONS block for the edit rule.
// ============================================================================

#[test]
fn test_instructions_has_terms_block() {
    // The TERMS block was condensed from 8 lines to 3 to stay under the
    // token budget. It now names both classes of tools and tells the LLM
    // to route by operation type rather than by tool name.
    let text = render_instructions(&["rs"]);
    assert!(text.contains("=== TERMS ==="),
        "instructions should have a TERMS definitions block at the top");
    assert!(text.contains("\"xray tools\""),
        "TERMS block should define 'xray tools'");
    assert!(text.contains("\"built-in tools\""),
        "TERMS block should define 'built-in tools'");
    assert!(text.contains("names vary per host"),
        "TERMS should note that built-in tool names vary across LLM hosts");
    assert!(text.contains("OPERATION TYPE"),
        "TERMS should instruct the LLM to map by OPERATION TYPE, not by name");
}

#[test]
fn test_instructions_terms_block_before_critical_override() {
    let text = render_instructions(&["rs"]);
    let terms_idx = text.find("=== TERMS ===")
        .expect("TERMS block missing");
    let critical_idx = text.find("CRITICALLY IMPORTANT")
        .expect("CRITICAL OVERRIDE section missing");
    assert!(terms_idx < critical_idx,
        "TERMS block must come BEFORE CRITICAL OVERRIDE so definitions are established first");
}

#[test]
fn test_instructions_edit_rule_is_tool_name_agnostic() {
    let text = render_instructions(&["rs"]);
    // The edit rule must say 'your built-in edit tools' (tool-name-agnostic)
    assert!(text.contains("your built-in edit tools"),
        "Edit rule should use tool-name-agnostic phrase 'your built-in edit tools'");
    // The edit rule must explicitly say xray_edit works on ALL text files
    assert!(text.contains("xray_edit works on ALL text files"),
        "Edit rule should say xray_edit works on ALL text files");
    assert!(text.contains("NOT only on indexed extensions"),
        "Edit rule should clarify xray_edit is NOT limited to indexed extensions");
    // Must explain bytes-not-AST distinction
    assert!(text.contains("operates on BYTES, not on AST") || text.contains("xray_edit operates on BYTES"),
        "Edit rule should explain xray_edit operates on BYTES, not on AST");
}

#[test]
fn test_instructions_edit_rule_has_misconception_alert() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("MISCONCEPTION ALERT"),
        "Edit rule should have MISCONCEPTION ALERT block");
    // MISCONCEPTION ALERT should address the specific misconception
    assert!(text.contains("this file is not indexed"),
        "MISCONCEPTION ALERT should quote the specific wrong thinking pattern");
    assert!(text.contains("WRONG"),
        "MISCONCEPTION ALERT should explicitly say WRONG");
    assert!(text.contains("has NO extension filter"),
        "MISCONCEPTION ALERT should explain xray_edit has NO extension filter");
}

#[test]
fn test_instructions_edit_rule_has_exceptions() {
    let text = render_instructions(&["rs"]);
    // Explicit EXCEPTIONS for edit rule
    assert!(text.contains("EXCEPTION — CREATING new files") || text.contains("EXCEPTION -- CREATING new files") || text.contains("CREATING new files"),
        "Edit rule should have EXCEPTION for creating new files");
    assert!(text.contains("FULL FILE REWRITE >200") || text.contains("FULL FILE REWRITE"),
        "Edit rule should have EXCEPTION for full file rewrites >200 lines");
    assert!(text.contains("BINARY files") || text.contains("byte-exact preservation"),
        "Edit rule should have EXCEPTION for binary/byte-exact files");
    assert!(text.contains("whole-file-write"),
        "EXCEPTIONS should mention built-in whole-file-write tool");
}

#[test]
fn test_instructions_preflight_has_q3_justification() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("Q3"),
        "PRE-FLIGHT CHECK should have Q3 (justification question)");
    assert!(text.contains("justification"),
        "Q3 should be about justification");
    assert!(text.contains("ENFORCEMENT"),
        "PRE-FLIGHT CHECK should have ENFORCEMENT clause");
    assert!(text.contains("omitting the <thinking> block"),
        "ENFORCEMENT should say omitting the <thinking> block is a violation");
    assert!(text.contains("SELF-AUDIT HOOK"),
        "PRE-FLIGHT CHECK should have SELF-AUDIT HOOK for recovery after a misstep");
}

#[test]
fn test_instructions_preflight_q1_q2_q3_symmetric() {
    // PRE-FLIGHT Q2 was consolidated from three separate READ/SEARCH/EDIT
    // lines into a single line to reclaim token budget. The consolidated
    // line still names all three operation types with their xray mappings.
    let text = render_instructions(&["rs"]);
    let preflight_start = text.find("MANDATORY PRE-FLIGHT CHECK").unwrap();
    let cost_start = text.find("COST REALITY").unwrap();
    let preflight = &text[preflight_start..cost_start];
    assert!(preflight.contains("READ"),
        "PRE-FLIGHT Q2 should still mention READ scope");
    assert!(preflight.contains("SEARCH"),
        "PRE-FLIGHT Q2 should still mention SEARCH scope");
    assert!(preflight.contains("EDIT"),
        "PRE-FLIGHT Q2 should still mention EDIT scope");
    assert!(preflight.contains("xray_definitions"),
        "PRE-FLIGHT Q2 should map READ to xray_definitions");
    assert!(preflight.contains("xray_grep"),
        "PRE-FLIGHT Q2 should map SEARCH to xray_grep");
    assert!(preflight.contains("xray_edit"),
        "PRE-FLIGHT Q2 should map EDIT to xray_edit");
    assert!(preflight.contains("UNJUSTIFIED"),
        "PRE-FLIGHT should call out 'habit/familiarity' as UNJUSTIFIED");
}

#[test]
fn test_instructions_anti_pattern_extension_based_edit() {
    // The "NEVER choose a built-in edit tool based on file extension" bullet
    // was removed from ANTI-PATTERNS as a duplicate — the same guidance is
    // carried more forcefully by FILE EDITING DECISION TRIGGER (NEVER USE
    // your built-in edit tools for EDITING existing text files ... regardless
    // of file extension) and by the MISCONCEPTION ALERT immediately below it.
    let text = render_instructions(&["rs"]);
    assert!(text.contains("regardless of file extension"),
        "FILE EDITING DECISION TRIGGER should say xray_edit applies regardless of extension");
    assert!(text.contains("MISCONCEPTION ALERT"),
        "FILE EDITING DECISION TRIGGER should keep the MISCONCEPTION ALERT callout");
    assert!(text.contains("NO extension filter"),
        "MISCONCEPTION ALERT should state xray_edit has NO extension filter");
}

#[test]
fn test_instructions_cost_reality_has_multiblock_example() {
    // The multi-block patch/diff example ("8 SEARCH/REPLACE blocks", "8x fewer
    // round-trips", "atomic rollback", "xray_edit does NOT care about --ext
    // for editing") was slimmed out of render_instructions to stay under the
    // system-prompt token budget. The equivalent guidance is now delivered via:
    //   - FILE EDITING DECISION TRIGGER — asserts xray_edit works on ANY text file
    //     regardless of indexed extensions ("xray_edit has NO extension filter")
    //   - xray_help — carries the detailed measured example on-demand.
    let text = render_instructions(&["rs"]);
    assert!(text.contains("NO extension filter"),
        "FILE EDITING DECISION TRIGGER should explicitly say xray_edit has no extension filter");
    assert!(text.contains("xray_edit works on ALL text files"),
        "FILE EDITING DECISION TRIGGER should state xray_edit works on ALL text files");
    assert!(text.contains("atomic"),
        "FILE EDITING DECISION TRIGGER should mention atomic semantics");
}

#[test]
fn test_instructions_symmetric_severity_across_operations() {
    // All three operation rules (READ, EDIT, SEARCH) must use the word NEVER at the same severity
    let text = render_instructions(&["rs"]);
    // Read rule (only present with indexed exts)
    assert!(text.contains("NEVER READ"),
        "READ rule should use NEVER (hard prohibition)");
    // Edit rule
    assert!(text.contains("NEVER USE your built-in edit tools"),
        "EDIT rule should use NEVER USE your built-in edit tools");
    // Search rule
    assert!(text.contains("NEVER USE search_files"),
        "SEARCH rule should use NEVER USE search_files");
}

#[test]
fn test_instructions_no_hardcoded_builtin_names_in_edit_rule() {
    // The NEW edit rule (the one starting with 'NEVER USE your built-in edit tools')
    // should NOT rely on specific built-in tool names as the primary identifier.
    // Specific names may appear inside parentheses as examples, but the rule itself
    // must be tool-name-agnostic.
    let text = render_instructions(&["rs"]);
    // The headline MUST not single out one specific tool — it must address the category.
    assert!(
        text.contains("NEVER USE your built-in edit tools for EDITING"),
        "Edit rule headline must address the tool CATEGORY ('your built-in edit tools'), \
         not a specific named tool. This keeps the policy portable across LLM hosts."
    );
}

#[test]
fn test_instructions_edit_rule_works_without_def_extensions() {
    // v3 symmetry: even without any indexed definitions, the EDIT rule must still
    // appear at full strength — xray_edit works on ANY text file regardless of --ext.
    let text = render_instructions(&[]);
    assert!(text.contains("NEVER USE your built-in edit tools"),
        "Edit rule must be present even when def_extensions is empty");
    assert!(text.contains("xray_edit works on ALL text files"),
        "Edit rule must say xray_edit works on ALL text files, independent of def_extensions");
    assert!(text.contains("MISCONCEPTION ALERT"),
        "MISCONCEPTION ALERT must be present even without def_extensions");
}

#[test]
fn test_instructions_terms_block_always_present() {
    for exts in [&[][..], &["rs"], &["cs", "ts"]] {
        let text = render_instructions(exts);
        assert!(text.contains("=== TERMS ==="),
            "TERMS block must always be present (exts: {:?})", exts);
    }
}


/// Regression test for Bug #1 (self-review 2026-04-17):
/// The Invalid-reasons line in PRE-FLIGHT CHECK must NOT hardcode the `.rs`
/// extension (it should be extension-agnostic since the policy applies to ANY
/// configured --ext). Also the "Just habit / familiarity" phrase must appear
/// as a coherent statement, not a malformed suffix to a list.
#[test]
fn test_instructions_preflight_invalid_reasons_is_extension_agnostic() {
    // With a non-.rs extension set, the policy text must still not contain the
    // literal ".rs" substring inside the Invalid-reasons line.
    let text_cs = render_instructions(&["cs"]);
    assert!(
        !text_cs.contains("this is not a .rs file"),
        "Invalid-reasons line should NOT hardcode '.rs' — it must be extension-agnostic. \
         Got text containing 'this is not a .rs file'."
    );
    // The policy must express the misconception in a neutral way, referring to
    // "file extension is not indexed" rather than naming any specific extension.
    assert!(
        text_cs.contains("extension is not indexed") || text_cs.contains("not indexed"),
        "Invalid-reasons line should call out the extension-indexing misconception in neutral wording"
    );
}

/// Regression test for Bug #1 (self-review 2026-04-17):
/// "Just habit / familiarity" must appear as a coherent clause explicitly
/// marking habit as NEVER a valid reason — not as a malformed dangling suffix
/// to a comma-separated list.
#[test]
fn test_instructions_habit_clause_is_coherent() {
    let text = render_instructions(&["rs"]);
    // The fix inserts a clear sentence ending with "NEVER a valid reason" after
    // the list of invalid reasons. The old broken form had the phrase appended
    // to the list with a dangling arrow.
    assert!(
        text.contains("Just habit / familiarity is NEVER a valid reason"),
        "Policy should contain a coherent habit clause ending with 'NEVER a valid reason'"
    );
    // And the old broken fragment should NOT be present anywhere.
    assert!(
        !text.contains("Just habit / familiarity -> UNJUSTIFIED"),
        "Old broken fragment 'Just habit / familiarity -> UNJUSTIFIED' must not appear — it was a dangling list suffix"
    );
}

// ============================================================================
// Part 4 tests (meta-observation from 2026-04-17 session):
// Prompt improvements to prevent habit-driven selection of built-in search tools
// when xray_grep countOnly=true would be the correct choice for validation intents.
// ============================================================================

/// 4.1: New validation-intent entries in INTENT -> TOOL MAPPING.
#[test]
fn test_instructions_has_validation_intent_mappings() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("validate/fact-check whether a term exists"),
        "INTENT -> TOOL MAPPING should have validate/fact-check entry");
    assert!(text.contains("quick yes/no: does X appear"),
        "INTENT -> TOOL MAPPING should have quick yes/no entry");
    assert!(text.contains("confirm absence of pattern before editing"),
        "INTENT -> TOOL MAPPING should have confirm absence entry");
    // All three must route to xray_grep countOnly=true
    assert!(text.contains("countOnly=true"),
        "Validation intents should route to xray_grep countOnly=true");
}

/// 4.2: EXAMPLE VIOLATION block in ANTI-PATTERNS.
#[test]
fn test_instructions_has_example_violation_block() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("EXAMPLE VIOLATION"),
        "ANTI-PATTERNS should have EXAMPLE VIOLATION block");
    assert!(text.contains("ROOT CAUSE"),
        "EXAMPLE VIOLATION should explain ROOT CAUSE");
    assert!(text.contains("PREVENTION"),
        "EXAMPLE VIOLATION should have PREVENTION guidance");
    assert!(text.contains("linguistic coincidence"),
        "EXAMPLE VIOLATION should call out the linguistic coincidence root cause");
}

/// 4.3: PRE-CALL SELF-AUDIT in addition to post-call SELF-AUDIT HOOK.
#[test]
fn test_instructions_has_pre_call_self_audit() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("PRE-CALL SELF-AUDIT"),
        "instructions should have PRE-CALL SELF-AUDIT (not only post-call SELF-AUDIT HOOK)");
    assert!(text.contains("What is my actual intent?"),
        "PRE-CALL SELF-AUDIT should include the 1-word intent question");
    assert!(text.contains("mental shortcuts on seemingly-trivial tasks"),
        "PRE-CALL SELF-AUDIT should name the violation class it prevents");
    // Post-call hook must still be present (both hooks coexist)
    assert!(text.contains("SELF-AUDIT HOOK"),
        "post-call SELF-AUDIT HOOK must remain alongside PRE-CALL SELF-AUDIT");
}

/// 4.4: "Trivial task trap" tip is present in tips().
#[test]
fn test_tips_contains_trivial_task_trap() {
    let all_tips = tips(&[]);
    let has_trivial = all_tips.iter().any(|t|
        t.rule.contains("Trivial task") && t.rule.contains("trivial policy check")
    );
    assert!(has_trivial, "tips() should contain the 'Trivial task != trivial policy check' tip");
}

/// 4.4: Tip also surfaces in JSON output (xray_help).
#[test]
fn test_render_json_contains_trivial_task_trap() {
    let json = render_json(&[]);
    let practices = json["bestPractices"].as_array().unwrap();
    let has_trivial = practices.iter().any(|p| {
        p["rule"].as_str().unwrap_or("").contains("Trivial task")
    });
    assert!(has_trivial, "JSON output should contain the 'Trivial task' tip");
}


// ─── Deleted file support documentation tests (2026-04-17) ──────────
// These tests guard the policy/tips updates for the deleted-file feature
// added in user story todo_approved_2026-04-17_git-deleted-files-support.md.

#[test]
fn test_tips_includes_deleted_files_tip() {
    let tip_rules: Vec<String> = tips(&["rs".to_string()])
        .iter()
        .map(|t| t.rule.to_string())
        .collect();
    assert!(
        tip_rules.iter().any(|r| r.contains("Deleted files: xray_git_*")),
        "tips() must include the Deleted-Files tip after 2026-04-17 update. Got rules: {:?}",
        tip_rules
    );
}

#[test]
fn test_strategies_includes_deleted_file_archaeology() {
    let strategy_names: Vec<&'static str> = strategies().iter().map(|s| s.name).collect();
    assert!(
        strategy_names.contains(&"Deleted File Archaeology"),
        "strategies() must include the 'Deleted File Archaeology' recipe. Got: {:?}",
        strategy_names
    );
}

#[cfg(all(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-sql"))]
#[test]
fn test_render_instructions_mentions_deleted_files_intent() {
    let text = render_instructions(crate::definitions::definition_extensions());
    assert!(
        text.contains("history of a file that was DELETED/removed"),
        "INTENT -> TOOL MAPPING must include the deleted-file intent line"
    );
    assert!(
        text.contains("show activity including deleted files"),
        "INTENT -> TOOL MAPPING must include the activity-with-deletions intent line"
    );
    assert!(
        text.contains("BOTH existing AND deleted files"),
        "the git blame/history/authors INTENT line must clarify deleted-file coverage"
    );
}

#[cfg(all(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-sql"))]
#[test]
fn test_render_instructions_anti_pattern_against_raw_git() {
    let text = render_instructions(crate::definitions::definition_extensions());
    assert!(
        text.contains("--diff-filter=D"),
        "ANTI-PATTERNS must explicitly call out raw `git log --all --diff-filter=D` as a violation"
    );
    assert!(
        text.contains("auto --follow"),
        "INTENT/ERROR-RECOVERY must explain WHY raw git is wrong (auto --follow fallback exists). Slim wording in render_instructions: 'xray_git_* covers deleted files via auto --follow.'"
    );
}

#[cfg(all(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-sql"))]
#[test]
fn test_render_instructions_error_recovery_mentions_git_specific_rule() {
    let text = render_instructions(crate::definitions::definition_extensions());
    assert!(
        text.contains("6. GIT:"),
        "ERROR RECOVERY pt 6 must include the git-specific fallback prohibition (slim wording)"
    );
    assert!(
        text.contains("ARE the answer"),
        "ERROR RECOVERY pt 6 must explain that 'never existed' / 'deleted' annotations ARE the answer"
    );
}

#[test]
fn test_parameter_examples_includes_deleted_file_docs() {
    let examples = parameter_examples(&["rs".to_string()]);
    let history = &examples["xray_git_history"];
    assert!(
        history.get("deletedFiles").is_some(),
        "xray_git_history parameter_examples must include 'deletedFiles' explanation"
    );
    let activity = &examples["xray_git_activity"];
    assert!(
        activity.get("includeDeleted").is_some(),
        "xray_git_activity parameter_examples must include 'includeDeleted' explanation"
    );
    let include_deleted_str = activity["includeDeleted"].as_str().unwrap_or("");
    assert!(
        include_deleted_str.contains("git ls-files"),
        "includeDeleted documentation must mention the single `git ls-files` performance invariant"
    );
}
