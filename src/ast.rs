/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! AST helpers for tree-sitter-biscuit
//!
//! This module provides high-level helpers for navigating, querying, and manipulating
//! the Biscuit AST using tree-sitter.

use ropey::Rope;
use std::collections::{HashMap, HashSet};

// ============================================================================
// Navigation Helpers
// ============================================================================

/// Find the enclosing rule, check, or policy node
pub fn find_enclosing_statement<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = *node;
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => return Some(current),
            _ => {
                current = current.parent()?;
            }
        }
    }
}

/// Find the enclosing rule_body node for scope-aware operations
pub fn find_enclosing_rule_body<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = *node;
    loop {
        match current.kind() {
            "rule_body" => return Some(current),
            _ => {
                current = current.parent()?;
            }
        }
    }
}

/// Find the enclosing predicate node
pub fn find_enclosing_predicate<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = *node;
    loop {
        if current.kind() == "predicate" {
            return Some(current);
        }
        current = current.parent()?;
    }
}

// ============================================================================
// Extraction Helpers
// ============================================================================

/// Extract all variables from a node
pub fn extract_variables(node: &tree_sitter::Node, rope: &Rope) -> HashSet<String> {
    let mut variables = HashSet::new();
    visit_variables(node, rope, &mut |var_text| {
        variables.insert(var_text.to_string());
    });
    variables
}

/// Extract the text of a node
pub fn extract_node_text(node: &tree_sitter::Node, rope: &Rope) -> String {
    rope.byte_slice(node.start_byte()..node.end_byte()).to_string()
}

/// Extract predicate parameters as a list of strings
pub fn extract_predicate_parameters(predicate_node: &tree_sitter::Node, rope: &Rope) -> Vec<String> {
    let mut params = Vec::new();
    let mut cursor = predicate_node.walk();
    for child in predicate_node.children(&mut cursor) {
        if child.kind() == "term" {
            params.push(extract_node_text(&child, rope));
        }
    }
    params
}

/// Extract predicate name
pub fn extract_predicate_name(predicate_node: &tree_sitter::Node, rope: &Rope) -> Option<String> {
    let name_node = predicate_node.child(0)?;
    if name_node.kind() == "nname" {
        Some(extract_node_text(&name_node, rope))
    } else {
        None
    }
}

/// Count predicate arity
pub fn count_predicate_arity(predicate_node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    let mut cursor = predicate_node.walk();
    for child in predicate_node.children(&mut cursor) {
        if child.kind() == "term" {
            count += 1;
        }
    }
    count
}

/// Check if a rule/check/policy has an origin clause
pub fn has_origin_clause(statement_node: &tree_sitter::Node) -> bool {
    let mut cursor = statement_node.walk();
    for child in statement_node.children(&mut cursor) {
        if child.kind() == "rule_body" {
            let mut body_cursor = child.walk();
            for body_child in child.children(&mut body_cursor) {
                if body_child.kind() == "origin_clause" {
                    return true;
                }
            }
        }
    }
    false
}

// ============================================================================
// Query Helpers
// ============================================================================

/// Find a rule definition by name and arity
pub fn find_rule_by_name_and_arity<'a>(
    root: &tree_sitter::Node<'a>,
    name: &str,
    arity: usize,
    rope: &Rope,
) -> Option<tree_sitter::Node<'a>> {
    let mut result_range: Option<(usize, usize)> = None;

    visit_node(root, &mut |node| {
        if result_range.is_some() {
            return;
        }
        if node.kind() == "rule" {
            if let Some(head) = node.child_by_field_name("head") {
                if head.kind() == "predicate" {
                    if let Some(rule_name) = extract_predicate_name(&head, rope) {
                        let rule_arity = count_predicate_arity(&head);
                        if rule_name == name && rule_arity == arity {
                            result_range = Some((node.start_byte(), node.end_byte()));
                        }
                    }
                }
            }
        }
    });

    if let Some((start, end)) = result_range {
        root.descendant_for_byte_range(start, end)
    } else {
        None
    }
}

/// Collect all rule names from the document
pub fn collect_all_rule_names(root: &tree_sitter::Node, rope: &Rope) -> HashSet<String> {
    let mut names = HashSet::new();

    visit_node(root, &mut |node| {
        if node.kind() == "rule" {
            if let Some(head) = node.child_by_field_name("head") {
                if head.kind() == "predicate" {
                    if let Some(name) = extract_predicate_name(&head, rope) {
                        names.insert(name);
                    }
                }
            }
        }
    });

    names
}

/// Generate a unique name not present in the given set
pub fn generate_unique_name(existing_names: &HashSet<String>, base_name: &str) -> String {
    let mut candidate = base_name.to_string();
    let mut counter = 1;
    while existing_names.contains(&candidate) {
        candidate = format!("{}_{}", base_name, counter);
        counter += 1;
    }
    candidate
}

// ============================================================================
// Manipulation Helpers
// ============================================================================

/// Substitute variables in a node using a substitution map
/// Returns the text with variables replaced
pub fn substitute_variables(
    node: &tree_sitter::Node,
    rope: &Rope,
    subst_map: &HashMap<String, String>,
) -> String {
    let mut result = String::new();
    let mut last_pos = node.start_byte();

    // Collect all variable replacements
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    visit_variables(node, rope, &mut |var_text| {
        if let Some(replacement) = subst_map.get(var_text) {
            // Find the actual node position
            visit_node(node, &mut |n| {
                if n.kind() == "variable" {
                    let text = extract_node_text(n, rope);
                    if text == var_text {
                        replacements.push((n.start_byte(), n.end_byte(), replacement.clone()));
                    }
                }
            });
        }
    });

    // Sort by position and deduplicate
    replacements.sort_by_key(|r| r.0);
    replacements.dedup_by_key(|r| r.0);

    // Build result with replacements
    for (start, end, replacement) in replacements {
        if start > last_pos {
            result.push_str(&rope.byte_slice(last_pos..start).to_string());
        }
        result.push_str(&replacement);
        last_pos = end;
    }

    if last_pos < node.end_byte() {
        result.push_str(&rope.byte_slice(last_pos..node.end_byte()).to_string());
    }

    result
}

// ============================================================================
// Node Type Helpers
// ============================================================================

/// Check if a node is a variable
pub fn is_variable(node: &tree_sitter::Node) -> bool {
    node.kind() == "variable"
}

/// Check if a node is a literal (string, number, boolean, date, bytes)
pub fn is_literal(node: &tree_sitter::Node) -> bool {
    matches!(node.kind(), "string" | "number" | "boolean" | "date" | "bytes")
}

// ============================================================================
// Visitor Helpers
// ============================================================================

/// Visit all nodes in a tree recursively
pub fn visit_node<F>(node: &tree_sitter::Node, visitor: &mut F)
where
    F: FnMut(&tree_sitter::Node),
{
    visitor(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(&child, visitor);
    }
}

/// Visit all variables in a tree and call the callback with their text
pub fn visit_variables<F>(node: &tree_sitter::Node, rope: &Rope, callback: &mut F)
where
    F: FnMut(&str),
{
    visit_node(node, &mut |n| {
        if is_variable(n) {
            let var_text = extract_node_text(n, rope);
            callback(&var_text);
        }
    });
}

// ============================================================================
// Rule Body Helpers
// ============================================================================

/// Represents an item in a rule body
#[derive(Debug, Clone, PartialEq)]
pub struct RuleBodyItem {
    pub text: String,
    pub kind: RuleBodyItemKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleBodyItemKind {
    Predicate,
    Expression,
}

/// Extract all items from a rule body
pub fn extract_rule_body_items(rule_body_node: &tree_sitter::Node, rope: &Rope) -> Vec<RuleBodyItem> {
    let mut items = Vec::new();
    let mut cursor = rule_body_node.walk();

    for child in rule_body_node.children(&mut cursor) {
        let kind = match child.kind() {
            "predicate" => RuleBodyItemKind::Predicate,
            "expression" => RuleBodyItemKind::Expression,
            _ => continue,
        };

        items.push(RuleBodyItem {
            text: extract_node_text(&child, rope),
            kind,
            start: child.start_byte(),
            end: child.end_byte(),
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_biscuit(code: &str) -> (tree_sitter::Tree, Rope) {
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_biscuit::language()).unwrap();
        let tree = parser.parse(code, None).unwrap();
        let rope = Rope::from_str(code);
        (tree, rope)
    }

    #[test]
    fn test_find_enclosing_statement() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let root = tree.root_node();

        // Find a variable node by byte range
        let mut var_range = None;
        visit_node(&root, &mut |node| {
            if node.kind() == "variable" && var_range.is_none() {
                var_range = Some((node.start_byte(), node.end_byte()));
            }
        });

        let (start, end) = var_range.unwrap();
        let var_node = root.descendant_for_byte_range(start, end).unwrap();
        let statement = find_enclosing_statement(&var_node).unwrap();
        assert_eq!(statement.kind(), "check");
    }

    #[test]
    fn test_extract_variables() {
        let (tree, rope) = parse_biscuit("rule($x, $y) <- user($x), role($y);");
        let root = tree.root_node();

        let vars = extract_variables(&root, &rope);
        assert_eq!(vars.len(), 2);
        assert!(vars.contains("$x"));
        assert!(vars.contains("$y"));
    }

    #[test]
    fn test_extract_predicate_name() {
        let (tree, rope) = parse_biscuit("user($x);");
        let root = tree.root_node();

        // Find predicate by byte range
        let mut pred_range = None;
        visit_node(&root, &mut |node| {
            if node.kind() == "predicate" && pred_range.is_none() {
                pred_range = Some((node.start_byte(), node.end_byte()));
            }
        });

        let (start, end) = pred_range.unwrap();
        let predicate = root.descendant_for_byte_range(start, end).unwrap();
        let name = extract_predicate_name(&predicate, &rope).unwrap();
        assert_eq!(name, "user");
    }

    #[test]
    fn test_count_predicate_arity() {
        let (tree, _rope) = parse_biscuit("user($x, $y, $z);");
        let root = tree.root_node();

        // Find predicate by byte range
        let mut pred_range = None;
        visit_node(&root, &mut |node| {
            if node.kind() == "predicate" && pred_range.is_none() {
                pred_range = Some((node.start_byte(), node.end_byte()));
            }
        });

        let (start, end) = pred_range.unwrap();
        let predicate = root.descendant_for_byte_range(start, end).unwrap();
        assert_eq!(count_predicate_arity(&predicate), 3);
    }

    #[test]
    fn test_has_origin_clause() {
        let (tree1, _) = parse_biscuit("check if user($u) trusting authority;");
        let root1 = tree1.root_node();

        // Find check node by byte range
        let mut check1_range = None;
        visit_node(&root1, &mut |node| {
            if node.kind() == "check" && check1_range.is_none() {
                check1_range = Some((node.start_byte(), node.end_byte()));
            }
        });
        let (start, end) = check1_range.unwrap();
        let check1 = root1.descendant_for_byte_range(start, end).unwrap();
        assert!(has_origin_clause(&check1));

        let (tree2, _) = parse_biscuit("check if user($u);");
        let root2 = tree2.root_node();

        // Find check node by byte range
        let mut check2_range = None;
        visit_node(&root2, &mut |node| {
            if node.kind() == "check" && check2_range.is_none() {
                check2_range = Some((node.start_byte(), node.end_byte()));
            }
        });
        let (start, end) = check2_range.unwrap();
        let check2 = root2.descendant_for_byte_range(start, end).unwrap();
        assert!(!has_origin_clause(&check2));
    }

    #[test]
    fn test_find_rule_by_name_and_arity() {
        let code = r#"
user($x) <- identity($x);
user($x, $role) <- identity($x), role($role);
"#;
        let (tree, rope) = parse_biscuit(code);
        let root = tree.root_node();

        // Find user/1
        let rule1 = find_rule_by_name_and_arity(&root, "user", 1, &rope).unwrap();
        assert_eq!(rule1.kind(), "rule");

        // Find user/2
        let rule2 = find_rule_by_name_and_arity(&root, "user", 2, &rope).unwrap();
        assert_eq!(rule2.kind(), "rule");

        // Should not find user/3
        let rule3 = find_rule_by_name_and_arity(&root, "user", 3, &rope);
        assert!(rule3.is_none());
    }

    #[test]
    fn test_generate_unique_name() {
        let mut names = HashSet::new();
        names.insert("foo".to_string());
        names.insert("foo_1".to_string());

        let unique = generate_unique_name(&names, "foo");
        assert_eq!(unique, "foo_2");
    }

    #[test]
    fn test_substitute_variables() {
        let (tree, rope) = parse_biscuit("user($x), role($x, $y);");
        let root = tree.root_node();

        let mut subst_map = HashMap::new();
        subst_map.insert("$x".to_string(), "$admin".to_string());
        subst_map.insert("$y".to_string(), "\"moderator\"".to_string());

        let result = substitute_variables(&root, &rope, &subst_map);
        assert!(result.contains("$admin"));
        assert!(result.contains("\"moderator\""));
    }

    #[test]
    fn test_extract_rule_body_items() {
        let (tree, rope) = parse_biscuit("rule($x) <- user($x), $x == \"admin\";");
        let root = tree.root_node();

        // Find rule_body by byte range
        let mut rule_body_range = None;
        visit_node(&root, &mut |node| {
            if node.kind() == "rule_body" && rule_body_range.is_none() {
                rule_body_range = Some((node.start_byte(), node.end_byte()));
            }
        });

        let (start, end) = rule_body_range.unwrap();
        let rule_body = root.descendant_for_byte_range(start, end).unwrap();

        let items = extract_rule_body_items(&rule_body, &rope);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, RuleBodyItemKind::Predicate);
        assert_eq!(items[1].kind, RuleBodyItemKind::Expression);
    }

    #[test]
    fn test_is_literal() {
        let (tree, _) = parse_biscuit("user(\"alice\", 42, true);");
        let root = tree.root_node();

        let mut literals = Vec::new();
        visit_node(&root, &mut |node| {
            if is_literal(node) {
                literals.push(node.kind().to_string());
            }
        });

        assert!(literals.contains(&"string".to_string()));
        assert!(literals.contains(&"number".to_string()));
        assert!(literals.contains(&"boolean".to_string()));
    }
}
