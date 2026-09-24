use futures::channel::oneshot::{self, Sender};
use async_lsp::{LanguageServer as _, ServerSocket, concurrency::ConcurrencyLayer, lsp_types::{InitializeParams, notification::{LogMessage, Progress, PublishDiagnostics, ShowMessage}}, panic::CatchUnwindLayer, router::Router, tracing::TracingLayer};
use gpui::AsyncApp;
use gpui_tokio::Tokio;
use smol::process::{Child, Command, Stdio};
use anyhow::{Result, anyhow};
use tower::ServiceBuilder;
use std::{ffi::OsStr, ops::ControlFlow, path::Path};
use async_lsp::lsp_types;
use lsp_types::*;

use crate::config;
use crate::logging::ResultLogExt as _;

#[derive(Debug)]
pub struct LspClient {
    _child: Child,
    pub server: ServerSocket,
    pub uri: Url,
    version: usize,
}

struct ClientState {
    tx: Option<Sender<()>>,
}

struct Stop;

impl LspClient {
    pub async fn start(_cmd: impl AsRef<OsStr>, cx: &mut AsyncApp) -> Result<Self> {
        let root_dir = Path::new("/home/ahri/.config/pyonji/")
            .canonicalize()?;

        let (tx, _) = oneshot::channel();

        let (mainloop, mut server) = async_lsp::MainLoop::new_client(|_server| {
            let mut router = Router::new(ClientState {
                tx: Some(tx),
            });
            router
                .notification::<Progress>(|this, prog| {
                    tracing::warn!("{:?} {:?}", prog.token, prog.value);
                    // Sometimes rust-analyzer auto-index multiple times?
                    if let Some(tx) = this.tx.take() {
                        _ = tx.send(());
                    }
                    ControlFlow::Continue(())
                })
                .notification::<PublishDiagnostics>(|_, _params| {
                    //tracing::warn!("diagnostics {}: {:?}", params.uri, params.diagnostics);
                    ControlFlow::Continue(())
                })
                .notification::<ShowMessage>(|_, params| {
                    tracing::warn!("Message {:?}: {}", params.typ, params.message);
                    ControlFlow::Continue(())
                })
                .notification::<LogMessage>(|_, params| {
                    tracing::warn!("Log {:?}: {}", params.typ, params.message);
                    ControlFlow::Continue(())
                })
                .event(|_, _: Stop| ControlFlow::Break(Ok(())));

            ServiceBuilder::new()
                .layer(TracingLayer::default())
                .layer(CatchUnwindLayer::default())
                .layer(ConcurrencyLayer::default())
                .service(router)
        });

        let mut child = Command::new("lua-language-server")
            .current_dir(&root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        tracing::warn!("spawned lua-language-server");
        Tokio::spawn(cx, async move {
            mainloop.run_buffered(stdout, stdin).await.log();
        }).detach();

        let root_uri = Url::from_file_path(&root_dir).map_err(|_| anyhow!("failed to parse url"))?;
        #[allow(deprecated)]
        server.initialize(InitializeParams {
            root_uri: Some(root_uri.clone()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: "root".into(),
            }]),
            capabilities: ClientCapabilities {
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..WindowClientCapabilities::default()
                }),
                ..ClientCapabilities::default()
            },
            ..InitializeParams::default()
        })
        .await?;
        server.initialized(InitializedParams {})?;

        let uri = Url::from_file_path("/home/ahri/.config/pyonji/init.lua").map_err(|_| anyhow!("failed to parse url"))?;
        server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "lua".into(),
                version: 0,
                text: "".into(),
            }
        })?;

        let types_uri = Url::from_file_path(root_dir.join("__pyonji_types__.lua"))
            .map_err(|_| anyhow!("failed to parse url"))?;
        server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: types_uri,
                language_id: "lua".into(),
                version: 0,
                text: config::DEFAULT_CONFIG.into(),
            }
        })?;

        Ok(Self {
            _child: child,
            server,
            uri: uri,
            version: 0,
        })
    }

    pub fn push_changes(&mut self, range: std::ops::Range<usize>, text: &str) -> Result<()> {
        let len = range.end - range.start;
        let range = Range {
            start: Position{ line: 0, character: range.start as u32 },
            end: Position{ line: 0, character: range.end as u32 }
        };
        let version = self.version + 1;
        self.version = version;
        self.server.did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: self.uri.clone(),
                version: version as i32,
            },
            content_changes: vec![
                TextDocumentContentChangeEvent {
                    range: Some(range),
                    range_length: Some(len as u32),
                    text: text.to_string()
                }
            ],
        })?;
        Ok(())
    }

    pub async fn get_completions_for(mut server: ServerSocket, uri: Url, trigger: char, position: u32) -> Result<Option<Vec<CompletionItem>>> {
        let r = server.completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line: 0, character: position }
            },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None
            },
            context: Some(CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(trigger.into())
            })
        }).await?;
        let mut items = r.map(|res| match res {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        });

        if let Some(items) = &mut items {
            for item in items {
                *item = Self::resolve_item(server.clone(), item.clone()).await.unwrap();
            }
        }

        Ok(items)
    }

    pub async fn resolve_item(mut server: ServerSocket, item: CompletionItem) -> Result<CompletionItem> {
        Ok(server.completion_item_resolve(item).await?)
    }
}
