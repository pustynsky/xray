use super::*;

#[test]
fn test_tips_not_empty() {
    assert!(!tips().is_empty());
}

#[test]
fn test_performance_tiers_not_empty() {
    assert!(!performance_tiers().is_empty());
}

#[test]
fn test_tool_priority_not_empty() {
    assert!(!tool_priority().is_empty());
}

#[test]
fn test_render_cli_contains_all_tips() {
    let output = render_cli();
    for tip in tips() {
        assert!(output.contains(tip.rule), "CLI output missing tip: {}", tip.rule);
    }
}

#[test]
fn test_strategies_not_empty() {
    assert!(!strategies().is_empty());
}

#[test]
fn test_render_json_has_best_practices() {
    let json = render_json();
    let practices = json["bestPractices"].as_array().unwrap();
    assert_eq!(practices.len(), tips().len());
}

#[test]
fn test_render_json_has_strategy_recipes() {
    let json = render_json();
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
    // Core tools mentioned
    assert!(text.contains("search_fast"), "instructions should mention search_fast");
    assert!(text.contains("search_callers"), "instructions should mention search_callers");
    assert!(text.contains("search_definitions"), "instructions should mention search_definitions");
    assert!(text.contains("search_grep"), "instructions should mention search_grep");
    // Key features mentioned in 5 bullets
    assert!(text.contains("substring"), "instructions should mention substring");
    assert!(text.contains("containsLine"), "instructions should mention containsLine");
    assert!(text.contains("includeBody"), "instructions should mention includeBody");
    assert!(text.contains("countOnly"), "instructions should mention countOnly");
    // search_help reference (soft, not urgent)
    assert!(text.contains("search_help"), "instructions should mention search_help");
    assert!(!text.contains("IMPORTANT: Call search_help first"), "instructions should NOT have urgent search_help prompt");
    // Strategy recipes and query budget
    assert!(text.contains("STRATEGY RECIPES"), "instructions should include strategy recipes");
    assert!(text.contains("Architecture Exploration"), "instructions should include arch exploration recipe");
    assert!(text.contains("<=3 search calls"), "instructions should mention query budget");
    // PREFER block -- client-agnostic, no emoji, CAPS emphasis
    assert!(text.contains("CRITICAL: ALWAYS use search-index tools"), "instructions should have CRITICAL PREFER block");
    assert!(text.contains("DO NOT browse directories"), "instructions should use DO NOT framing");
    // Dynamic extension list: verify all definition extensions appear in the NEVER READ rule
    for ext in crate::definitions::definition_extensions() {
        assert!(text.contains(&format!(".{}", ext)),
            "instructions should mention .{} extension in NEVER READ rule", ext);
    }
    assert!(text.contains("NEVER READ"), "instructions should have absolute prohibition on reading indexed files");
    assert!(text.contains("FILES DIRECTLY"), "instructions should have FILES DIRECTLY in prohibition");
    assert!(text.contains("DECISION TRIGGER"), "instructions should have decision trigger for file extension check");
    assert!(text.contains("BATCH SPLIT"), "instructions should have batch split instruction for mixed .cs + .md reads");
    assert!(text.contains("ONLY exceptions for"), "instructions should list exceptions for indexed file reading");
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
    let output = render_cli();
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
    let json = render_json();
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
    let tools = tool_definitions();
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
#[test]
fn test_render_instructions_empty_extensions() {
    let text = render_instructions(&[]);
    // Should NOT contain NEVER READ (no definition-supported extensions)
    assert!(!text.contains("NEVER READ"),
        "Empty def_extensions should not produce NEVER READ block");
    assert!(!text.contains("DECISION TRIGGER"),
        "Empty def_extensions should not produce DECISION TRIGGER");
    assert!(!text.contains("BATCH SPLIT"),
        "Empty def_extensions should not produce BATCH SPLIT");
    // Should contain fallback note
    assert!(text.contains("search_definitions is not available"),
        "Empty def_extensions should have fallback note about search_definitions");
    // Core tools should still be mentioned (PREFER block is always present)
    assert!(text.contains("search_grep"), "should still mention search_grep");
    assert!(text.contains("search_fast"), "should still mention search_fast");
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
fn test_all_renderers_consistent_tip_count() {
    let tip_count = tips().len();
    let json = render_json();
    let practices = json["bestPractices"].as_array().unwrap();
    assert_eq!(practices.len(), tip_count, "JSON and tips() count mismatch");

    // Verify CLI output mentions each tip rule
    let cli = render_cli();
    for tip in tips() {
        assert!(cli.contains(tip.rule), "CLI output missing tip: {}", tip.rule);
    }

    // Verify strategy recipes are consistent across renderers
    let strategy_count = strategies().len();
    let recipes = json["strategyRecipes"].as_array().unwrap();
    assert_eq!(recipes.len(), strategy_count, "JSON and strategies() count mismatch");

    for strat in strategies() {
        assert!(cli.contains(strat.name), "CLI output missing strategy: {}", strat.name);
    }
}
