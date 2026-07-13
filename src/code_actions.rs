/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use ropey::Rope;
use tower_lsp::lsp_types::*;

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
