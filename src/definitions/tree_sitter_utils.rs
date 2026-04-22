#![cfg(any(feature = "lang-csharp", feature = "lang-typescript", feature = "lang-rust"))]
// Individual helpers and static configs are used only by the parser matching their
// language. When a single-language feature set is active (e.g. only `lang-rust`),
// the helpers/configs for the other languages become dead code. Allowing it here
// keeps per-feature builds clean without scattering `#[cfg]` noise across every
// helper.
#![allow(dead_code)]
//! Shared tree-sitter AST utility functions used by all language parsers.
//!
//! These functions are identical across C#, TypeScript, and Rust parsers.
//! Centralizing them here eliminates 12 duplicate function definitions.
//!
//! Gated behind the tree-sitter-backed language features (csharp / typescript / rust):
//! the entire module operates on `tree_sitter::Node`, so without any of these features
//! the `tree-sitter` crate is not in the dependency graph and this module must not
//! be compiled.
/// Extract the UTF-8 text of a tree-sitter node from the source bytes.
///
/// Returns an empty string if the node's byte range contains invalid UTF-8,
/// and emits a `tracing::warn!` once per process (MINOR-13). The warning
/// is coalesced with an atomic flag so a file with many non-UTF-8 nodes
/// cannot flood the log — the first occurrence names the node kind and
/// line for triage, subsequent ones are silent.
pub(crate) fn node_text<'a>(node: tree_sitter::Node, source: &'a [u8]) -> &'a str {
    match node.utf8_text(source) {
        Ok(s) => s,
        Err(_) => {
            use std::sync::atomic::{AtomicBool, Ordering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    node_kind = node.kind(),
                    line = node.start_position().row + 1,
                    "node_text: byte range is not valid UTF-8; returning empty string \
                     (further occurrences suppressed). Non-UTF-8 source files \
                     (Windows-1251, SHIFT-JIS) are indexed lossily."
                );
            }
            ""
        }
    }
}

/// Find the first direct child of `node` whose `kind()` matches `kind`.
///
/// Only checks immediate children (depth 1), not descendants.
pub(crate) fn find_child_by_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == kind {
                return Some(child);
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
///
/// Returns `u32` so that pathological auto-generated code with very large
/// parameter lists is observable at call sites (MINOR-12). Callers saturate
/// this to the narrower storage type (`u8` in [`CodeStats`]) and emit a
/// `tracing::warn!` when truncation actually happens, so saturation is no
/// longer silent.
pub(crate) fn count_named_children(node: tree_sitter::Node) -> u32 {
    (0..node.child_count())
        .filter(|&i| node.child(i).map(|c| c.is_named()).unwrap_or(false))
        .count() as u32
}

/// Saturate a `u32` count into a `u8` for on-disk [`CodeStats`] storage and
/// emit a `tracing::warn!` the first time truncation happens per call site.
/// Use at every site that assigns `count_named_children` (or similar wide
/// counts) into a `u8` field. Keeps the storage format unchanged while
/// making lossy saturation observable in logs (MINOR-12).
pub(crate) fn saturate_count_to_u8(value: u32, context: &str) -> u8 {
    if value > u8::MAX as u32 {
        tracing::warn!(
            value,
            context,
            "count exceeds u8::MAX; saturating at 255 — complexity metric will be lossy"
        );
        u8::MAX
    } else {
        value as u8
    }
}

/// Hard cap on recursive AST descent for tree-sitter walkers (MINOR-27).
/// Matches the value used in [`parser_xml`] for consistency. In normal
/// Rust/TypeScript code the AST depth is well under 50; the cap protects
/// against pathological auto-generated sources and tree-sitter grammar
/// regressions which could otherwise cause a `SIGABRT` stack overflow
/// on an MCP stdio server.
pub(crate) const MAX_AST_RECURSION_DEPTH: usize = 1024;

/// PARSE-002: hard cap on input size before invoking tree-sitter. Tree-sitter
/// allocates ~5–10× source size for the parse tree and token table; a 100 MB
/// generated/vendored file (or a checked-in bundle) would transiently consume
/// ~1 GB per worker. Matches `ripgrep`'s default `--max-filesize` of 4 MB.
pub(crate) const MAX_PARSE_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// PARSE-001: per-parse wall-clock ceiling for tree-sitter.
///
/// Without this, a hand-crafted source file (deeply nested generics, attribute
/// spam, known-pathological tree-sitter inputs) can pin a parser worker thread
/// indefinitely, freezing the entire incremental indexing pipeline because the
/// worker pool is fixed at `num_cpus()`.
///
/// 2 seconds is generous for legitimate parses (the longest production parse
/// observed in benches is ~150 ms on a 4 MB file) and short enough to recover
/// from a single bad file within the user's patience window.
pub(crate) const PARSE_TIMEOUT_MICROS: u64 = 2_000_000;


/// Emit a one-shot `tracing::warn!` when a walker hits
/// [`MAX_AST_RECURSION_DEPTH`]. Coalesced with a process-global atomic so
/// a single pathological file cannot flood the log. The first occurrence
/// reports the parser name and node line; subsequent ones are silent.
pub(crate) fn warn_ast_depth_exceeded(parser: &str, node: tree_sitter::Node) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            parser,
            line = node.start_position().row + 1,
            max_depth = MAX_AST_RECURSION_DEPTH,
            "AST recursion depth exceeded; subtree truncated \
             (further occurrences suppressed). Definitions / call sites \
             inside the truncated subtree will be missing from the index."
        );
    }
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
/// * `depth` — current AST recursion depth (start at 0). Bounded by
///   [`MAX_AST_RECURSION_DEPTH`] to protect against pathological / generated code
///   blowing the worker thread stack (TSU-004).
/// * `stats` — mutable stats accumulator
/// * `config` — language-specific node name configuration
pub(crate) fn walk_code_stats(
    node: tree_sitter::Node,
    source: &[u8],
    nesting: u32,
    depth: u32,
    stats: &mut CodeStats,
    config: &CodeStatsConfig,
) {
    // TSU-004: depth guard. Without this, a method body with deeply nested
    // expressions (5 000+) overflows the worker thread stack (~8 MB default)
    // and SIGABRTs the indexer. Per-language parser walkers already enforce
    // this cap; walk_code_stats runs *inside* those walkers and must do the
    // same so the whole call chain stays bounded.
    if depth as usize >= MAX_AST_RECURSION_DEPTH {
        warn_ast_depth_exceeded("walk_code_stats", node);
        return;
    }

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
        // MINOR-14: tree-sitter grammars expose the operator via a named field
        // (`operator`) on `binary_expression`. Prefer field lookup and fall back
        // to positional `child(1)` for grammars that do NOT mark the field.
        if let Some(op) = node
            .child_by_field_name("operator")
            .or_else(|| node.child(1))
        {
            debug_assert!(
                !op.kind().is_empty(),
                "binary_op_node expected an operator child; got empty kind for node kind `{}`",
                kind,
            );
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
                    .and_then(|p| p.child_by_field_name("operator").or_else(|| p.child(1)))
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

            walk_code_stats(child, source, child_nesting, depth + 1, stats, config);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "tree_sitter_utils_tests.rs"]
mod tests;
