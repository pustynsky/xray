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
    // Core tools mentioned (via TASK ROUTING table)
    assert!(text.contains("search_fast"), "instructions should mention search_fast");
    assert!(text.contains("search_callers"), "instructions should mention search_callers");
    assert!(text.contains("search_definitions"), "instructions should mention search_definitions");
    assert!(text.contains("search_grep"), "instructions should mention search_grep");
    assert!(text.contains("search_edit"), "instructions should mention search_edit");
    // TASK ROUTING table present
    assert!(text.contains("TASK ROUTING"), "instructions should have TASK ROUTING table");
    assert!(text.contains("check BEFORE using any built-in tool"), "TASK ROUTING should have routing directive");
    // Key features mentioned in strategy recipes / DECISION TRIGGERs
    assert!(text.contains("includeBody"), "instructions should mention includeBody");
    // search_help reference (soft, not urgent)
    assert!(text.contains("search_help"), "instructions should mention search_help");
    assert!(!text.contains("IMPORTANT: Call search_help first"), "instructions should NOT have urgent search_help prompt");
    // Strategy recipes and query budget
    assert!(text.contains("STRATEGY RECIPES"), "instructions should include strategy recipes");
    assert!(text.contains("Architecture Exploration"), "instructions should include arch exploration recipe");
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
    // Zero-result hints auto-follow rule
    assert!(text.contains("ZERO-RESULT HINTS"), "instructions should have ZERO-RESULT HINTS auto-follow rule");
    assert!(text.contains("AUTOMATICALLY follow the hint"), "instructions should tell LLM to auto-follow hints");
    assert!(text.contains("NEAREST MATCH"), "instructions should have NEAREST MATCH auto-follow rule");
    assert!(text.contains("KIND MISMATCH"), "instructions should have KIND MISMATCH auto-follow rule");
    assert!(text.contains("NEVER ask the user whether to follow a hint"), "instructions should prohibit asking user about hints");
    // Removed sections should NOT be present
    assert!(!text.contains("Quick Reference"), "instructions should NOT have Quick Reference (replaced by TASK ROUTING)");
    assert!(!text.contains("TOOL PRIORITY"), "instructions should NOT have TOOL PRIORITY (replaced by TASK ROUTING)");
    assert!(!text.contains("CRITICAL: ALWAYS use search-index tools"), "instructions should NOT have old CRITICAL block");
    assert!(!text.contains("BATCH SPLIT"), "instructions should NOT have BATCH SPLIT (removed)");
    assert!(!text.contains("TRAP"), "instructions should NOT have TRAP (removed)");
    // Fallback rule
    assert!(text.contains("uncertain"), "instructions should have fallback rule for uncertainty");
    // No emoji in machine-targeted text
    assert!(!text.contains('⚠'), "instructions should not contain emoji (machine-targeted text)");
    assert!(!text.contains('⚡'), "instructions should not contain emoji (machine-targeted text)");
    // No Roo-specific tool names
    assert!(!text.contains("read_file"), "instructions should not reference Roo-specific read_file");
    assert!(!text.contains("list_files"), "instructions should not reference Roo-specific list_files");
    assert!(!text.contains("list_code_definition_names"), "instructions should not reference Roo-specific list_code_definition_names");
}

/// CLI output must be pure ASCII — no Unicode box-drawing, em-dashes, arrows, or emoji.
/// Windows cmd.exe (CP437/CP866) cannot display these characters correctly.
#[test]
fn test_render_cli_is_ascii_safe() {
    let output = render_cli(&vec!["rs".to_string()]);
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
    let json = render_json(&vec!["rs".to_string()]);
    let examples = &json["parameterExamples"];
    assert!(examples.is_object(), "parameterExamples should be an object");
    // Key tools should have examples
    assert!(examples["search_definitions"].is_object(), "search_definitions should have examples");
    assert!(examples["search_grep"].is_object(), "search_grep should have examples");
    assert!(examples["search_callers"].is_object(), "search_callers should have examples");
    assert!(examples["search_fast"].is_object(), "search_fast should have examples");
    // Spot-check a few specific examples
    assert!(examples["search_definitions"]["name"].is_string(), "name should have example");
    assert!(examples["search_definitions"]["containsLine"].is_string(), "containsLine should have example");
    assert!(examples["search_grep"]["terms"].is_string(), "terms should have example");
    assert!(examples["search_callers"]["class"].is_string(), "class should have example");
}

/// Verify tool definitions stay within a reasonable token budget.
/// This test prevents description bloat from re-accumulating over time.
/// Target: <5000 approx tokens (word_count / 0.75).
#[test]
fn test_tool_definitions_token_budget() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec!["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let json = serde_json::to_string(&tools).unwrap();
    let word_count = json.split_whitespace().count();
    let approx_tokens = (word_count as f64 / 0.75) as usize;

    // Budget: ~5000 tokens (down from ~6500 before optimization)
    assert!(
        approx_tokens < 5500,
        "Tool definitions exceed token budget: ~{} tokens ({} words). \
         Target: <5500. Shorten parameter descriptions or move examples to search_help.",
        approx_tokens, word_count
    );
}

/// Empty def_extensions: NEVER READ block should be skipped,
/// fallback note about search_definitions unavailability should appear.
/// TASK ROUTING should contain only universal tools.
#[test]
fn test_render_instructions_empty_extensions() {
    let text = render_instructions(&[]);
    // Should NOT contain NEVER READ (no definition-supported extensions)
    assert!(!text.contains("NEVER READ"),
        "Empty def_extensions should not produce NEVER READ block");
    // The file-reading DECISION TRIGGER must NOT appear (no indexed extensions).
    // But the file-editing DECISION TRIGGER and zero-result hints DECISION TRIGGER SHOULD appear (always present).
    // Count occurrences: should be exactly 2 (editing + zero-result hints, not reading).
    let dt_count = text.matches("DECISION TRIGGER").count();
    assert_eq!(dt_count, 2,
        "Empty def_extensions should have 2 DECISION TRIGGERs (editing + zero-result hints), not {} (reading trigger should be absent)", dt_count);
    // Should contain fallback note
    assert!(text.contains("search_definitions is not available"),
        "Empty def_extensions should have fallback note about search_definitions");
    // TASK ROUTING should be present but WITHOUT definition-dependent tools
    assert!(text.contains("TASK ROUTING"), "should have TASK ROUTING even with empty extensions");
    assert!(text.contains("search_grep"), "should still mention search_grep");
    assert!(text.contains("search_fast"), "should still mention search_fast");
    assert!(text.contains("search_edit"), "should still mention search_edit");
    assert!(!text.contains("Read/explore source code"),
        "should NOT have definition-dependent task routing when def_extensions is empty");
    assert!(!text.contains("Find callers"),
        "should NOT have callers routing when def_extensions is empty");
    assert!(text.contains("STRATEGY RECIPES"), "should still include strategy recipes");
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

// ─── New tests for Task Routing ─────────────────────────────────────

#[test]
fn test_task_routings_not_empty() {
    assert!(!task_routings().is_empty(), "task_routings() should not be empty");
}

#[test]
fn test_task_routing_with_definitions() {
    let text = render_instructions(&["cs", "ts"]);
    assert!(text.contains("TASK ROUTING"), "should have TASK ROUTING");
    // Definition-dependent routes should be present
    assert!(text.contains("Read/explore source code"), "should have source code exploration route");
    assert!(text.contains("Find callers or callees"), "should have callers route");
    assert!(text.contains("Code complexity"), "should have code health route");
    // Universal routes always present
    assert!(text.contains("Search file contents"), "should have grep route");
    assert!(text.contains("Find a file by name"), "should have fast route");
    assert!(text.contains("Edit a file"), "should have edit route");
    assert!(text.contains("Git blame"), "should have git route");
}

#[test]
fn test_task_routing_without_definitions() {
    let text = render_instructions(&[]);
    assert!(text.contains("TASK ROUTING"), "should have TASK ROUTING even without defs");
    // Definition-dependent task descriptions should NOT be in routing table
    assert!(!text.contains("Read/explore source code"), "should NOT have definition-dependent routing");
    assert!(!text.contains("Find callers or callees"), "should NOT have callers routing");
    assert!(!text.contains("Code complexity"), "should NOT have code health routing");
    // Universal routes always present
    assert!(text.contains("search_grep"), "should mention search_grep");
    assert!(text.contains("search_fast"), "should mention search_fast");
    assert!(text.contains("search_edit"), "should mention search_edit");
    assert!(text.contains("search_git_blame"), "should mention git tools");
}

#[test]
fn test_task_routing_always_has_universal_tools() {
    for exts in [&[][..], &["cs"], &["cs", "ts", "rs"]] {
        let text = render_instructions(exts);
        assert!(text.contains("search_grep"), "search_grep should always be in routing (exts: {:?})", exts);
        assert!(text.contains("search_fast"), "search_fast should always be in routing (exts: {:?})", exts);
        assert!(text.contains("search_edit"), "search_edit should always be in routing (exts: {:?})", exts);
    }
}

/// Validate that all tool names referenced in task_routings() actually exist
/// in tool_definitions(). Prevents routing drift when tools are renamed.
#[test]
fn test_routing_tool_names_exist_in_definitions() {
    use crate::mcp::handlers::tool_definitions;
    let tool_names: Vec<String> = tool_definitions(&vec!["cs".to_string()]).iter().map(|t| t.name.clone()).collect();
    for tr in task_routings() {
        // Tool field may contain multiple tools separated by " / "
        for tool_name in tr.tool.split(" / ") {
            let tool_name = tool_name.trim();
            assert!(
                tool_names.contains(&tool_name.to_string()),
                "TaskRouting references tool '{}' (task: '{}') which does not exist in tool_definitions(). \
                 Available tools: {:?}",
                tool_name, tr.task, tool_names
            );
        }
    }
}

/// Routing-critical tools must contain a routing hint in their description.
#[test]
fn test_routing_critical_tools_have_hints() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec!["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let routing_critical = ["search_definitions", "search_grep", "search_fast", "search_edit"];
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
    assert!(text.contains("search_info"), "fallback should mention search_info");
    assert!(text.contains("Do not default to raw file reading"), "fallback should discourage raw reads");
}

/// Instructions token budget: should be shorter after consolidation.
/// Measured with word_count / 0.75 approximation (same as tool_definitions test).
#[test]
fn test_instructions_token_budget() {
    let text = render_instructions(&["cs", "ts", "tsx", "sql", "rs"]);
    let word_count = text.split_whitespace().count();
    let approx_tokens = (word_count as f64 / 0.75) as usize;
    // After consolidation: target <=1500 tokens (was ~1800 before)
    assert!(
        approx_tokens < 1500,
        "Instructions exceed token budget: ~{} tokens ({} words). \
         Target: <1500 after consolidation. Remove redundant sections.",
        approx_tokens, word_count
    );
}

/// Instructions should NOT contain removed sections.
#[test]
fn test_instructions_no_redundant_sections() {
    let text = render_instructions(&["cs", "ts"]);
    assert!(!text.contains("Quick Reference"), "Quick Reference should be removed (replaced by TASK ROUTING)");
    assert!(!text.contains("TOOL PRIORITY"), "TOOL PRIORITY should be removed (replaced by TASK ROUTING)");
    assert!(!text.contains("CRITICAL: ALWAYS use search-index tools"), "Old CRITICAL block should be removed");
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
    let tools = tool_definitions(&vec!["rs".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "search_definitions").unwrap();
    assert!(def_tool.description.contains("Rust"),
        "search_definitions description should contain 'Rust' when ext=rs. Got: {}", def_tool.description);
    assert!(!def_tool.description.contains("C#"),
        "search_definitions should NOT contain 'C#' when only rs is configured");

    let callers_tool = tools.iter().find(|t| t.name == "search_callers").unwrap();
    assert!(callers_tool.description.contains("Rust"),
        "search_callers description should contain 'Rust'");
}

#[test]
fn test_tool_definitions_empty_extensions() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec![]);
    let def_tool = tools.iter().find(|t| t.name == "search_definitions").unwrap();
    assert!(def_tool.description.contains("not available"),
        "search_definitions should say 'not available' when no extensions");

    let callers_tool = tools.iter().find(|t| t.name == "search_callers").unwrap();
    assert!(callers_tool.description.contains("not available"),
        "search_callers should say 'not available' when no extensions");
}

#[test]
fn test_tool_definitions_cs_ts_tsx() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec!["cs".to_string(), "ts".to_string(), "tsx".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "search_definitions").unwrap();
    assert!(def_tool.description.contains("C# and TypeScript/TSX"),
        "search_definitions should contain 'C# and TypeScript/TSX'. Got: {}", def_tool.description);
}

#[test]
fn test_render_instructions_example_line() {
    let text = render_instructions(&["rs"]);
    assert!(text.contains("EXAMPLE: instead of reading handler.rs directly"),
        "Instructions should contain EXAMPLE line for the configured extension");
    assert!(text.contains("search_definitions file='handler.rs'"),
        "EXAMPLE should show search_definitions with the correct extension");
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
    let tools = tool_definitions(&vec!["sql".to_string()]);
    let def_tool = tools.iter().find(|t| t.name == "search_definitions").unwrap();
    assert!(def_tool.description.contains("SQL (regex-based parser)"),
        "search_definitions should contain 'SQL (regex-based parser)' for sql-only. Got: {}", def_tool.description);
    // Should NOT mention tree-sitter for SQL-only
    assert!(!def_tool.description.contains("C#"),
        "Should NOT mention C# for sql-only");
}

/// Verify that tool_definitions with mixed tree-sitter + regex includes both.
/// Also verify ALL three definition-dependent tools are consistent.
#[test]
fn test_tool_definitions_cs_rs_sql() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec!["cs".to_string(), "rs".to_string(), "sql".to_string()]);

    // search_definitions
    let def_tool = tools.iter().find(|t| t.name == "search_definitions").unwrap();
    assert!(def_tool.description.contains("C# and Rust"),
        "search_definitions should contain both tree-sitter languages");
    assert!(def_tool.description.contains("SQL supported via regex parser"),
        "search_definitions should mention SQL via regex parser");

    // search_callers must mention the same languages
    let callers_tool = tools.iter().find(|t| t.name == "search_callers").unwrap();
    assert!(callers_tool.description.contains("C# and Rust"),
        "search_callers should contain same tree-sitter languages as search_definitions");
    assert!(callers_tool.description.contains("SQL supported via regex parser"),
        "search_callers should mention SQL via regex parser");

    // search_reindex_definitions must also mention languages
    let reindex_tool = tools.iter().find(|t| t.name == "search_reindex_definitions").unwrap();
    assert!(reindex_tool.description.contains("C# and Rust"),
        "search_reindex_definitions should contain tree-sitter languages");
    assert!(reindex_tool.description.contains("SQL supported via regex parser"),
        "search_reindex_definitions should mention SQL via regex parser");
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

/// Verify all 3 definition-dependent tools say "not available" consistently when empty.
#[test]
fn test_tool_definitions_all_three_say_not_available_when_empty() {
    use crate::mcp::handlers::tool_definitions;
    let tools = tool_definitions(&vec![]);
    for tool_name in ["search_definitions", "search_callers", "search_reindex_definitions"] {
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
}

#[test]
fn test_tool_definitions_reindex_defs_dynamic() {
    use crate::mcp::handlers::tool_definitions;
    // С расширениями — описание содержит языки
    let tools = tool_definitions(&vec!["rs".to_string()]);
    let reindex = tools.iter().find(|t| t.name == "search_reindex_definitions").unwrap();
    assert!(reindex.description.contains("Rust"),
        "search_reindex_definitions should mention 'Rust'. Got: {}", reindex.description);
    assert!(!reindex.description.contains("tree-sitter"),
        "search_reindex_definitions should NOT hardcode 'tree-sitter'");

    // Без расширений — "not available"
    let tools_empty = tool_definitions(&vec![]);
    let reindex_empty = tools_empty.iter().find(|t| t.name == "search_reindex_definitions").unwrap();
    assert!(reindex_empty.description.contains("not available"),
        "search_reindex_definitions should say 'not available' when empty");
}
