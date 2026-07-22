/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tree-sitter helpers for biscuit LSP

use ropey::Rope;
use tower_lsp::lsp_types::*;
use tree_sitter::Parser;

// Re-export commonly used tree-sitter types for convenience
pub use tree_sitter::Tree;

/// Document data including rope and parse tree
#[derive(Debug)]
pub struct DocumentData {
    pub rope: Rope,
    pub tree: Option<Tree>,
}

impl DocumentData {
    /// Create a new empty document
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            tree: None,
        }
    }

    /// Create a document from text
    pub fn from_text(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let tree = parse_text(text);
        Self { rope, tree }
    }

    /// Get tree-sitter syntax errors
    pub fn get_syntax_errors(&self) -> Vec<Diagnostic> {
        match &self.tree {
            Some(tree) => get_tree_sitter_errors(tree, &self.rope),
            None => Vec::new(),
        }
    }
}

impl Default for DocumentData {
    fn default() -> Self {
        Self::new()
    }
}

/// Create and configure a tree-sitter parser for biscuit
fn create_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_biscuit::language())
        .expect("Error loading biscuit grammar");
    parser
}

/// Parse text into a tree
fn parse_text(text: &str) -> Option<Tree> {
    let mut parser = create_parser();
    parser.parse(text, None)
}

/// Extract syntax errors from tree-sitter ERROR nodes
fn get_tree_sitter_errors(tree: &Tree, rope: &Rope) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let root = tree.root_node();

    // Walk the tree to find ERROR and MISSING nodes
    visit_node(&root, &mut |node| {
        if node.is_error() || node.is_missing() {
            let start = node.start_byte();
            let end = node.end_byte();

            // Get a meaningful error message
            let message = if node.is_missing() {
                format!("Missing: {}", node.kind())
            } else {
                // For ERROR nodes, try to show what was expected vs what was found
                let text_slice = if end > start && (end - start) < 50 {
                    rope.byte_slice(start..end).to_string()
                } else {
                    "<unknown>".to_string()
                };
                format!("Syntax error near: {}", text_slice)
            };

            if let (Some(start_pos), Some(end_pos)) = (
                offset_to_position(start, rope),
                offset_to_position(end.max(start + 1), rope),
            ) {
                diagnostics.push(Diagnostic::new(
                    Range::new(start_pos, end_pos),
                    Some(DiagnosticSeverity::ERROR),
                    None,
                    None,
                    message,
                    None,
                    None,
                ));
            }
        }
    });

    diagnostics
}

/// Visit all nodes in the tree recursively
fn visit_node<F>(node: &tree_sitter::Node, visitor: &mut F)
where
    F: FnMut(&tree_sitter::Node),
{
    visitor(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(&child, visitor);
    }
}

/// Find the most specific node at the cursor position
/// Falls back to parent if cursor is in ERROR or MISSING node
pub fn find_node_at_cursor(root: tree_sitter::Node, byte_offset: usize) -> tree_sitter::Node {
    let mut node = root
        .descendant_for_byte_range(byte_offset, byte_offset)
        .unwrap_or(root);

    // If we're in an ERROR or MISSING node, try to use the parent for better context
    while (node.is_error() || node.is_missing()) && node.parent().is_some() {
        node = node.parent().unwrap();
    }

    node
}

/// Check if we're in a context where method completion makes sense
pub fn is_in_method_context(node: tree_sitter::Node, byte_offset: usize, rope: &Rope) -> bool {
    // Check if we're inside a methods node
    let mut current = node;
    loop {
        if current.kind() == "methods" {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // Check if the previous character is a dot
    if byte_offset > 0 {
        let prev_char = rope.byte_slice((byte_offset - 1)..byte_offset);
        if prev_char == "." {
            return true;
        }
    }

    false
}

/// Convert LSP position to byte offset in rope
pub fn position_to_offset(position: &Position, rope: &Rope) -> Option<usize> {
    let line = position.line as usize;
    let character = position.character as usize;
    let line_start = rope.try_line_to_char(line).ok()?;
    Some(line_start + character)
}

/// Convert byte offset to LSP position
fn offset_to_position(offset: usize, rope: &Rope) -> Option<Position> {
    let line = rope.try_byte_to_line(offset).ok()?;
    let first_char_of_line = rope.try_line_to_byte(line).ok()?;
    let column = offset - first_char_of_line;
    Some(Position::new(line as u32, column as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_biscuit(code: &str) -> (tree_sitter::Tree, Rope) {
        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_biscuit::language())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let rope = Rope::from_str(code);
        (tree, rope)
    }

    #[test]
    fn test_find_node_at_cursor_basic() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let root = tree.root_node();

        // Find node at position inside "user"
        let user_offset = 10; // Position of "u" in "user"
        let node = find_node_at_cursor(root, user_offset);

        assert_eq!(node.kind(), "nname");
    }

    #[test]
    fn test_find_node_at_cursor_falls_back_on_error() {
        let (tree, _rope) = parse_biscuit("check if user(");
        let root = tree.root_node();

        // Find node at end of broken code
        let node = find_node_at_cursor(root, 13);

        // Should fall back to parent instead of staying in ERROR node
        assert!(!node.is_error());
    }

    #[test]
    fn test_is_in_method_context_after_dot() {
        let (tree, rope) = parse_biscuit("check if $x.");
        let root = tree.root_node();

        // Position right after the dot
        let after_dot_offset = 12;
        let node = find_node_at_cursor(root, after_dot_offset);

        assert!(is_in_method_context(node, after_dot_offset, &rope));
    }

    #[test]
    fn test_is_in_method_context_inside_methods() {
        let (tree, rope) = parse_biscuit("check if $x.contains(\"foo\");");
        let root = tree.root_node();

        // Position inside "contains"
        let contains_offset = 14;
        let node = find_node_at_cursor(root, contains_offset);

        assert!(is_in_method_context(node, contains_offset, &rope));
    }

    #[test]
    fn test_is_not_in_method_context() {
        let (tree, rope) = parse_biscuit("check if user($u);");
        let root = tree.root_node();

        // Position at "$u" - not in method context
        let dollar_u_offset = 14;
        let node = find_node_at_cursor(root, dollar_u_offset);

        assert!(!is_in_method_context(node, dollar_u_offset, &rope));
    }

    #[test]
    fn test_position_to_offset_first_line() {
        let rope = Rope::from_str("check if user($u);");

        // Position at "$" (line 0, character 14)
        let position = Position::new(0, 14);
        let offset = position_to_offset(&position, &rope).unwrap();

        assert_eq!(offset, 14);
    }

    #[test]
    fn test_position_to_offset_multiline() {
        let rope = Rope::from_str("check if user($u);\nallow if admin($a);");

        // Position at "a" in second line (line 1, character 6)
        let position = Position::new(1, 6);
        let offset = position_to_offset(&position, &rope).unwrap();

        // First line is 19 chars (including newline), second line starts at 19
        assert_eq!(offset, 19 + 6);
    }

    #[test]
    fn test_offset_to_position_first_line() {
        let rope = Rope::from_str("check if user($u);");

        // Offset 14 is at "$"
        let position = offset_to_position(14, &rope).unwrap();

        assert_eq!(position.line, 0);
        assert_eq!(position.character, 14);
    }

    #[test]
    fn test_offset_to_position_multiline() {
        let rope = Rope::from_str("check if user($u);\nallow if admin($a);");

        // Offset 25 is at "a" in second line
        let position = offset_to_position(25, &rope).unwrap();

        assert_eq!(position.line, 1);
        assert_eq!(position.character, 6);
    }
}
