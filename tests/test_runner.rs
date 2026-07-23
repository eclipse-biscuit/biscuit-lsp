/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! Test runner using libtest-mimic for dynamic test case generation

use biscuit_language_server::testing::*;
use biscuit_language_server::{completion, tree_sitter::DocumentData};
use libtest_mimic::{Arguments, Trial};
use std::fs;
use std::path::PathBuf;
use tower_lsp::lsp_types::CompletionItem;

/// Get completions for a test case
fn get_completions_for_test(code: &str, cursor_offset: usize) -> Vec<CompletionItem> {
    let doc_data = DocumentData::from_text(code);
    let tree = doc_data.tree.as_ref().expect("Failed to parse tree");
    completion::get_completions(tree, &doc_data.rope, cursor_offset)
}

fn main() {
    let args = Arguments::from_args();

    // Discover all .yaml test files
    let test_files = discover_test_files("tests/completion");

    // Generate a Trial for each YAML document
    let mut tests = Vec::new();

    for file_path in test_files {
        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read test file {:?}: {}", file_path, e);
                continue;
            }
        };

        let test_cases = match parse_test_file(&content) {
            Ok(cases) => cases,
            Err(e) => {
                eprintln!("Failed to parse test file {:?}: {}", file_path, e);
                continue;
            }
        };

        for test_case in test_cases {
            let file_name = file_path.file_stem().unwrap().to_string_lossy().to_string();
            let test_name = format!("{}::{}", file_name, test_case.title.replace(" ", "_"));

            let trial = Trial::test(test_name, move || {
                run_test_case(&test_case, get_completions_for_test, None).into_result()
            });
            tests.push(trial);
        }
    }

    // Run all tests
    libtest_mimic::run(&args, tests).exit();
}

/// Discover all .yaml test files in a directory
fn discover_test_files(dir: &str) -> Vec<PathBuf> {
    let dir_path = PathBuf::from(dir);

    if !dir_path.exists() {
        eprintln!("Warning: Test directory {:?} does not exist", dir);
        return Vec::new();
    }

    match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |ext| ext == "yaml"))
            .collect(),
        Err(e) => {
            eprintln!("Failed to read test directory {:?}: {}", dir, e);
            Vec::new()
        }
    }
}
