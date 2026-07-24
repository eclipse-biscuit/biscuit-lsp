/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Completion providers for biscuit LSP

use ropey::Rope;
use tower_lsp::lsp_types::*;

use crate::ast::{detect_valid_insertions, Insertion, KEYWORDS, METHODS};
use crate::tree_sitter;

pub fn get_completions(
    tree: &tree_sitter::Tree,
    rope: &Rope,
    byte_offset: usize,
) -> Vec<CompletionItem> {
    let valid_insertions = detect_valid_insertions(tree, rope, byte_offset);

    let mut completions = Vec::new();

    if valid_insertions.contains(&Insertion::MethodName) {
        completions.extend(get_method_completions());
    }

    if valid_insertions.contains(&Insertion::PredicateName) {
        completions.extend(get_symbol_completions(tree, rope));
    }

    if valid_insertions.contains(&Insertion::Variable) {
        completions.extend(get_variable_completions(tree, rope, byte_offset));
    }

    completions.extend(get_keyword_completions(&valid_insertions));
    completions
}

fn generate_placeholders(arity: usize) -> String {
    (0..arity)
        .map(|i| format!("${}", i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Extract predicate names from the tree for completion
fn get_symbol_completions(tree: &tree_sitter::Tree, rope: &Rope) -> Vec<CompletionItem> {
    let symbols = tree_sitter::get_symbols(tree, rope);

    symbols
        .into_iter()
        .map(|(name, arity)| CompletionItem {
            label: format!("{}/{}", name, arity),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(name.to_string()),
            insert_text: Some(format!("{}({})", name, generate_placeholders(arity))),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            filter_text: Some(name.to_string()),
            sort_text: Some(name),
            ..Default::default()
        })
        .collect()
}

/// Get variable completions from the current scope
fn get_variable_completions(
    tree: &tree_sitter::Tree,
    rope: &Rope,
    byte_offset: usize,
) -> Vec<CompletionItem> {
    let root = tree.root_node();

    let context_node = tree_sitter::find_enclosing_context(root, byte_offset);

    let variables = if let Some(context) = context_node {
        tree_sitter::get_variables_in_scope(context, rope, byte_offset)
    } else {
        vec![]
    };

    variables
        .into_iter()
        .map(|var| CompletionItem {
            label: var.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some("Variable".to_string()),
            ..Default::default()
        })
        .collect()
}

fn get_method_completions() -> Vec<CompletionItem> {
    METHODS
        .iter()
        .map(|(name, description, has_argument)| {
            let insert_text = if *has_argument {
                format!("{}($0)", name)
            } else {
                format!("{}()", name)
            };

            CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(description.to_string()),
                insert_text: Some(insert_text),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            }
        })
        .chain([CompletionItem {
            label: "extern function".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("extern function".to_string()),
            insert_text: Some("extern::$0()".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }])
        .collect()
}

/// Get keyword completions valid in the given insertion contexts
fn get_keyword_completions(valid_insertions: &[Insertion]) -> Vec<CompletionItem> {
    KEYWORDS
        .iter()
        .filter(|(_, _, _, contexts)| contexts.iter().any(|c| valid_insertions.contains(c)))
        .map(|(keyword, description, has_snippet, _)| {
            let insert_text = if *has_snippet {
                Some(format!("{} $0", keyword))
            } else {
                None
            };

            CompletionItem {
                label: keyword.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some(description.to_string()),
                insert_text,
                insert_text_format: if *has_snippet {
                    Some(InsertTextFormat::SNIPPET)
                } else {
                    None
                },
                ..Default::default()
            }
        })
        .collect()
}
