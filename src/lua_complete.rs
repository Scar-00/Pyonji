use anyhow::{Result, anyhow};
use async_lsp::lsp_types;
use async_lsp::{
    LanguageServer as _, ServerSocket,
    concurrency::ConcurrencyLayer,
    lsp_types::{
        InitializeParams,
        notification::{LogMessage, Progress, PublishDiagnostics, ShowMessage},
    },
    panic::CatchUnwindLayer,
    router::Router,
    tracing::TracingLayer,
};
use futures::channel::oneshot::{self, Sender};
use gpui::{AsyncApp, Task};
use gpui_tokio::{JoinError, Tokio};
use lsp_types::*;
use smol::process::{Child, Command, Stdio};
use std::{
    ffi::OsStr,
    ops::ControlFlow,
    path::{Path, PathBuf},
};
use tower::ServiceBuilder;

use crate::config;
use crate::logging::ResultLogExt as _;
#[cfg(windows)]
use smol::process::windows::CommandExt;

#[derive(Debug)]
pub struct LspClient {
    _child: Child,
    pub server: ServerSocket,
    pub uri: Url,
    pub resolve_completions: bool,
    version: usize,
    _task: Task<Result<(), JoinError>>,
}

struct ClientState {
    _tx: Option<Sender<()>>,
}

struct Stop;

pub fn default_command() -> PathBuf {
    if let Ok(path) = std::env::var("LUA_LANGUAGE_SERVER") {
        return PathBuf::from(path);
    }
    if let Some(home) = dirs::home_dir() {
        let candidates = [
            home.join(".local/share/nvim/mason/bin/lua-language-server"),
            home.join(".local/bin/lua-language-server"),
            home.join("bin/lua-language-server"),
        ];
        if let Some(path) = candidates.into_iter().find(|path| path.exists()) {
            return path;
        }
    }
    PathBuf::from("lua-language-server")
}

impl LspClient {
    pub async fn start(cmd: impl AsRef<OsStr>, cx: &mut AsyncApp) -> Result<Self> {
        let root_dir = config::util::config_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.canonicalize())
            .transpose()?
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| anyhow!("failed to resolve Pyonji config directory"))?;

        let (tx, _) = oneshot::channel();

        let (mainloop, mut server) = async_lsp::MainLoop::new_client(|_server| {
            let mut router = Router::new(ClientState { _tx: Some(tx) });
            router
                .notification::<Progress>(|_, _| {
                    //tracing::warn!("{:?} {:?}", prog.token, prog.value);
                    // Sometimes rust-analyzer auto-index multiple times?
                    //if let Some(tx) = this.tx.take() {
                    //    _ = tx.send(());
                    //}
                    ControlFlow::Continue(())
                })
                .notification::<PublishDiagnostics>(|_, _| {
                    //tracing::warn!("diagnostics {}: {:?}", params.uri, params.diagnostics);
                    ControlFlow::Continue(())
                })
                .notification::<ShowMessage>(|_, _| {
                    //tracing::warn!("Message {:?}: {}", params.typ, params.message);
                    ControlFlow::Continue(())
                })
                .notification::<LogMessage>(|_, _| {
                    //tracing::warn!("Log {:?}: {}", params.typ, params.message);
                    ControlFlow::Continue(())
                })
                .event(|_, _: Stop| ControlFlow::Break(Ok(())));

            ServiceBuilder::new()
                .layer(TracingLayer::default())
                .layer(CatchUnwindLayer::default())
                .layer(ConcurrencyLayer::default())
                .service(router)
        });

        let mut cmd = Command::new(cmd);
        cmd.current_dir(&root_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        cmd.kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let task = Tokio::spawn(cx, async move {
            mainloop.run_buffered(stdout, stdin).await.log();
        });

        let root_uri =
            Url::from_file_path(&root_dir).map_err(|_| anyhow!("failed to parse url"))?;
        #[allow(deprecated)]
        let initialized = server
            .initialize(InitializeParams {
                root_uri: Some(root_uri.clone()),
                workspace_folders: Some(vec![WorkspaceFolder {
                    uri: root_uri,
                    name: "root".into(),
                }]),
                capabilities: ClientCapabilities {
                    text_document: Some(TextDocumentClientCapabilities {
                        completion: Some(CompletionClientCapabilities {
                            completion_item: Some(CompletionItemCapability {
                                documentation_format: Some(vec![
                                    MarkupKind::Markdown,
                                    MarkupKind::PlainText,
                                ]),
                                deprecated_support: Some(true),
                                preselect_support: Some(true),
                                label_details_support: Some(true),
                                resolve_support: Some(CompletionItemCapabilityResolveSupport {
                                    properties: vec!["detail".into(), "documentation".into()],
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
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
        let resolve_completions = initialized
            .capabilities
            .completion_provider
            .and_then(|provider| provider.resolve_provider)
            .unwrap_or(false);

        let config_path = config::util::config_path()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    root_dir.join(path)
                }
            })
            .unwrap_or_else(|| root_dir.join("init.lua"));
        let uri = Url::from_file_path(config_path).map_err(|_| anyhow!("failed to parse url"))?;
        server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "lua".into(),
                version: 0,
                text: "".into(),
            },
        })?;

        let types_uri = Url::from_file_path(root_dir.join("__pyonji_types__.lua"))
            .map_err(|_| anyhow!("failed to parse url"))?;
        server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: types_uri,
                language_id: "lua".into(),
                version: 0,
                text: config::DEFAULT_CONFIG.into(),
            },
        })?;

        Ok(Self {
            _child: child,
            server,
            uri,
            resolve_completions,
            version: 0,
            _task: task,
        })
    }

    pub fn push_changes(&mut self, text: &str) -> Result<()> {
        let version = self.version + 1;
        self.version = version;
        self.server.did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: self.uri.clone(),
                version: version as i32,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                // A full replacement also handles deletions, empty input and
                // Unicode without referring to offsets in the previous value.
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        })?;
        Ok(())
    }

    pub async fn get_completions_for(
        mut server: ServerSocket,
        uri: Url,
        position: Position,
    ) -> Result<Option<Vec<CompletionItem>>> {
        let r = server
            .completion(CompletionParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams {
                    work_done_token: None,
                },
                partial_result_params: PartialResultParams {
                    partial_result_token: None,
                },
                context: Some(CompletionContext {
                    trigger_kind: CompletionTriggerKind::INVOKED,
                    trigger_character: None,
                }),
            })
            .await?;
        let items = r.map(|res| match res {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        });

        Ok(items)
    }

    pub async fn resolve_item(
        server: &mut ServerSocket,
        item: CompletionItem,
    ) -> Result<CompletionItem> {
        Ok(server.completion_item_resolve(item).await?)
    }
}
