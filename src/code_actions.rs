/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use ropey::Rope;
use tower_lsp::lsp_types::*;

use std::collections::HashSet;

use crate::offset_to_position;

/// Create code action to add a trusting clause to a rule/check/policy
pub fn create_add_trusting_clause_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing rule/check/policy
    let mut current = *node;
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => break,
            _ => {
                current = current.parent()?;
            }
        }
    }

    // Check if it already has a trusting clause by looking for origin_clause
    // The origin_clause is a child of rule_body, not a direct child of rule/check/policy
    let has_trusting = {
        let mut has_it = false;
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            if child.kind() == "rule_body" {
                // Look inside rule_body for origin_clause
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    if body_child.kind() == "origin_clause" {
                        has_it = true;
                        break;
                    }
                }
            }
            if has_it {
                break;
            }
        }
        has_it
    };

    if has_trusting {
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

    // Create two variants: trusting authority and trusting previous
    let mut changes_authority = std::collections::HashMap::new();
    changes_authority.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(insert_pos, insert_pos),
            new_text: " trusting authority".to_string(),
        }],
    );

    let mut changes_previous = std::collections::HashMap::new();
    changes_previous.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(insert_pos, insert_pos),
            new_text: " trusting previous".to_string(),
        }],
    );

    // Return both as separate actions
    // For now, return the authority one (we can make this a menu later)
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Add trusting clause (authority)".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes_authority),
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
    let mut current = *node;
    loop {
        match current.kind() {
            "check" => break,
            _ => {
                match current.parent() {
                    Some(parent) => current = parent,
                    None => return Vec::new(),
                }
            }
        }
    }

    // The text of the check should start with "check if", "check all", or "reject if"
    let start_byte = current.start_byte();
    let check_text = rope.byte_slice(start_byte..current.end_byte()).to_string();

    let current_variant = if check_text.starts_with("check if") {
        "check if"
    } else if check_text.starts_with("check all") {
        "check all"
    } else if check_text.starts_with("reject if") {
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
    let mut current = *node;
    loop {
        match current.kind() {
            "policy" => break,
            _ => {
                match current.parent() {
                    Some(parent) => current = parent,
                    None => return Vec::new(),
                }
            }
        }
    }

    // The text of the policy should start with "allow if" or "deny if"
    let start_byte = current.start_byte();
    let policy_text = rope.byte_slice(start_byte..current.end_byte()).to_string();

    let (current_type, target_type) = if policy_text.starts_with("allow if") {
        ("allow if", "deny if")
    } else if policy_text.starts_with("deny if") {
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
    // Check if cursor is on a literal value (string, number, boolean, date, bytes)
    let target_node = match node.kind() {
        "string" | "number" | "boolean" | "date" | "bytes" => *node,
        _ => {
            // Try parent
            let parent = node.parent()?;
            match parent.kind() {
                "string" | "number" | "boolean" | "date" | "bytes" => parent,
                _ => return None,
            }
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

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert '{}' to parameter", value),
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
) -> Option<CodeActionOrCommand> {
    // Find the enclosing check or policy
    let mut current = *node;
    let parent_kind = loop {
        match current.kind() {
            "check" | "policy" => break current.kind(),
            _ => {
                current = current.parent()?;
            }
        }
    };

    // Find the rule_body
    let mut rule_body = None;
    let mut cursor = current.walk();
    for child in current.children(&mut cursor) {
        if child.kind() == "rule_body" {
            rule_body = Some(child);
            break;
        }
    }

    let rule_body_node = rule_body?;
    let body_start = rule_body_node.start_byte();
    let body_end = rule_body_node.end_byte();
    let body_text = rope.byte_slice(body_start..body_end).to_string();

    // Extract all variables from the rule body
    let variables = extract_variables_from_node(&rule_body_node, rope);

    // Generate rule signature
    let rule_name = "extracted_rule";
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

/// Extract all variables from a tree-sitter node
fn extract_variables_from_node(node: &tree_sitter::Node, rope: &Rope) -> HashSet<String> {
    let mut variables = HashSet::new();
    visit_node_for_variables(node, rope, &mut variables);
    variables
}

/// Recursively visit nodes to collect variables
fn visit_node_for_variables(node: &tree_sitter::Node, rope: &Rope, variables: &mut HashSet<String>) {
    if node.kind() == "variable" {
        let start = node.start_byte();
        let end = node.end_byte();
        let var_text = rope.byte_slice(start..end).to_string();
        variables.insert(var_text);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node_for_variables(&child, rope, variables);
    }
}

/// Create code action to inline a rule call
pub fn create_inline_rule_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
    root: tree_sitter::Node,
) -> Option<CodeActionOrCommand> {
    // Find if we're on a predicate
    let mut current = *node;
    let predicate_node = loop {
        if current.kind() == "predicate" {
            break current;
        }
        current = current.parent()?;
    };

    // Get predicate name and arity
    let name_node = predicate_node.child(0)?;
    if name_node.kind() != "nname" {
        return None;
    }

    let pred_name = rope
        .byte_slice(name_node.start_byte()..name_node.end_byte())
        .to_string();

    // Count arguments
    let arity = count_predicate_arguments(&predicate_node, rope);

    // Find the rule definition with matching name and arity
    let rule_def = find_rule_definition(&root, &pred_name, arity, rope)?;

    // Get rule head parameters
    let rule_head = rule_def.child_by_field_name("head")?;
    let rule_params = extract_predicate_parameters(&rule_head, rope);

    // Get call arguments
    let call_args = extract_predicate_parameters(&predicate_node, rope);

    // Get rule body
    let rule_body = rule_def.child_by_field_name("body")?;
    let body_text = rope
        .byte_slice(rule_body.start_byte()..rule_body.end_byte())
        .to_string();

    // Perform variable substitution
    let mut inlined_body = body_text.clone();
    for (param, arg) in rule_params.iter().zip(call_args.iter()) {
        // Replace all occurrences of param with arg in the body
        inlined_body = inlined_body.replace(param, arg);
    }

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

/// Count arguments in a predicate
fn count_predicate_arguments(predicate_node: &tree_sitter::Node, _rope: &Rope) -> usize {
    let mut count = 0;
    let mut cursor = predicate_node.walk();
    for child in predicate_node.children(&mut cursor) {
        if child.kind() == "term" {
            count += 1;
        }
    }
    count
}

/// Find a rule definition by name and arity
fn find_rule_definition<'a>(
    root: &tree_sitter::Node<'a>,
    name: &str,
    arity: usize,
    rope: &Rope,
) -> Option<tree_sitter::Node<'a>> {
    // Store the byte range instead of the node
    let mut result_range: Option<(usize, usize)> = None;
    visit_node_simple(root, &mut |node| {
        if result_range.is_some() {
            return; // Already found
        }
        if node.kind() == "rule" {
            if let Some(head) = node.child_by_field_name("head") {
                if head.kind() == "predicate" {
                    if let Some(name_node) = head.child(0) {
                        if name_node.kind() == "nname" {
                            let rule_name = rope
                                .byte_slice(name_node.start_byte()..name_node.end_byte())
                                .to_string();
                            let rule_arity = count_predicate_arguments(&head, rope);
                            if rule_name == name && rule_arity == arity {
                                result_range = Some((node.start_byte(), node.end_byte()));
                            }
                        }
                    }
                }
            }
        }
    });

    // Reconstruct the node from the byte range
    if let Some((start, end)) = result_range {
        root.descendant_for_byte_range(start, end)
    } else {
        None
    }
}

/// Extract parameters from a predicate (as strings)
fn extract_predicate_parameters(predicate_node: &tree_sitter::Node, rope: &Rope) -> Vec<String> {
    let mut params = Vec::new();
    let mut cursor = predicate_node.walk();
    for child in predicate_node.children(&mut cursor) {
        if child.kind() == "term" {
            let param_text = rope.byte_slice(child.start_byte()..child.end_byte()).to_string();
            params.push(param_text);
        }
    }
    params
}

/// Visit all nodes recursively (simple version)
fn visit_node_simple<F>(node: &tree_sitter::Node, visitor: &mut F)
where
    F: FnMut(&tree_sitter::Node),
{
    visitor(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node_simple(&child, visitor);
    }
}

/// Create code action to sort a rule body
pub fn create_sort_rule_body_action(
    node: &tree_sitter::Node,
    rope: &Rope,
    uri: &Url,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing rule, check, or policy
    let mut current = *node;
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => break,
            _ => {
                current = current.parent()?;
            }
        }
    }

    // Find the first rule_body
    let mut rule_body_node = None;
    let mut cursor = current.walk();
    for child in current.children(&mut cursor) {
        if child.kind() == "rule_body" {
            rule_body_node = Some(child);
            break;
        }
    }

    let body_node = rule_body_node?;

    // Extract all predicates and expressions from the body
    let mut items: Vec<(String, usize, usize)> = Vec::new(); // (text, start, end)
    let mut body_cursor = body_node.walk();
    for child in body_node.children(&mut body_cursor) {
        if child.kind() == "predicate" || child.kind() == "expression" {
            let text = rope.byte_slice(child.start_byte()..child.end_byte()).to_string();
            items.push((text, child.start_byte(), child.end_byte()));
        }
    }

    if items.len() <= 1 {
        return None; // Nothing to sort
    }

    // Sort items: predicates before expressions, then alphabetically
    let mut sorted_items = items.clone();
    sorted_items.sort_by(|a, b| {
        // Determine if item is predicate or expression
        let a_is_pred = !a.0.contains("==") && !a.0.contains("!=") && !a.0.contains('>') && !a.0.contains('<');
        let b_is_pred = !b.0.contains("==") && !b.0.contains("!=") && !b.0.contains('>') && !b.0.contains('<');

        match (a_is_pred, b_is_pred) {
            (true, false) => std::cmp::Ordering::Less,    // predicates before expressions
            (false, true) => std::cmp::Ordering::Greater, // expressions after predicates
            _ => a.0.cmp(&b.0),                           // alphabetical within same type
        }
    });

    // Check if already sorted
    if items.iter().map(|i| &i.0).eq(sorted_items.iter().map(|i| &i.0)) {
        return None; // Already sorted
    }

    // Build the sorted text
    let sorted_text = sorted_items
        .iter()
        .map(|i| i.0.clone())
        .collect::<Vec<_>>()
        .join(", ");

    // Find the range of all items
    let start = items.iter().map(|i| i.1).min()?;
    let end = items.iter().map(|i| i.2).max()?;

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
