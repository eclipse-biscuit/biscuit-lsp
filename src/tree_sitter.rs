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

/// Biscuit language keywords with their descriptions
/// Format: (keyword, description, has_snippet)
pub const KEYWORDS: &[(&str, &str, bool)] = &[
    ("check if", "Check constraint", true),
    ("check all", "Check all constraints", true),
    ("reject if", "Reject if condition", true),
    ("allow if", "Allow if condition (policy)", true),
    ("deny if", "Deny if condition (policy)", true),
    ("trusting", "Origin clause", true),
    ("previous", "Trust previous block", false),
    ("authority", "Trust authority block", false),
    ("true", "Boolean true", false),
    ("false", "Boolean false", false),
    ("null", "Null value", false),
];

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

/// Find the enclosing rule, check, or policy node for the given byte offset
/// If no proper context is found, returns the nearest ERROR node as a fallback
pub fn find_enclosing_context(
    root: tree_sitter::Node,
    byte_offset: usize,
) -> Option<tree_sitter::Node> {
    let mut current = root.descendant_for_byte_range(byte_offset, byte_offset);

    // If we got the root and we're not at position 0, try one byte before
    // This handles cases where cursor is at the very end of content
    if let Some(node) = current {
        if node.kind() == "source_file" && byte_offset > 0 {
            current = root.descendant_for_byte_range(
                byte_offset.saturating_sub(1),
                byte_offset.saturating_sub(1),
            );
        }
    }

    let mut current = current?;
    let mut error_fallback = None;

    // Walk up the tree to find a rule, check, or policy node
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => return Some(current),
            "ERROR"
                // Remember the ERROR node as a fallback
                if error_fallback.is_none() => {
                    error_fallback = Some(current);
                }
            _ => {}
        }

        // Before moving to parent, check if any siblings are rule/check/policy
        // This handles cases where cursor is at a sibling of the context (e.g., at ";")
        if let Some(parent) = current.parent() {
            let mut cursor = parent.walk();
            for sibling in parent.children(&mut cursor) {
                if matches!(sibling.kind(), "rule" | "check" | "policy")
                    && sibling.start_byte() <= byte_offset
                    && sibling.end_byte() >= byte_offset
                {
                    return Some(sibling);
                }
            }
            current = parent;
        } else {
            // Reached the root without finding a proper context
            // Use ERROR node as fallback if available
            return error_fallback;
        }
    }
}

/// Count arguments in a predicate (fact or rule head)
/// Facts use "fact_term" children, predicates use "term" children
pub fn count_predicate_arguments(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Both "term" (for predicates) and "fact_term" (for facts) are argument nodes
        if child.kind() == "term" || child.kind() == "fact_term" {
            count += 1;
        }
    }
    count
}

/// Extract all facts and predicates from the tree
/// Returns unique (name, arity) pairs
pub fn get_symbols(tree: &Tree, rope: &Rope) -> Vec<(String, usize)> {
    let mut symbols = std::collections::HashSet::new();
    let root = tree.root_node();

    visit_node(&root, &mut |node| {
        match node.kind() {
            "fact" | "predicate" => {
                // Both facts and predicates have structure: nname "(" args... ")"
                let name_node = if node.kind() == "fact" {
                    node.child_by_field_name("name").or_else(|| node.child(0))
                } else {
                    node.child(0)
                };

                if let Some(name_node) = name_node {
                    if name_node.kind() == "nname" {
                        let start = name_node.start_byte();
                        let end = name_node.end_byte();
                        let name = rope.byte_slice(start..end).to_string();

                        // Skip names containing newlines (error recovery artifacts)
                        if name.contains('\n') {
                            return;
                        }

                        let arity = count_predicate_arguments(node);

                        symbols.insert((name, arity));
                    }
                }
            }
            _ => {}
        }
    });

    symbols.into_iter().collect()
}

/// Extract all variables from a given scope
pub fn get_variables_in_scope(
    context: tree_sitter::Node,
    rope: &Rope,
    byte_offset: usize,
) -> Vec<String> {
    let mut variables = std::collections::HashSet::new();

    visit_node(&context, &mut |node| {
        if node.kind() == "variable" {
            let start = node.start_byte();
            let end = node.end_byte();
            // Exclude only the variable currently being typed (cursor is within its range)
            // Include all other variables in scope, even if they appear later
            if !(byte_offset >= start && byte_offset <= end) {
                let var_text = rope.byte_slice(start..end).to_string();
                // Strip the leading "$" for completion
                let var_name = var_text.strip_prefix('$').unwrap_or(&var_text);
                variables.insert(var_name.to_string());
            }
        }
    });

    variables.into_iter().collect()
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

/// Check if cursor is typing a variable (using tree-sitter grammar)
/// This includes being inside a variable node or right after one
pub fn is_typing_variable(node: tree_sitter::Node, byte_offset: usize, rope: &Rope) -> bool {
    // Check if current node is a variable
    if node.kind() == "variable" {
        return true;
    }

    // Check if any parent is a variable
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "variable" && byte_offset >= n.start_byte() && byte_offset <= n.end_byte() {
            return true;
        }
        current = n.parent();
    }

    // Also check if cursor is right after a variable (e.g., cursor at the end of "$fo")
    // This handles the case where cursor is at the boundary between variable and next token
    // Recursively search for a variable node that ends at cursor position
    fn find_variable_ending_at(node: tree_sitter::Node, byte_offset: usize) -> bool {
        // Check current node
        if node.kind() == "variable" && node.end_byte() == byte_offset {
            return true;
        }

        // If this node ends at or before cursor, check its children
        if node.end_byte() >= byte_offset {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if find_variable_ending_at(child, byte_offset) {
                    return true;
                }
            }
        }

        false
    }

    // Search from root to find any variable ending at cursor
    let root = {
        let mut n = node;
        while let Some(p) = n.parent() {
            n = p;
        }
        n
    };
    if find_variable_ending_at(root, byte_offset) {
        return true;
    }

    // Fallback: if tree-sitter doesn't detect a variable (e.g., incomplete parse with lone "$"),
    // check if the character immediately before cursor is "$"
    if byte_offset > 0 && rope.byte(byte_offset - 1) == b'$' {
        return true;
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

    // Method context tests have been moved to tests/completion/methods.yaml

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

    #[test]
    fn test_find_enclosing_context_in_rule() {
        let (tree, _rope) = parse_biscuit("admin($u) <- user($u), role($u, \"admin\");");
        let root = tree.root_node();

        // Position inside the rule body
        let offset = 20; // Inside "user($u)"
        let context = find_enclosing_context(root, offset);

        assert!(context.is_some());
        assert_eq!(context.unwrap().kind(), "rule");
    }

    #[test]
    fn test_find_enclosing_context_in_check() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let root = tree.root_node();

        // Position inside the check
        let offset = 12; // Inside "user"
        let context = find_enclosing_context(root, offset);

        assert!(context.is_some());
        assert_eq!(context.unwrap().kind(), "check");
    }

    #[test]
    fn test_find_enclosing_context_outside() {
        let (tree, _rope) = parse_biscuit("user(\"alice\");");
        let root = tree.root_node();

        // Position in a fact (not in rule/check/policy)
        let offset = 5;
        let context = find_enclosing_context(root, offset);

        // Should not find a context (facts are not rule/check/policy)
        assert!(context.is_none());
    }

    fn find_first_node_of_kind<'a>(
        node: tree_sitter::Node<'a>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_first_node_of_kind(child, kind) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn test_count_predicate_arguments_fact_one() {
        let (tree, _rope) = parse_biscuit("user(\"alice\");");
        let root = tree.root_node();

        // Find the fact node
        let fact = find_first_node_of_kind(root, "fact").unwrap();
        let count = count_predicate_arguments(&fact);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_predicate_arguments_fact_multiple() {
        let (tree, _rope) = parse_biscuit("role(\"alice\", \"admin\");");
        let root = tree.root_node();

        // Find the fact node
        let fact = find_first_node_of_kind(root, "fact").unwrap();
        let count = count_predicate_arguments(&fact);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_predicate_arguments_one() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let root = tree.root_node();

        // Find the predicate node
        let predicate = find_first_node_of_kind(root, "predicate").unwrap();
        let count = count_predicate_arguments(&predicate);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_predicate_arguments_multiple() {
        let (tree, rope) = parse_biscuit("admin($u) <- user($u), role($u, \"admin\");");
        let root = tree.root_node();

        // Collect all predicates
        let mut predicates = Vec::new();
        visit_node(&root, &mut |node| {
            if node.kind() == "predicate" {
                if let Some(name_node) = node.child(0) {
                    if name_node.kind() == "nname" {
                        let start = name_node.start_byte();
                        let end = name_node.end_byte();
                        let name = rope.byte_slice(start..end).to_string();
                        if name == "role" {
                            // Found the role predicate, count its arguments
                            predicates.push(count_predicate_arguments(node));
                        }
                    }
                }
            }
        });

        // Should have found the "role" predicate with 2 arguments
        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0], 2);
    }

    #[test]
    fn test_get_symbols_deduplicates() {
        // Same predicate name/arity appears as both fact and rule head
        let (tree, rope) = parse_biscuit(
            r#"
            user("alice");
            user($u) <- admin($u);
            "#,
        );

        let symbols = get_symbols(&tree, &rope);

        // "user/1" should appear only once, not twice
        let user_symbols: Vec<_> = symbols
            .iter()
            .filter(|(name, arity)| name == "user" && *arity == 1)
            .collect();

        assert_eq!(user_symbols.len(), 1);

        // Should also have admin/1
        let admin_symbols: Vec<_> = symbols
            .iter()
            .filter(|(name, arity)| name == "admin" && *arity == 1)
            .collect();

        assert_eq!(admin_symbols.len(), 1);
    }

    #[test]
    fn test_get_symbols_multiline_error_node() {
        // Error case where tree-sitter's error recovery causes nname to span lines
        // e.g., typing "user" on one line then "another_predicate();" on the next
        // creates a malformed nname containing "user\nanother_predicate"
        let (tree, rope) = parse_biscuit(
            r#"user
another_predicate();"#,
        );

        let symbols = get_symbols(&tree, &rope);

        // Should not extract any malformed names with newlines
        for (name, _arity) in &symbols {
            assert!(
                !name.contains('\n'),
                "Symbol name should not contain newlines: {:?}",
                name
            );
        }
    }
}
