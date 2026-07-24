/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Test harness for LSP features (completions, actions, contexts)
//!
//! Reads multi-document YAML test files and generates test cases dynamically.

use serde::Deserialize;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

/// A single test case from a YAML document
#[derive(Debug, Deserialize)]
pub struct TestCase {
    pub title: String,
    pub input: String,
    #[serde(default)]
    pub completions: Option<CompletionExpectation>,
    #[serde(default)]
    pub actions: Option<ActionExpectation>,
    #[serde(default)]
    pub contexts: Option<Vec<String>>,
}

/// Expectation for completion items - either exact match or partial (contains/not_contains)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CompletionExpectation {
    Exact(Vec<CompletionMatcher>),
    Partial {
        #[serde(default)]
        contains: Vec<CompletionMatcher>,
        #[serde(default)]
        not_contains: Vec<CompletionMatcher>,
    },
}

/// Expectation for code actions - either exact match or partial (contains/not_contains)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ActionExpectation {
    Exact(Vec<String>),
    Partial {
        #[serde(default)]
        contains: Vec<String>,
        #[serde(default)]
        not_contains: Vec<String>,
    },
}

/// Matcher for completion items - all fields are optional (wildcard matching)
#[derive(Debug, Deserialize)]
pub struct CompletionMatcher {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// Result of running a test case
#[derive(Debug)]
pub enum TestResult {
    Pass,
    Fail(String),
}

impl TestResult {
    /// Convert to libtest_mimic result type
    pub fn into_result(self) -> Result<(), libtest_mimic::Failed> {
        match self {
            TestResult::Pass => Ok(()),
            TestResult::Fail(msg) => Err(msg.into()),
        }
    }
}

/// Parse a multi-document YAML file into test cases
pub fn parse_test_file(content: &str) -> Result<Vec<TestCase>, serde_yaml::Error> {
    serde_yaml::Deserializer::from_str(content)
        .map(TestCase::deserialize)
        .collect()
}

/// Extract cursor position from input string (marked with `|`)
///
/// Returns (clean_code, byte_offset) where clean_code has the `|` removed
pub fn extract_cursor_position(input: &str) -> Result<(String, usize), String> {
    let pipe_count = input.chars().filter(|&c| c == '|').count();

    if pipe_count == 0 {
        return Err("No cursor position marker '|' found in input".to_string());
    }

    if pipe_count > 1 {
        return Err(format!(
            "Multiple cursor position markers found ({})",
            pipe_count
        ));
    }

    // Find byte offset of the pipe character
    let mut byte_offset = 0;
    let mut found = false;

    for (idx, ch) in input.char_indices() {
        if ch == '|' {
            byte_offset = idx;
            found = true;
            break;
        }
    }

    if !found {
        return Err("Cursor position marker '|' not found".to_string());
    }

    // Remove the pipe character
    let clean_code = input.replacen('|', "", 1);

    Ok((clean_code, byte_offset))
}

/// Run a single test case
pub fn run_test_case<F, C>(
    test_case: &TestCase,
    completion_fn: F,
    context_fn: C,
) -> TestResult
where
    F: Fn(&str, usize) -> Vec<CompletionItem>,
    C: Fn(&str, usize) -> Vec<String>,
{
    // Extract cursor position
    let (clean_code, cursor_offset) = match extract_cursor_position(&test_case.input) {
        Ok(result) => result,
        Err(e) => return TestResult::Fail(format!("Failed to extract cursor position: {}", e)),
    };

    // Test completions if specified
    if let Some(ref expectation) = test_case.completions {
        let completions = completion_fn(&clean_code, cursor_offset);
        if let Err(e) = check_completions(expectation, &completions) {
            return TestResult::Fail(format!("Completion check failed: {}", e));
        }
    }

    // Test actions if specified
    if let Some(ref _expectation) = test_case.actions {
        // TODO: Implement when code actions are available
        return TestResult::Fail("Code action testing not yet implemented".to_string());
    }

    // Test contexts if specified
    if let Some(ref expected_contexts) = test_case.contexts {
        let actual_contexts = context_fn(&clean_code, cursor_offset);

        // Compare case-insensitively so YAML can spell variant names however
        // is most readable (both sides are normalized, not just one).
        let mut actual_sorted: Vec<String> =
            actual_contexts.iter().map(|s| s.to_lowercase()).collect();
        actual_sorted.sort();
        let mut expected_sorted: Vec<String> =
            expected_contexts.iter().map(|s| s.to_lowercase()).collect();
        expected_sorted.sort();

        if actual_sorted != expected_sorted {
            return TestResult::Fail(format!(
                "Context check failed: expected {:?}, got {:?}",
                expected_sorted, actual_sorted
            ));
        }
    }

    TestResult::Pass
}

/// Check that actual completions match the expectation
fn check_completions(
    expectation: &CompletionExpectation,
    actual: &[CompletionItem],
) -> Result<(), String> {
    match expectation {
        CompletionExpectation::Exact(expected_items) => {
            check_exact_completions(expected_items, actual)
        }
        CompletionExpectation::Partial {
            contains,
            not_contains,
        } => check_partial_completions(contains, not_contains, actual),
    }
}

/// Check exact match: all and only these items should be present
fn check_exact_completions(
    expected: &[CompletionMatcher],
    actual: &[CompletionItem],
) -> Result<(), String> {
    // Check that all expected items are present
    for matcher in expected {
        if !has_matching_completion(actual, matcher) {
            return Err(format!(
                "Expected completion not found: {}",
                format_matcher(matcher)
            ));
        }
    }

    // Check that no unexpected items are present
    for item in actual {
        if !matches_any_expected(item, expected) {
            return Err(format!(
                "Unexpected completion found: {} (kind: {:?})",
                item.label, item.kind
            ));
        }
    }

    // Check count matches
    if expected.len() != actual.len() {
        return Err(format!(
            "Expected {} completions, found {}",
            expected.len(),
            actual.len()
        ));
    }

    Ok(())
}

/// Check partial match: contains items must be present, not_contains items must not be present
fn check_partial_completions(
    contains: &[CompletionMatcher],
    not_contains: &[CompletionMatcher],
    actual: &[CompletionItem],
) -> Result<(), String> {
    // Check that all "contains" items are present
    for matcher in contains {
        if !has_matching_completion(actual, matcher) {
            return Err(format!(
                "Required completion not found: {}",
                format_matcher(matcher)
            ));
        }
    }

    // Check that all "not_contains" items are absent
    for matcher in not_contains {
        if has_matching_completion(actual, matcher) {
            return Err(format!(
                "Forbidden completion found: {}",
                format_matcher(matcher)
            ));
        }
    }

    Ok(())
}

/// Check if any completion matches the matcher
fn has_matching_completion(completions: &[CompletionItem], matcher: &CompletionMatcher) -> bool {
    completions
        .iter()
        .any(|item| matches_completion(item, matcher))
}

/// Check if a completion item matches any of the expected matchers
fn matches_any_expected(item: &CompletionItem, expected: &[CompletionMatcher]) -> bool {
    expected
        .iter()
        .any(|matcher| matches_completion(item, matcher))
}

/// Check if a completion item matches a matcher (wildcard logic)
///
/// Omitted fields in the matcher are ignored (don't care).
fn matches_completion(item: &CompletionItem, matcher: &CompletionMatcher) -> bool {
    // Check label if specified
    if let Some(ref expected_label) = matcher.label {
        if &item.label != expected_label {
            return false;
        }
    }

    // Check kind if specified
    if let Some(ref expected_kind_str) = matcher.kind {
        let expected_kind = match parse_completion_kind(expected_kind_str) {
            Some(kind) => kind,
            None => return false, // Unknown kind string doesn't match
        };

        match item.kind {
            Some(actual_kind) if actual_kind == expected_kind => {}
            _ => return false,
        }
    }

    true
}

/// Parse a completion kind string to CompletionItemKind enum
fn parse_completion_kind(s: &str) -> Option<CompletionItemKind> {
    match s {
        "VARIABLE" => Some(CompletionItemKind::VARIABLE),
        "FUNCTION" => Some(CompletionItemKind::FUNCTION),
        "KEYWORD" => Some(CompletionItemKind::KEYWORD),
        "METHOD" => Some(CompletionItemKind::METHOD),
        "CONSTANT" => Some(CompletionItemKind::CONSTANT),
        "CLASS" => Some(CompletionItemKind::CLASS),
        "MODULE" => Some(CompletionItemKind::MODULE),
        "PROPERTY" => Some(CompletionItemKind::PROPERTY),
        _ => None,
    }
}

/// Format a matcher for error messages
fn format_matcher(matcher: &CompletionMatcher) -> String {
    let mut parts = Vec::new();

    if let Some(ref label) = matcher.label {
        parts.push(format!("label={}", label));
    }

    if let Some(ref kind) = matcher.kind {
        parts.push(format!("kind={}", kind));
    }

    if parts.is_empty() {
        "(any)".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cursor_position_simple() {
        let input = "check if user($u|)";
        let (clean, offset) = extract_cursor_position(input).unwrap();
        assert_eq!(clean, "check if user($u)");
        assert_eq!(offset, 16); // byte offset of |
    }

    #[test]
    fn test_extract_cursor_position_at_start() {
        let input = "|check if";
        let (clean, offset) = extract_cursor_position(input).unwrap();
        assert_eq!(clean, "check if");
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_extract_cursor_position_at_end() {
        let input = "check if|";
        let (clean, offset) = extract_cursor_position(input).unwrap();
        assert_eq!(clean, "check if");
        assert_eq!(offset, 8);
    }

    #[test]
    fn test_extract_cursor_position_no_marker() {
        let input = "check if user";
        let result = extract_cursor_position(input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No cursor position marker"));
    }

    #[test]
    fn test_extract_cursor_position_multiple_markers() {
        let input = "check| if |user";
        let result = extract_cursor_position(input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Multiple cursor position markers"));
    }

    #[test]
    fn test_parse_completion_kind() {
        assert_eq!(
            parse_completion_kind("VARIABLE"),
            Some(CompletionItemKind::VARIABLE)
        );
        assert_eq!(
            parse_completion_kind("FUNCTION"),
            Some(CompletionItemKind::FUNCTION)
        );
        assert_eq!(
            parse_completion_kind("KEYWORD"),
            Some(CompletionItemKind::KEYWORD)
        );
        assert_eq!(
            parse_completion_kind("METHOD"),
            Some(CompletionItemKind::METHOD)
        );
        assert_eq!(parse_completion_kind("UNKNOWN"), None);
    }
}
