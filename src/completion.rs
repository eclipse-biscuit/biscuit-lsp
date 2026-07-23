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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_sitter::DocumentData;

    #[test]
    fn test_no_predicate_completions_when_typing_variable() {
        // When typing a variable like "$u", we should only get variable completions,
        // not predicate completions
        let code = r#"
check if user($u), role($u, $r);
"#;
        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position inside "$u" - between $ and u
        let offset = code.find("$u").unwrap() + 1;

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should not include any predicate completions (FUNCTION kind)
        let has_predicates = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::FUNCTION));
        assert!(
            !has_predicates,
            "Should not suggest predicates when typing a variable"
        );

        // Should include variable completions
        let has_variables = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::VARIABLE));
        assert!(
            has_variables,
            "Should suggest variables when typing a variable"
        );
    }

    #[test]
    fn test_variables_from_entire_scope_suggested() {
        // Variables appearing later in the rule should still be suggested
        let code = r#"
check if user($u), role($u, $r);
"#;
        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position inside "$u" - before $r is defined
        let offset = code.find("$u").unwrap() + 1;

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should include both $u and $r even though $r appears later
        let variable_names: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::VARIABLE))
            .map(|c| c.label.as_str())
            .collect();

        assert!(variable_names.contains(&"u"), "Should suggest 'u' variable");
        assert!(
            variable_names.contains(&"r"),
            "Should suggest 'r' variable even though it appears later"
        );
    }

    #[test]
    fn test_predicates_shown_when_not_typing_variable() {
        // When not typing a variable, predicate completions should be shown
        // but variables should not be shown
        let code = r#"
check if user($u);
fact("test");
"#;
        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position after "check if " - before any predicate name
        let offset = code.find("user").unwrap();

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should include predicate completions
        let has_predicates = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::FUNCTION));
        assert!(
            has_predicates,
            "Should suggest predicates when not typing a variable"
        );

        // Should NOT include variable completions
        let has_variables = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::VARIABLE));
        assert!(
            !has_variables,
            "Should not suggest variables when not typing a variable"
        );
    }

    #[test]
    fn test_only_variables_after_dollar_sign() {
        // After typing just "$", should only see variable completions
        let code = r#"foo(true);
bar($) <- foo($foobar);"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after "$" in "bar($"
        let offset = code.find("bar($").unwrap() + 5;

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should NOT include predicate completions (foo, bar)
        let predicate_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::FUNCTION))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            predicate_labels.is_empty(),
            "Should not suggest predicates after $: {:?}",
            predicate_labels
        );

        // Should include variable completions (foobar)
        let variable_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::VARIABLE))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            variable_labels.contains(&"foobar"),
            "Should suggest foobar variable"
        );
    }

    #[test]
    fn test_only_predicates_when_typing_predicate_name() {
        // When typing a predicate name (not starting with $), don't suggest variables
        let code = r#"foo(true);
bar($) <- foo($foobar), "#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after the comma and space
        let offset = code.len();

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should include predicate completions
        let has_predicates = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::FUNCTION));
        assert!(has_predicates, "Should suggest predicates");

        // Should NOT include variables (foobar shouldn't be suggested here)
        let variable_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::VARIABLE))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            variable_labels.is_empty(),
            "Should not suggest variables when typing predicate name: {:?}",
            variable_labels
        );
    }

    #[test]
    fn test_partial_variable_name_no_predicates() {
        // When typing a partial variable like "$fo", should not suggest predicates
        let code = r#"foo(true);
bar($bar) <- foo($fo);"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position in the middle of "$fo" - after "o"
        let fo_pos = code.find("$fo").unwrap();
        let offset = fo_pos + 3; // After "$fo"

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should NOT suggest predicates when typing a variable
        let predicate_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::FUNCTION))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            predicate_labels.is_empty(),
            "Should not suggest predicates when typing partial variable: {:?}",
            predicate_labels
        );

        // Should suggest variables
        let has_variables = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::VARIABLE));
        assert!(has_variables, "Should suggest variables");
    }

    #[test]
    fn test_just_dollar_sign_trailing() {
        // When typing just "$" at the end of an expression, should only suggest variables
        let code = r#"foo(true);
bar($bar) <- foo($bar), $"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after the trailing "$"
        let offset = code.len();

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should NOT suggest predicates when typing a variable
        let predicate_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::FUNCTION))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            predicate_labels.is_empty(),
            "Should not suggest predicates after $: {:?}",
            predicate_labels
        );

        // Should suggest variables
        let variable_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::VARIABLE))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            variable_labels.contains(&"bar"),
            "Should suggest bar variable: {:?}",
            variable_labels
        );
    }

    #[test]
    fn test_partial_variable_with_semicolon() {
        // When typing "$fo" followed by semicolon, should only suggest variables
        let code = r#"foo(true);
bar($bar) <- foo($bar), $fo;"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after "o" in "$fo"
        let fo_offset = code.rfind("$fo").unwrap() + 3;

        let completions = get_completions(tree, &doc_data.rope, fo_offset);

        // Should NOT suggest predicates when typing a variable
        let predicate_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::FUNCTION))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            predicate_labels.is_empty(),
            "Should not suggest predicates when typing $fo: {:?}",
            predicate_labels
        );

        // Should suggest variables
        let has_variables = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::VARIABLE));
        assert!(has_variables, "Should suggest variables");
    }

    #[test]
    fn test_dollar_in_rule_body() {
        // When typing "$" in rule body, should suggest variables from head
        let code = r#"foo($bar) <- $;"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after "$" in body
        let offset = code.find(" <- $").unwrap() + 5;

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should suggest variables from the rule head
        let variable_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::VARIABLE))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            variable_labels.contains(&"bar"),
            "Should suggest bar variable from rule head: {:?}",
            variable_labels
        );
    }

    #[test]
    fn test_keywords_suggested_with_predicates() {
        // Keywords should be suggested when not typing variables or in method context
        let code = r#"foo(true);
"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position at the end after newline
        let offset = code.len();

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should suggest keywords
        let keyword_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::KEYWORD))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            keyword_labels.contains(&"check if"),
            "Should suggest 'check if' keyword: {:?}",
            keyword_labels
        );
        assert!(
            keyword_labels.contains(&"true"),
            "Should suggest 'true' keyword: {:?}",
            keyword_labels
        );
        assert!(
            keyword_labels.contains(&"false"),
            "Should suggest 'false' keyword: {:?}",
            keyword_labels
        );

        // Should also suggest predicates
        let predicate_labels: Vec<_> = completions
            .iter()
            .filter(|c| c.kind == Some(CompletionItemKind::FUNCTION))
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            predicate_labels.contains(&"foo/1"),
            "Should suggest 'foo/1' predicate: {:?}",
            predicate_labels
        );
    }

    #[test]
    fn test_no_keywords_when_typing_variable() {
        // Keywords should NOT be suggested when typing a variable
        let code = r#"check if user($uid, $name), role($u);"#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after "$" in "$u"
        let offset = code.find("$u)").unwrap() + 1;

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should NOT suggest keywords
        let has_keywords = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::KEYWORD));
        assert!(
            !has_keywords,
            "Should not suggest keywords when typing a variable"
        );

        // Should only suggest variables (uid and name)
        let has_variables = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::VARIABLE));
        assert!(has_variables, "Should suggest variables");
    }

    #[test]
    fn test_no_keywords_in_method_context() {
        // Keywords should NOT be suggested in method context
        let code = r#"check if user("test")."#;

        let doc_data = DocumentData::from_text(code);
        let tree = doc_data.tree.as_ref().unwrap();

        // Position right after "."
        let offset = code.len();

        let completions = get_completions(tree, &doc_data.rope, offset);

        // Should NOT suggest keywords
        let has_keywords = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::KEYWORD));
        assert!(
            !has_keywords,
            "Should not suggest keywords in method context"
        );

        // Should only suggest methods
        let has_methods = completions
            .iter()
            .any(|c| c.kind == Some(CompletionItemKind::METHOD));
        assert!(has_methods, "Should suggest methods");
    }
}
