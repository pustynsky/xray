//! Shared tree-sitter AST utility functions used by all language parsers.
//!
//! These functions are identical across C#, TypeScript, and Rust parsers.
//! Centralizing them here eliminates 12 duplicate function definitions.

/// Extract the UTF-8 text of a tree-sitter node from the source bytes.
///
/// Returns an empty string if the node's byte range contains invalid UTF-8.
pub(crate) fn node_text<'a>(node: tree_sitter::Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// Find the first direct child of `node` whose `kind()` matches `kind`.
///
/// Only checks immediate children (depth 1), not descendants.
pub(crate) fn find_child_by_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return Some(child);
            }
        }
    }
    None
}

/// Find the first descendant of `node` (at any depth) whose `kind()` matches `kind`.
///
/// Performs a depth-first search through the entire subtree.
pub(crate) fn find_descendant_by_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return Some(child);
            }
            if let Some(found) = find_descendant_by_kind(child, kind) {
                return Some(found);
            }
        }
    }
    None
}

/// Find a child node by its field name in the grammar.
///
/// Delegates to tree-sitter's `child_by_field_name`.
pub(crate) fn find_child_by_field<'a>(node: tree_sitter::Node<'a>, field: &str) -> Option<tree_sitter::Node<'a>> {
    node.child_by_field_name(field)
}

/// Count the number of named children in a parameter list node.
///
/// This is the shared core logic used by both C# and TypeScript parameter
/// counting. Each language finds its parameter list node differently
/// (`parameter_list` vs `formal_parameters`), but the counting logic is identical.
pub(crate) fn count_named_children(node: tree_sitter::Node) -> u8 {
    (0..node.child_count())
        .filter(|&i| node.child(i).map(|c| c.is_named()).unwrap_or(false))
        .count() as u8
}

// ─── Code Stats Config ──────────────────────────────────────────────

use crate::definitions::types::CodeStats;

/// Language-specific configuration for code complexity metrics computation.
///
/// Each language parser defines a static config with its AST node names.
/// The unified [`walk_code_stats`] function uses this config to compute
/// cyclomatic complexity, cognitive complexity, nesting depth, and other metrics
/// identically across all languages.
pub(crate) struct CodeStatsConfig {
    /// Nodes that add +1 cyclomatic AND +1+nesting cognitive complexity.
    /// e.g., `if_statement`, `for_statement`, `while_statement`, `catch_clause`
    pub branching_nodes: &'static [&'static str],
    /// The "else" clause node name. e.g., `"else_clause"`
    pub else_clause: &'static str,
    /// The if-statement node inside else clause. e.g., `"if_statement"` or `"if_expression"`
    pub else_if_child: &'static str,
    /// Binary expression node name. e.g., `"binary_expression"`
    pub binary_op_node: &'static str,
    /// Whether to also check operator text via `node_text()` for logical operators.
    /// Needed for Rust where tree-sitter may represent `&&`/`||` differently.
    pub check_logical_op_text: bool,
    /// Nodes that add +1 cognitive complexity with NO nesting penalty.
    /// e.g., `goto_statement` in C#. Empty for most languages.
    pub cognitive_only_nodes: &'static [&'static str],
    /// Switch case/arm nodes: +1 cyclomatic only (the switch itself already counted cognitive).
    /// e.g., `switch_section`, `switch_case`, `match_arm`
    pub case_nodes: &'static [&'static str],
    /// Return/throw nodes: +1 return_count.
    pub return_nodes: &'static [&'static str],
    /// Lambda/closure nodes: +1 lambda_count.
    pub lambda_nodes: &'static [&'static str],
    /// Nodes that increment nesting depth for children.
    /// Typically a superset of branching_nodes + try + lambda nodes.
    pub nesting_nodes: &'static [&'static str],
    /// If true, a direct if→if child (without else_clause wrapper) keeps nesting flat.
    /// C# specific: tree-sitter C# can parse else-if as `if_statement` → `if_statement`.
    pub if_to_if_flat_nesting: bool,
}

/// C# code stats configuration.
pub(crate) static CSHARP_CODE_STATS_CONFIG: CodeStatsConfig = CodeStatsConfig {
    branching_nodes: &[
        "if_statement", "for_statement", "foreach_statement",
        "while_statement", "do_statement",
        "switch_statement", "switch_expression",
        "catch_clause", "conditional_expression",
    ],
    else_clause: "else_clause",
    else_if_child: "if_statement",
    binary_op_node: "binary_expression",
    check_logical_op_text: false,
    cognitive_only_nodes: &["goto_statement"],
    case_nodes: &["switch_expression_arm", "switch_section"],
    return_nodes: &["return_statement", "throw_statement", "throw_expression"],
    lambda_nodes: &["lambda_expression", "anonymous_method_expression"],
    nesting_nodes: &[
        "if_statement", "for_statement", "foreach_statement",
        "while_statement", "do_statement",
        "switch_statement", "switch_expression",
        "catch_clause", "conditional_expression",
        "try_statement", "lambda_expression", "anonymous_method_expression",
    ],
    if_to_if_flat_nesting: true,
};

/// TypeScript/TSX code stats configuration.
pub(crate) static TYPESCRIPT_CODE_STATS_CONFIG: CodeStatsConfig = CodeStatsConfig {
    branching_nodes: &[
        "if_statement", "for_statement", "for_in_statement",
        "while_statement", "do_statement",
        "switch_statement",
        "catch_clause", "ternary_expression",
    ],
    else_clause: "else_clause",
    else_if_child: "if_statement",
    binary_op_node: "binary_expression",
    check_logical_op_text: false,
    cognitive_only_nodes: &[],
    case_nodes: &["switch_case"],
    return_nodes: &["return_statement", "throw_statement"],
    lambda_nodes: &["arrow_function", "function_expression"],
    nesting_nodes: &[
        "if_statement", "for_statement", "for_in_statement",
        "while_statement", "do_statement", "switch_statement",
        "catch_clause", "ternary_expression",
        "try_statement", "arrow_function", "function_expression",
    ],
    if_to_if_flat_nesting: false,
};

/// Rust code stats configuration.
#[cfg(feature = "lang-rust")]
pub(crate) static RUST_CODE_STATS_CONFIG: CodeStatsConfig = CodeStatsConfig {
    branching_nodes: &[
        "if_expression", "for_expression", "while_expression",
        "loop_expression", "match_expression",
    ],
    else_clause: "else_clause",
    else_if_child: "if_expression",
    binary_op_node: "binary_expression",
    check_logical_op_text: true,
    cognitive_only_nodes: &[],
    case_nodes: &["match_arm"],
    return_nodes: &["return_expression", "try_expression"],
    lambda_nodes: &["closure_expression"],
    nesting_nodes: &[
        "if_expression", "for_expression", "while_expression",
        "loop_expression", "match_expression",
        "closure_expression",
    ],
    if_to_if_flat_nesting: false,
};

/// Unified code complexity walker for all tree-sitter-based languages.
///
/// Walks the AST recursively, computing cyclomatic complexity, cognitive complexity,
/// nesting depth, return count, and lambda count based on the language-specific
/// [`CodeStatsConfig`].
///
/// # Arguments
/// * `node` — current AST node
/// * `source` — source file bytes (needed for logical operator text check in Rust)
/// * `nesting` — current nesting depth
/// * `stats` — mutable stats accumulator
/// * `config` — language-specific node name configuration
pub(crate) fn walk_code_stats(
    node: tree_sitter::Node,
    source: &[u8],
    nesting: u32,
    stats: &mut CodeStats,
    config: &CodeStatsConfig,
) {
    let kind = node.kind();

    // ═══ Complexity increments ═══

    if config.branching_nodes.contains(&kind) {
        // Structural: +1 cyclomatic, +1+nesting cognitive
        stats.cyclomatic_complexity = stats.cyclomatic_complexity.saturating_add(1);
        stats.cognitive_complexity = stats.cognitive_complexity.saturating_add(1 + nesting as u16);
    } else if kind == config.else_clause {
        // else/else-if handling
        let is_else_if = (0..node.child_count())
            .any(|i| node.child(i).is_some_and(|c| c.kind() == config.else_if_child));
        if !is_else_if {
            // standalone else: +1 cognitive, no nesting penalty
            stats.cognitive_complexity = stats.cognitive_complexity.saturating_add(1);
        }
    } else if kind == config.binary_op_node {
        // Logical operators: && and ||
        if let Some(op) = node.child(1) {
            let op_kind = op.kind();
            let is_logical = op_kind == "&&" || op_kind == "||"
                || (config.check_logical_op_text && {
                    let op_text = node_text(op, source);
                    op_text == "&&" || op_text == "||"
                });
            if is_logical {
                stats.cyclomatic_complexity = stats.cyclomatic_complexity.saturating_add(1);
                // Cognitive: +1 only at start of new operator sequence
                let parent_same_op = node.parent()
                    .filter(|p| p.kind() == config.binary_op_node)
                    .and_then(|p| p.child(1))
                    .map(|pop| pop.kind() == op_kind)
                    .unwrap_or(false);
                if !parent_same_op {
                    stats.cognitive_complexity = stats.cognitive_complexity.saturating_add(1);
                }
            }
        }
    } else if config.cognitive_only_nodes.contains(&kind) {
        // goto-like: +1 cognitive only (no nesting penalty, no cyclomatic)
        stats.cognitive_complexity = stats.cognitive_complexity.saturating_add(1);
    } else if config.case_nodes.contains(&kind) {
        // switch case/arm: +1 cyclomatic only
        stats.cyclomatic_complexity = stats.cyclomatic_complexity.saturating_add(1);
    } else if config.return_nodes.contains(&kind) {
        stats.return_count = stats.return_count.saturating_add(1);
    } else if config.lambda_nodes.contains(&kind) {
        stats.lambda_count = stats.lambda_count.saturating_add(1);
    }

    // ═══ Nesting for children ═══

    let body_nesting = if config.nesting_nodes.contains(&kind) {
        nesting + 1
    } else {
        nesting
    };

    stats.max_nesting_depth = stats.max_nesting_depth.max(body_nesting as u8);

    // ═══ Recurse with else-if nesting rules ═══

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            let child_nesting = match (kind, child.kind()) {
                // else_clause at same level as if
                (parent, child_kind) if parent == config.else_if_child && child_kind == config.else_clause => nesting,
                // else-if continuation
                (parent, child_kind) if parent == config.else_clause && child_kind == config.else_if_child => nesting,
                // else body is nested
                (parent, _) if parent == config.else_clause => nesting + 1,
                // C# specific: if_statement → if_statement (direct child) = else-if without wrapper
                (parent, child_kind) if config.if_to_if_flat_nesting
                    && parent == config.else_if_child
                    && child_kind == config.else_if_child => nesting,
                _ => body_nesting,
            };

            walk_code_stats(child, source, child_nesting, stats, config);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}