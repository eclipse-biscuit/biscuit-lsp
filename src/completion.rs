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
    let mut items = Vec::new();

    // Find the node at cursor
    let node = tree_sitter::find_node_at_cursor(tree.root_node(), byte_offset);

    // Check if we're in a method context
    if tree_sitter::is_in_method_context(node, byte_offset, rope) {
        items.extend(get_method_completions());
    }

    items
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
                format!("{}()$0", name)
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
