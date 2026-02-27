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
}