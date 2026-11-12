/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Code actions (refactors) for biscuit LSP.
//!
//! Each action is computed as a [`ProposedAction`] — a title plus a set of
//! byte-range text replacements — which the LSP handler turns into a
//! `CodeAction`. Keeping the byte-range form separate from the LSP types makes
//! the actions testable by simply applying the edits to the source.

use std::collections::HashMap;

use ropey::Rope;
use tower_lsp::lsp_types::*;

use crate::ast;
use crate::tree_sitter::{find_node_at_cursor, offset_to_position, ByteRange, Tree};

/// A refactor the user can apply: a title, a `CodeActionKind`, and the edits.
pub struct ProposedAction {
    pub title: String,
    pub kind: CodeActionKind,
    pub is_preferred: bool,
    /// Byte-range replacements to apply to the current document.
    pub edits: Vec<(ByteRange, String)>,
}

impl ProposedAction {
    /// Convert to an LSP `CodeAction` scoped to `uri`, resolving byte ranges to
    /// positions in `rope`. Returns `None` if a range can't be mapped.
    fn into_code_action(self, uri: &Url, rope: &Rope) -> Option<CodeActionOrCommand> {
        let text_edits = self
            .edits
            .into_iter()
            .map(|((start, end), new_text)| {
                Some(TextEdit {
                    range: Range::new(
                        offset_to_position(start, rope)?,
                        offset_to_position(end, rope)?,
                    ),
                    new_text,
                })
            })
            .collect::<Option<Vec<_>>>()?;

        let mut changes = HashMap::new();
        changes.insert(uri.clone(), text_edits);

        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: self.title,
            kind: Some(self.kind),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            is_preferred: Some(self.is_preferred),
            ..Default::default()
        }))
    }
}

/// All code actions available at `byte_offset`, as LSP `CodeAction`s for `uri`.
pub fn code_actions(
    tree: &Tree,
    rope: &Rope,
    uri: &Url,
    byte_offset: usize,
) -> Vec<CodeActionOrCommand> {
    proposed_actions(tree, rope, byte_offset)
        .into_iter()
        .filter_map(|action| action.into_code_action(uri, rope))
        .collect()
}

/// The refactors available at `byte_offset`, in byte-range form. Public so the
/// test harness can assert on offered titles and on applied edits.
pub fn proposed_actions(tree: &Tree, rope: &Rope, byte_offset: usize) -> Vec<ProposedAction> {
    let node = find_node_at_cursor(tree.root_node(), byte_offset);

    let mut actions = Vec::new();
    actions.extend(add_trusting_clause(&node));
    actions.extend(convert_statement_keyword(&node, rope));
    actions
}

/// 1. Add a `trusting authority` clause to a statement that lacks one.
fn add_trusting_clause(node: &::tree_sitter::Node) -> Option<ProposedAction> {
    let statement = ast::find_enclosing_statement(node)?;
    if ast::has_origin_clause(&statement) {
        return None;
    }

    // Insert right after the last rule_body of the statement.
    let mut cursor = statement.walk();
    let insert_at = statement
        .children(&mut cursor)
        .filter(|child| child.kind() == "rule_body")
        .map(|child| child.end_byte())
        .last()?;

    Some(ProposedAction {
        title: "Add trusting clause (authority)".to_string(),
        kind: CodeActionKind::REFACTOR,
        is_preferred: true,
        edits: vec![((insert_at, insert_at), " trusting authority".to_string())],
    })
}

/// The leading keywords a `check` or `policy` statement can start with. Since
/// checks and policies share the same `<keyword> <rule_body>;` shape, any one
/// can be converted into any other by swapping this keyword — a check can
/// become a policy and vice versa.
const CONVERTIBLE_KEYWORDS: [&str; 5] =
    ["check if", "check all", "reject if", "allow if", "deny if"];

/// 2/3. Convert a `check`/`policy` statement to any other check variant or
///      policy type.
fn convert_statement_keyword(node: &::tree_sitter::Node, rope: &Rope) -> Vec<ProposedAction> {
    let statement = match ast::find_enclosing_statement(node) {
        Some(n) if matches!(n.kind(), "check" | "policy") => n,
        _ => return Vec::new(),
    };
    let Some(current) = leading_keyword(&statement, rope) else {
        return Vec::new();
    };

    let start = statement.start_byte();
    let keyword_range = (start, start + current.len());
    CONVERTIBLE_KEYWORDS
        .iter()
        .filter(|&&keyword| keyword != current)
        .map(|&target| ProposedAction {
            title: format!("Convert to '{target}'"),
            kind: CodeActionKind::REFACTOR,
            is_preferred: false,
            edits: vec![(keyword_range, target.to_string())],
        })
        .collect()
}

/// The [`CONVERTIBLE_KEYWORDS`] keyword the statement begins with, if any.
fn leading_keyword(statement: &::tree_sitter::Node, rope: &Rope) -> Option<&'static str> {
    // The longest keyword is 9 bytes ("reject if"); sample a bit more for slack.
    let start = statement.start_byte();
    let sample_end = (start + 12).min(statement.end_byte());
    let prefix = rope
        .byte_slice(start..sample_end)
        .to_string()
        .to_lowercase();
    CONVERTIBLE_KEYWORDS
        .iter()
        .copied()
        .find(|keyword| prefix.starts_with(keyword))
}
