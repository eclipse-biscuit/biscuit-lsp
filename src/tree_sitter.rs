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

/// Convert byte offset to LSP position
fn offset_to_position(offset: usize, rope: &Rope) -> Option<Position> {
    let line = rope.try_byte_to_line(offset).ok()?;
    let first_char_of_line = rope.try_line_to_byte(line).ok()?;
    let column = offset - first_char_of_line;
    Some(Position::new(line as u32, column as u32))
}
