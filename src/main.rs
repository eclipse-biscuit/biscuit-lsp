/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */
mod completion;
mod tree_sitter;

use nom::Offset;

use biscuit_auth::parser::parse_source;
use dashmap::DashMap;
use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use tree_sitter::DocumentData;

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
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string()]),
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
        let byte_offset = tree_sitter::position_to_offset(&position, &doc_data.rope).unwrap_or(0);

        // Get completions
        let items = completion::get_completions(tree, &doc_data.rope, byte_offset);

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "file opened!")
            .await;
        self.on_change(
            params.text_document.uri,
            params.text_document.text,
            params.text_document.version,
        )
        .await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        self.on_change(
            params.text_document.uri,
            std::mem::take(&mut params.content_changes[0].text),
            params.text_document.version,
        )
        .await;
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

impl Backend {
    /// Handle a new document or full document replacement
    async fn on_change(&self, uri: Url, text: String, version: i32) {
        // Create document data from full text
        let doc_data = DocumentData::from_text(&text);
        self.document_map.insert(uri.to_string(), doc_data);

        // Run diagnostics
        self.run_diagnostics(&uri, version).await;
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
        diagnostics.extend(doc_data.get_syntax_errors());

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
