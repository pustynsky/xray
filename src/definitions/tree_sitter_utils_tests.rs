#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::{node_text, find_child_by_kind, find_descendant_by_kind, find_child_by_field};

/// Helper: parse a C# snippet and return the root node + source bytes.
/// Uses the C# grammar since it's a default feature.
fn parse_csharp_snippet(source: &str) -> (tree_sitter::Tree, Vec<u8>) {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .expect("Error loading C# grammar");
    let tree = parser.parse(source, None).expect("parse failed");
    (tree, source.as_bytes().to_vec())
}

#[test]
fn test_node_text_basic() {
    let source = "class UserService {}";
    let (tree, bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    assert_eq!(node_text(root, &bytes), source);
}

#[test]
fn test_find_child_by_kind_found() {
    let source = "class UserService {}";
    let (tree, _bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let cls = find_child_by_kind(root, "class_declaration");
    assert!(cls.is_some(), "should find class_declaration");
}

#[test]
fn test_find_child_by_kind_not_found() {
    let source = "class UserService {}";
    let (tree, _bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let result = find_child_by_kind(root, "function_item");
    assert!(result.is_none(), "should not find function_item in C#");
}

#[test]
fn test_find_descendant_by_kind_found() {
    let source = "class UserService { void Process() { int x = 1; } }";
    let (tree, _bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let method = find_descendant_by_kind(root, "method_declaration");
    assert!(method.is_some(), "should find method_declaration as descendant");
}

#[test]
fn test_find_descendant_by_kind_not_found() {
    let source = "class UserService {}";
    let (tree, _bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let result = find_descendant_by_kind(root, "while_statement");
    assert!(result.is_none(), "should not find while_statement");
}

#[test]
fn test_find_child_by_field_found() {
    let source = "class UserService {}";
    let (tree, bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let cls = find_child_by_kind(root, "class_declaration").unwrap();
    let name = find_child_by_field(cls, "name");
    assert!(name.is_some(), "class_declaration should have 'name' field");
    assert_eq!(node_text(name.unwrap(), &bytes), "UserService");
}

#[test]
fn test_find_child_by_field_not_found() {
    let source = "class UserService {}";
    let (tree, _bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let cls = find_child_by_kind(root, "class_declaration").unwrap();
    let result = find_child_by_field(cls, "nonexistent_field");
    assert!(result.is_none());
}

// ─── walk_code_stats config-driven tests ────────────────────────

#[test]
fn test_walk_code_stats_csharp_config_if_else() {
    // Verify the unified walk_code_stats produces correct metrics for C#
    // using CSHARP_CODE_STATS_CONFIG (regression test for the data-driven refactoring).
    let source = r#"
class UserService {
void Process(int x) {
    if (x > 0) {
        Console.WriteLine("positive");
    } else if (x < 0) {
        Console.WriteLine("negative");
    } else {
        Console.WriteLine("zero");
    }
}
}
"#;
    let (tree, bytes) = parse_csharp_snippet(source);
    let root = tree.root_node();
    let method = find_descendant_by_kind(root, "block").unwrap();

    let mut stats = super::CodeStats::default();
    stats.cyclomatic_complexity = 1; // base
    super::walk_code_stats(method, &bytes, 0, &mut stats, &super::CSHARP_CODE_STATS_CONFIG);

    // if (+1), else-if's inner if (+1) = cyclomatic 3
    assert_eq!(stats.cyclomatic_complexity, 3,
        "C# if/else-if should add +2 cyclomatic (base=1 → total=3)");
    // cognitive: if (+1+0=1), else-if's if (+1+0=1) = 2
    // Note: tree-sitter C# (0.23) does NOT produce else_clause nodes for standalone else,
    // so the else block does NOT add +1 cognitive. This is a known grammar difference.
    assert_eq!(stats.cognitive_complexity, 2,
        "C# if/else-if should have cognitive=2 (tree-sitter C# has no else_clause nodes)");
}

#[test]
fn test_walk_code_stats_typescript_config_arrow_function() {
    // Verify that TypeScript lambda_nodes (arrow_function) are counted correctly
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .expect("Error loading TypeScript grammar");
    let source = "const handler = () => { if (true) { return 1; } }";
    let tree = parser.parse(source, None).expect("parse failed");
    let bytes = source.as_bytes();
    let root = tree.root_node();
    // Find the arrow_function body (statement_block)
    let arrow = find_descendant_by_kind(root, "arrow_function").unwrap();
    let body = find_child_by_kind(arrow, "statement_block").unwrap();

    let mut stats = super::CodeStats::default();
    stats.cyclomatic_complexity = 1;
    super::walk_code_stats(body, bytes, 0, &mut stats, &super::TYPESCRIPT_CODE_STATS_CONFIG);

    assert_eq!(stats.cyclomatic_complexity, 2, "if inside arrow adds +1 cyclomatic");
    assert_eq!(stats.return_count, 1, "return statement counted");
}
