/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Completion providers for biscuit LSP

use ropey::Rope;
use tower_lsp::lsp_types::*;

use crate::tree_sitter;

/// Get completions for the given position
pub fn get_completions(
    tree: &tree_sitter::Tree,
    rope: &Rope,
    byte_offset: usize,
) -> Vec<CompletionItem> {
    let node = tree_sitter::find_node_at_cursor(tree.root_node(), byte_offset);

    if tree_sitter::is_typing_variable(node, byte_offset, rope) {
        get_variable_completions(tree, rope, byte_offset)
    } else if tree_sitter::is_in_method_context(node, byte_offset, rope) {
        get_method_completions()
    } else {
        // Combine keywords and symbol (predicate) completions
        let mut completions = get_keyword_completions();
        completions.extend(get_symbol_completions(tree, rope));
        completions
    }
}

/// Generate placeholders given the provided arity
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

    // Find the enclosing rule/check/policy for the cursor position
    let context_node = tree_sitter::find_enclosing_context(root, byte_offset);

    let variables = if let Some(context) = context_node {
        // Only collect variables from the current scope
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

/// Get hardcoded method completions for biscuit built-in methods
/// Source: biscuit-rust/biscuit-parser/src/parser.rs
fn get_method_completions() -> Vec<CompletionItem> {
    // (name, description, has_argument)
    const METHODS: &[(&str, &str, bool)] = &[
        // Unary methods
        ("length", "Get length of string/array/set", false),
        ("type", "Get the type of the value", false),
        // Binary methods
        (
            "contains",
            "Check if string/array/set contains element",
            true,
        ),
        ("starts_with", "Check if string starts with prefix", true),
        ("ends_with", "Check if string ends with suffix", true),
        ("matches", "Check if string matches regex pattern", true),
        ("intersection", "Get intersection of two sets", true),
        ("union", "Get union of two sets", true),
        (
            "all",
            "Check if all elements satisfy a condition (closure)",
            true,
        ),
        (
            "any",
            "Check if any element satisfies a condition (closure)",
            true,
        ),
        ("get", "Get element from array/map", true),
        ("try_or", "Try operation or return fallback value", true),
    ];

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

/// Get keyword completions for biscuit language constructs
fn get_keyword_completions() -> Vec<CompletionItem> {
    tree_sitter::KEYWORDS
        .iter()
        .map(|(keyword, description, has_snippet)| {
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

// All completion tests have been moved to YAML test files in tests/completion/
// Run with: cargo test --test completion_tests
