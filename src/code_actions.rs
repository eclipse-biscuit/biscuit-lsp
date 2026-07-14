/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use ropey::Rope;
use tower_lsp::lsp_types::*;

use std::collections::HashMap;

use crate::ast::{self, RuleBodyItemKind};
use crate::offset_to_position;

/// Create code action to add a trusting clause to a rule/check/policy
pub fn create_add_trusting_clause_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing rule/check/policy
    let current = ast::find_enclosing_statement(node)?;

    // Check if it already has a trusting clause
    if ast::has_origin_clause(&current) {
        return None; // Already has trusting clause
    }

    // Find where to insert the trusting clause
    // For rules: after the rule_body
    // For checks/policies: after the last rule_body
    let insert_position = {
        let mut last_body_end = None;
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            if child.kind() == "rule_body" {
                last_body_end = Some(child.end_byte());
            }
        }
        last_body_end?
    };

    let insert_pos = offset_to_position(insert_position, rope)?;

    // Create trusting authority clause
    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(insert_pos, insert_pos),
            new_text: " trusting authority".to_string(),
        }],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Add trusting clause (authority)".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    }))
}

/// Create code actions to convert between check variants (check if/check all/reject if)
/// Returns all possible conversions from the current variant
pub fn create_convert_check_variant_actions(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    // Find the enclosing check
    let current = match ast::find_enclosing_statement(node) {
        Some(node) if node.kind() == "check" => node,
        _ => return Vec::new(),
    };

    // Extract the check variant by examining the check text
    // The grammar defines: choice("check if", "check all", "reject if")
    // We need to find which variant this is by looking at the beginning of the node
    let start_byte = current.start_byte();

    // Look at first ~12 bytes to determine variant (longest is "reject if" = 9 chars)
    let sample_end = (start_byte + 12).min(current.end_byte());
    let check_prefix = rope.byte_slice(start_byte..sample_end).to_string().trim().to_lowercase();

    let current_variant = if check_prefix.starts_with("check if") {
        "check if"
    } else if check_prefix.starts_with("check all") {
        "check all"
    } else if check_prefix.starts_with("reject if") {
        "reject if"
    } else {
        return Vec::new();
    };

    // All possible variants
    let all_variants = vec!["check if", "check all", "reject if"];

    // Filter out the current variant
    let target_variants: Vec<&str> = all_variants
        .into_iter()
        .filter(|&v| v != current_variant)
        .collect();

    let variant_len = current_variant.len();
    let start_pos = match offset_to_position(start_byte, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };
    let end_pos = match offset_to_position(start_byte + variant_len, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };

    // Create a code action for each target variant
    target_variants
        .into_iter()
        .map(|target_variant| {
            let mut changes = std::collections::HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range::new(start_pos, end_pos),
                    new_text: target_variant.to_string(),
                }],
            );

            CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Convert to '{}'", target_variant),
                kind: Some(CodeActionKind::REFACTOR),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                command: None,
                is_preferred: None,
                disabled: None,
                data: None,
            })
        })
        .collect()
}

/// Create code actions to convert between policy types (allow if/deny if)
pub fn create_convert_policy_type_actions(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    // Find the enclosing policy
    let current = match ast::find_enclosing_statement(node) {
        Some(node) if node.kind() == "policy" => node,
        _ => return Vec::new(),
    };

    // Extract the policy type by examining the policy text
    // The grammar defines: choice("allow if", "deny if")
    let start_byte = current.start_byte();

    // Look at first ~10 bytes to determine type (longest is "allow if" = 8 chars)
    let sample_end = (start_byte + 10).min(current.end_byte());
    let policy_prefix = rope.byte_slice(start_byte..sample_end).to_string().trim().to_lowercase();

    let (current_type, target_type) = if policy_prefix.starts_with("allow if") {
        ("allow if", "deny if")
    } else if policy_prefix.starts_with("deny if") {
        ("deny if", "allow if")
    } else {
        return Vec::new();
    };

    let type_len = current_type.len();
    let start_pos = match offset_to_position(start_byte, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };
    let end_pos = match offset_to_position(start_byte + type_len, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(start_pos, end_pos),
            new_text: target_type.to_string(),
        }],
    );

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert to '{}'", target_type),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    })]
}

/// Create code actions to convert a parameter to literal values
pub fn create_convert_parameter_to_literal_actions(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    // Check if cursor is on a parameter
    let target_node = match node.kind() {
        "param" => *node,
        _ => {
            // Try parent
            match node.parent() {
                Some(parent) if parent.kind() == "param" => parent,
                _ => return Vec::new(),
            }
        }
    };

    let start = target_node.start_byte();
    let end = target_node.end_byte();

    let start_pos = match offset_to_position(start, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };
    let end_pos = match offset_to_position(end, rope) {
        Some(pos) => pos,
        None => return Vec::new(),
    };

    // Get current date in RFC3339 format
    let now = chrono::Utc::now();
    let date_str = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(start_pos, end_pos),
            new_text: date_str.clone(),
        }],
    );

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert to current date ({})", date_str),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    })]
}

/// Create code action to convert a literal value to a parameter placeholder
pub fn create_convert_to_parameter_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
    _start_offset: usize,
    _end_offset: usize,
) -> Option<CodeActionOrCommand> {
    // Check if cursor is on a literal value
    let target_node = if ast::is_literal(node) {
        *node
    } else {
        // Try parent
        let parent = node.parent()?;
        if ast::is_literal(&parent) {
            parent
        } else {
            return None;
        }
    };

    let start = target_node.start_byte();
    let end = target_node.end_byte();
    let value = rope.byte_slice(start..end).to_string();

    // Generate a parameter name based on the value type
    let param_name = match target_node.kind() {
        "string" => "value",
        "number" => "number",
        "boolean" => "flag",
        "date" => "timestamp",
        "bytes" => "data",
        _ => "param",
    };

    let start_pos = offset_to_position(start, rope)?;
    let end_pos = offset_to_position(end, rope)?;

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(start_pos, end_pos),
            new_text: format!("{{{}}}", param_name),
        }],
    );

    // Truncate value for display (max 30 chars)
    let display_value = if value.len() > 30 {
        format!("{}...", value.chars().take(27).collect::<String>())
    } else {
        value
    };

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert '{}' to parameter", display_value),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }))
}

/// Create code action to extract a check/policy body into a new rule
pub fn create_extract_rule_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
    root: tree_sitter::Node,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing check or policy
    let current = ast::find_enclosing_statement(node)?;
    let parent_kind = current.kind();

    if parent_kind != "check" && parent_kind != "policy" {
        return None;
    }

    // Find the rule_body
    let rule_body_node = ast::find_enclosing_rule_body(node)?;
    let body_start = rule_body_node.start_byte();
    let body_end = rule_body_node.end_byte();
    let body_text = rope.byte_slice(body_start..body_end).to_string();

    // Extract all variables from the rule body
    let variables = ast::extract_variables(&rule_body_node, rope);

    // Generate unique rule name
    let existing_names = ast::collect_all_rule_names(&root, rope);
    let rule_name = ast::generate_unique_name(&existing_names, "extracted_rule");

    let params = if variables.is_empty() {
        String::new()
    } else {
        let mut vars: Vec<_> = variables.into_iter().collect();
        vars.sort();
        vars.join(", ")
    };

    // Create the new rule
    let new_rule = format!("{}({}) <- {};\n", rule_name, params, body_text);

    // Find where to insert (at the start of the line containing the check/policy)
    let insert_line_start = {
        let start_byte = current.start_byte();
        let line = rope.try_byte_to_line(start_byte).ok()?;
        rope.try_line_to_byte(line).ok()?
    };
    let insert_pos = offset_to_position(insert_line_start, rope)?;

    // Replace the body with a call to the new rule
    let body_start_pos = offset_to_position(body_start, rope)?;
    let body_end_pos = offset_to_position(body_end, rope)?;
    let replacement = format!("{}({})", rule_name, params);

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            // Insert the new rule at the beginning of the line
            TextEdit {
                range: Range::new(insert_pos, insert_pos),
                new_text: new_rule,
            },
            // Replace the body with a predicate call
            TextEdit {
                range: Range::new(body_start_pos, body_end_pos),
                new_text: replacement,
            },
        ],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Extract {} body to rule", parent_kind),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }))
}


/// Create code action to inline a rule call
pub fn create_inline_rule_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
    root: tree_sitter::Node,
) -> Option<CodeActionOrCommand> {
    // Find if we're on a predicate
    let predicate_node = ast::find_enclosing_predicate(node)?;

    // Get predicate name and arity
    let pred_name = ast::extract_predicate_name(&predicate_node, rope)?;
    let arity = ast::count_predicate_arity(&predicate_node);

    // Find the rule definition with matching name and arity
    let rule_def = ast::find_rule_by_name_and_arity(&root, &pred_name, arity, rope)?;

    // Get rule head parameters
    let rule_head = rule_def.child_by_field_name("head")?;
    let rule_params = ast::extract_predicate_parameters(&rule_head, rope);

    // Get call arguments
    let call_args = ast::extract_predicate_parameters(&predicate_node, rope);

    // Check arity match
    if rule_params.len() != call_args.len() {
        return None; // Arity mismatch, can't inline safely
    }

    // Get rule body
    let rule_body = rule_def.child_by_field_name("body")?;

    // Build parameter substitution map
    let mut subst_map = HashMap::new();
    for (param, arg) in rule_params.iter().zip(call_args.iter()) {
        subst_map.insert(param.clone(), arg.clone());
    }

    // Perform AST-based variable substitution
    let inlined_body = ast::substitute_variables(&rule_body, rope, &subst_map);

    // Replace the predicate with the inlined body
    let pred_start = predicate_node.start_byte();
    let pred_end = predicate_node.end_byte();
    let pred_start_pos = offset_to_position(pred_start, rope)?;
    let pred_end_pos = offset_to_position(pred_end, rope)?;

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(pred_start_pos, pred_end_pos),
            new_text: inlined_body,
        }],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Inline rule '{}'", pred_name),
        kind: Some(CodeActionKind::REFACTOR_INLINE),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }))
}


/// Create code action to sort a rule body
pub fn create_sort_rule_body_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing rule, check, or policy
    let _statement = ast::find_enclosing_statement(node)?;

    // Find the rule_body
    let body_node = ast::find_enclosing_rule_body(node)?;

    // Extract all predicates and expressions from the body
    let items = ast::extract_rule_body_items(&body_node, rope);

    if items.len() <= 1 {
        return None; // Nothing to sort
    }

    // Sort items: predicates before expressions, then alphabetically
    let mut sorted_items = items.clone();
    sorted_items.sort_by(|a, b| {
        match (a.kind, b.kind) {
            (RuleBodyItemKind::Predicate, RuleBodyItemKind::Expression) => std::cmp::Ordering::Less,
            (RuleBodyItemKind::Expression, RuleBodyItemKind::Predicate) => std::cmp::Ordering::Greater,
            _ => a.text.cmp(&b.text),
        }
    });

    // Check if already sorted
    if items.iter().map(|i| &i.text).eq(sorted_items.iter().map(|i| &i.text)) {
        return None; // Already sorted
    }

    // Build the sorted text with proper separators
    let sorted_text = sorted_items
        .iter()
        .map(|i| i.text.clone())
        .collect::<Vec<_>>()
        .join(", ");

    // Find the range of all items
    let start = items.iter().map(|i| i.start).min()?;
    let end = items.iter().map(|i| i.end).max()?;

    let start_pos = offset_to_position(start, rope)?;
    let end_pos = offset_to_position(end, rope)?;

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(start_pos, end_pos),
            new_text: sorted_text,
        }],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Sort rule body".to_string(),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }))
}
