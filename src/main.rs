/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */
use nom::Offset;

use biscuit_auth::parser::parse_source;
use dashmap::DashMap;
use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tree_sitter::{Parser, Tree};

#[derive(Debug)]
struct DocumentData {
    rope: Rope,
    tree: Option<Tree>,
}

#[derive(Debug)]
struct Backend {
    client: Client,
    document_map: DashMap<String, DocumentData>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            offset_encoding: None,
            capabilities: ServerCapabilities {
                inlay_hint_provider: None,
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
                    ..Default::default()
                }),
                execute_command_provider: None,

                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                semantic_tokens_provider: None,
                definition_provider: None,
                references_provider: None,
                rename_provider: None,
                ..ServerCapabilities::default()
            },
        })
    }
    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "initialized!")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;

        let doc_data = match self.document_map.get(&uri) {
            Some(data) => data,
            None => return Ok(None),
        };

        let tree = match &doc_data.tree {
            Some(tree) => tree,
            None => return Ok(None),
        };

        // Convert position to byte offset
        let byte_offset = position_to_offset(&position, &doc_data.rope).unwrap_or(0);

        // Find the node at cursor and extract data before dropping tree reference
        let (node_kind, in_method_context) = {
            let node = find_node_at_cursor(tree.root_node(), byte_offset);
            let node_kind = node.kind().to_string();
            let in_method_context = is_in_method_context(node, byte_offset, &doc_data.rope);
            (node_kind, in_method_context)
        };

        // Get completions from multiple sources
        let mut items = Vec::new();

        // 1. Symbol-based completions (fact and rule names)
        let symbol_items = get_symbol_completions(tree, &doc_data.rope);
        items.extend(symbol_items);

        // 2. Variable completions (scoped to current context)
        let variable_items = get_variable_completions(tree, &doc_data.rope, byte_offset);
        items.extend(variable_items);

        // 3. Method completions (if we're in a method call context)
        if in_method_context {
            items.extend(get_method_completions());
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!("Completion at byte {} in node kind: {:?}, found {} items", byte_offset, node_kind, items.len()),
            )
            .await;

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "file opened!")
            .await;
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.text_document.text,
            version: params.text_document.version,
        })
        .await
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();

        // Get or create document data
        let mut doc_data = self.document_map.entry(uri.clone()).or_insert_with(|| {
            DocumentData {
                rope: Rope::new(),
                tree: None,
            }
        });

        // Apply each change incrementally
        for change in params.content_changes {
            if let Some(range) = change.range {
                // Incremental change
                let rope = &doc_data.rope;

                // Convert LSP positions to byte offsets
                let start_byte = position_to_offset(&range.start, rope).unwrap_or(0);
                let end_byte = position_to_offset(&range.end, rope).unwrap_or(start_byte);

                // Calculate positions for tree-sitter InputEdit
                let start_position = tree_sitter::Point::new(
                    range.start.line as usize,
                    range.start.character as usize,
                );
                let old_end_position = tree_sitter::Point::new(
                    range.end.line as usize,
                    range.end.character as usize,
                );

                // Apply edit to rope
                doc_data.rope.remove(start_byte..end_byte);
                doc_data.rope.insert(start_byte, &change.text);

                // Calculate new end position after insertion
                let new_text_len = change.text.len();
                let new_end_byte = start_byte + new_text_len;
                let new_end_position = offset_to_point(new_end_byte, &doc_data.rope);

                // Apply edit to tree if it exists
                if let Some(ref mut tree) = doc_data.tree {
                    let edit = tree_sitter::InputEdit {
                        start_byte,
                        old_end_byte: end_byte,
                        new_end_byte,
                        start_position,
                        old_end_position,
                        new_end_position,
                    };
                    tree.edit(&edit);
                }
            } else {
                // Full document sync (fallback)
                doc_data.rope = Rope::from_str(&change.text);
                doc_data.tree = None;
            }
        }

        // Reparse with tree-sitter incrementally using chunked parsing
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_biscuit::language()).expect("Error loading biscuit grammar");
        doc_data.tree = parse_rope(&mut parser, &doc_data.rope, doc_data.tree.as_ref());

        drop(doc_data);

        // Run diagnostics
        self.run_diagnostics(&params.text_document.uri, params.text_document.version).await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "file saved!")
            .await;
    }
    async fn did_close(&self, _: DidCloseTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "file closed!")
            .await;
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        self.client
            .log_message(MessageType::INFO, "configuration changed!")
            .await;
    }

    async fn did_change_workspace_folders(&self, _: DidChangeWorkspaceFoldersParams) {
        self.client
            .log_message(MessageType::INFO, "workspace folders changed!")
            .await;
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        self.client
            .log_message(MessageType::INFO, "watched files have changed!")
            .await;
    }
}

struct TextDocumentItem {
    uri: Url,
    text: String,
    version: i32,
}
impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let rope = ropey::Rope::from_str(&params.text);

        // Parse with tree-sitter
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_biscuit::language()).expect("Error loading biscuit grammar");
        let tree = parser.parse(&params.text, None);

        self.document_map.insert(
            params.uri.to_string(),
            DocumentData {
                rope: rope.clone(),
                tree,
            },
        );

        self.run_diagnostics(&params.uri, params.version).await;
    }

    async fn run_diagnostics(&self, uri: &Url, version: i32) {
        let doc_data = match self.document_map.get(&uri.to_string()) {
            Some(data) => data,
            None => return,
        };

        let text = doc_data.rope.to_string();
        let rope = &doc_data.rope;
        let mut diagnostics = Vec::new();

        // 1. Tree-sitter syntax errors (from ERROR nodes)
        if let Some(ref tree) = doc_data.tree {
            let ts_errors = get_tree_sitter_errors(tree, rope);
            diagnostics.extend(ts_errors);
        }

        // 2. Biscuit-auth semantic errors (only if no syntax errors)
        // This avoids cascading errors from broken syntax
        if diagnostics.is_empty() {
            let errors = match parse_source(&text) {
                Ok(_) => vec![],
                Err(e) => e,
            };

            let semantic_diagnostics = errors
                .into_iter()
                .filter_map(|item| {
                    let message = item.message.unwrap_or_else(|| "parse error".to_string());
                    let range = {
                        let input = item.input.trim();
                        // the parser sometimes returns an error with an empty message and empty input
                        if input.is_empty() {
                            return None;
                        }
                        let start = text.offset(input);
                        let end = start + input.len();
                        Range::new(
                            offset_to_position(start, rope).unwrap(),
                            offset_to_position(end, rope).unwrap(),
                        )
                    };
                    Some(Diagnostic::new(
                        range,
                        Some(DiagnosticSeverity::ERROR),
                        None,
                        None,
                        message,
                        None,
                        None,
                    ))
                })
                .collect::<Vec<_>>();

            diagnostics.extend(semantic_diagnostics);
        }

        self.client
            .publish_diagnostics(uri.clone(), diagnostics, Some(version))
            .await;
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        document_map: DashMap::new(),
    })
    .finish();

    serde_json::json!({"test": 20});
    Server::new(stdin, stdout, socket).serve(service).await;
}

fn offset_to_position(offset: usize, rope: &Rope) -> Option<Position> {
    let line = rope.try_char_to_line(offset).ok()?;
    let first_char_of_line = rope.try_line_to_char(line).ok()?;
    let column = offset - first_char_of_line;
    Some(Position::new(line as u32, column as u32))
}

fn position_to_offset(position: &Position, rope: &Rope) -> Option<usize> {
    let line = position.line as usize;
    let character = position.character as usize;
    let line_start = rope.try_line_to_char(line).ok()?;
    Some(line_start + character)
}

fn offset_to_point(offset: usize, rope: &Rope) -> tree_sitter::Point {
    let line = rope.try_char_to_line(offset).unwrap_or(0);
    let line_start = rope.try_line_to_char(line).unwrap_or(0);
    let column = offset.saturating_sub(line_start);
    tree_sitter::Point::new(line, column)
}

/// Parse a Rope with tree-sitter using chunked callbacks to avoid allocating the full text
fn parse_rope(parser: &mut Parser, rope: &Rope, old_tree: Option<&Tree>) -> Option<Tree> {
    parser.parse_with(
        &mut |byte_offset, _point| {
            // Return a chunk of bytes starting from byte_offset
            // tree-sitter will call this callback multiple times to get the text in chunks
            if byte_offset >= rope.len_bytes() {
                return "";
            }
            let slice = rope.byte_slice(byte_offset..);
            // Get the first chunk from the rope slice
            // This avoids allocating a full string - we return a reference to ropey's internal buffer
            slice.chunks().next().unwrap_or("")
        },
        old_tree,
    )
}

/// Find the most specific node at the cursor position
/// Falls back to parent if cursor is in ERROR or MISSING node
fn find_node_at_cursor(root: tree_sitter::Node, byte_offset: usize) -> tree_sitter::Node {
    let mut node = root.descendant_for_byte_range(byte_offset, byte_offset)
        .unwrap_or(root);

    // If we're in an ERROR or MISSING node, try to use the parent for better context
    while (node.is_error() || node.is_missing()) && node.parent().is_some() {
        node = node.parent().unwrap();
    }

    node
}

/// Extract syntax errors from tree-sitter ERROR nodes
fn get_tree_sitter_errors(tree: &Tree, rope: &Rope) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let root = tree.root_node();

    // Walk the tree to find ERROR and MISSING nodes
    visit_node(&root, &mut |node| {
        if node.is_error() || node.is_missing() {
            let start = node.start_byte();
            let end = node.end_byte();

            // Get a meaningful error message
            let message = if node.is_missing() {
                format!("Missing: {}", node.kind())
            } else {
                // For ERROR nodes, try to show what was expected vs what was found
                let text_slice = if end > start && (end - start) < 50 {
                    rope.byte_slice(start..end).to_string()
                } else {
                    "<unknown>".to_string()
                };
                format!("Syntax error near: {}", text_slice)
            };

            if let (Some(start_pos), Some(end_pos)) = (
                offset_to_position(start, rope),
                offset_to_position(end.max(start + 1), rope),
            ) {
                diagnostics.push(Diagnostic::new(
                    Range::new(start_pos, end_pos),
                    Some(DiagnosticSeverity::ERROR),
                    None,
                    None,
                    message,
                    None,
                    None,
                ));
            }
        }
    });

    diagnostics
}

/// Extract fact and rule names from the tree for completion
fn get_symbol_completions(tree: &Tree, rope: &Rope) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let root = tree.root_node();

    // Walk the tree to find all facts and rules
    visit_node(&root, &mut |node| {
        match node.kind() {
            "fact" => {
                // Facts have structure: nname "(" fact_term, ... ")"
                if let Some(name_node) = node.child_by_field_name("name").or_else(|| node.child(0)) {
                    if name_node.kind() == "nname" {
                        let start = name_node.start_byte();
                        let end = name_node.end_byte();
                        let name = rope.byte_slice(start..end).to_string();

                        // Count the number of arguments
                        let arity = count_fact_arguments(node);
                        let snippet = create_predicate_snippet(&name, arity);

                        items.push(CompletionItem {
                            label: format!("{}/{}", name, arity),
                            kind: Some(CompletionItemKind::FUNCTION),
                            detail: Some(format!("Fact: {}", name)),
                            insert_text: Some(snippet),
                            insert_text_format: Some(InsertTextFormat::SNIPPET),
                            filter_text: Some(name.clone()),
                            sort_text: Some(name.clone()),
                            ..Default::default()
                        });
                    }
                }
            }
            "rule" => {
                // Rules have structure: head "<-" body
                if let Some(head) = node.child_by_field_name("head") {
                    if head.kind() == "predicate" {
                        // Predicates have structure: nname "(" term, ... ")"
                        if let Some(name_node) = head.child(0) {
                            if name_node.kind() == "nname" {
                                let start = name_node.start_byte();
                                let end = name_node.end_byte();
                                let name = rope.byte_slice(start..end).to_string();

                                // Count the number of arguments
                                let arity = count_predicate_arguments(&head);
                                let snippet = create_predicate_snippet(&name, arity);

                                items.push(CompletionItem {
                                    label: format!("{}/{}", name, arity),
                                    kind: Some(CompletionItemKind::FUNCTION),
                                    detail: Some(format!("Rule: {}", name)),
                                    insert_text: Some(snippet),
                                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                                    filter_text: Some(name.clone()),
                                    sort_text: Some(name.clone()),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    });

    items
}

/// Count arguments in a fact node
fn count_fact_arguments(fact_node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    let mut cursor = fact_node.walk();
    for child in fact_node.children(&mut cursor) {
        if child.kind() == "fact_term" {
            count += 1;
        }
    }
    count
}

/// Count arguments in a predicate node
fn count_predicate_arguments(predicate_node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    let mut cursor = predicate_node.walk();
    for child in predicate_node.children(&mut cursor) {
        if child.kind() == "term" {
            count += 1;
        }
    }
    count
}

/// Create a snippet for a predicate with placeholders for each argument
fn create_predicate_snippet(name: &str, arity: usize) -> String {
    if arity == 0 {
        format!("{}()$0", name)
    } else {
        let placeholders: Vec<String> = (1..=arity)
            .map(|i| format!("${}", i))
            .collect();
        format!("{}({})$0", name, placeholders.join(", "))
    }
}

/// Visit all nodes in the tree recursively
fn visit_node<F>(node: &tree_sitter::Node, visitor: &mut F)
where
    F: FnMut(&tree_sitter::Node),
{
    visitor(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(&child, visitor);
    }
}

/// Extract variables from the tree for completion, scoped to the current context
/// Returns unique variable names found in scope at the given byte offset
fn get_variable_completions(tree: &Tree, rope: &Rope, byte_offset: usize) -> Vec<CompletionItem> {
    let mut variables = std::collections::HashSet::new();
    let root = tree.root_node();

    // Find the enclosing rule/check/policy for the cursor position
    let context_node = find_enclosing_context(root, byte_offset);

    if let Some(context) = context_node {
        // Only collect variables from the current scope
        visit_node(&context, &mut |node| {
            if node.kind() == "variable" {
                let start = node.start_byte();
                let end = node.end_byte();
                // Exclude the variable currently being typed (cursor is within its range)
                // and only include variables that end before the cursor
                if end < byte_offset {
                    let var_text = rope.byte_slice(start..end).to_string();
                    variables.insert(var_text);
                }
            }
        });
    } else {
        // Fallback: collect all variables in the document
        visit_node(&root, &mut |node| {
            if node.kind() == "variable" {
                let start = node.start_byte();
                let end = node.end_byte();
                // Exclude the variable currently being typed
                if end < byte_offset {
                    let var_text = rope.byte_slice(start..end).to_string();
                    variables.insert(var_text);
                }
            }
        });
    }

    // Convert to completion items
    variables
        .into_iter()
        .map(|var| {
            // Strip the $ prefix from insert_text to avoid duplication
            let insert_text = if let Some(stripped) = var.strip_prefix('$') {
                stripped.to_string()
            } else {
                var.clone()
            };

            CompletionItem {
                label: var.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some("Variable".to_string()),
                insert_text: Some(insert_text),
                ..Default::default()
            }
        })
        .collect()
}

/// Find the enclosing rule/check/policy node for scope-aware completion
fn find_enclosing_context(root: tree_sitter::Node, byte_offset: usize) -> Option<tree_sitter::Node> {
    let mut current = root.descendant_for_byte_range(byte_offset, byte_offset)?;

    // Walk up the tree to find a rule, check, or policy node
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => return Some(current),
            _ => {
                current = current.parent()?;
            }
        }
    }
}

/// Check if we're in a context where method completion makes sense
fn is_in_method_context(node: tree_sitter::Node, byte_offset: usize, rope: &Rope) -> bool {
    // Check if we're inside a methods node
    let mut current = node;
    loop {
        if current.kind() == "methods" {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // Check if the previous character is a dot
    if byte_offset > 0 {
        let prev_char = rope.byte_slice((byte_offset - 1)..byte_offset);
        if prev_char == "." {
            return true;
        }
    }

    false
}

/// Get hardcoded method completions for biscuit built-in methods
/// Source: biscuit-rust/biscuit-parser/src/parser.rs
fn get_method_completions() -> Vec<CompletionItem> {
    vec![
        // Binary methods (take an argument) - cursor between parens
        CompletionItem {
            label: "contains".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if string/array/set contains element".to_string()),
            insert_text: Some("contains($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "starts_with".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if string starts with prefix".to_string()),
            insert_text: Some("starts_with($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "ends_with".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if string ends with suffix".to_string()),
            insert_text: Some("ends_with($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "matches".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if string matches regex pattern".to_string()),
            insert_text: Some("matches($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "intersection".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Get intersection of two sets".to_string()),
            insert_text: Some("intersection($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "union".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Get union of two sets".to_string()),
            insert_text: Some("union($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "all".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if all elements satisfy a condition (closure)".to_string()),
            insert_text: Some("all($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "any".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Check if any element satisfies a condition (closure)".to_string()),
            insert_text: Some("any($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "get".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Get element from array/map".to_string()),
            insert_text: Some("get($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "try_or".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Try operation or return fallback value".to_string()),
            insert_text: Some("try_or($0)".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        // Unary methods (no argument) - cursor after parens
        CompletionItem {
            label: "length".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Get length of string/array/set".to_string()),
            insert_text: Some("length()$0".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        CompletionItem {
            label: "type".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Get type of value".to_string()),
            insert_text: Some("type()$0".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
        // External function call - cursor after :: to type function name
        CompletionItem {
            label: "extern::".to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some("Call external function (FFI)".to_string()),
            insert_text: Some("extern::$0()".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        },
    ]
}
