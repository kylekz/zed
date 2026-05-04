mod host;
mod lockfile;
mod mcp_stubs;
mod protocol;
mod server;

pub use host::{
    GlobalRemoteWorkspaceDispatcher, ProjectToolDispatcher, RemoteWorkspaceDispatcher,
    WorkspaceToolHandler, dispatch_remote_workspace_tool, is_workspace_scoped_tool,
};
pub use protocol::{
    AtMentionPayload, ContentBlock, DiagnosticsChangedPayload, Position, SelectionPayload,
    SelectionRange, ToolResult,
};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc;
use futures::stream::StreamExt as _;
use gpui::{App, AppContext as _, Context, Entity, Task};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::server::PendingToolCall;

/// Per-project Claude Code IDE bridge. Owns the WS server lifecycle, lockfile,
/// outbound channel to the connected `claude` CLI client, and the foreground
/// loop that bridges tokio-side tool requests into GPUI updates.
pub struct ClaudeCodeIdeServer {
    port: u16,
    lockfile_path: PathBuf,
    outbound: server::OutboundSender,
    workspace_folders: Vec<String>,
    auth_token: Arc<String>,
    dispatcher: Box<dyn ProjectToolDispatcher>,
    workspace_handler: Option<Box<dyn WorkspaceToolHandler>>,
    latest_selection: Option<SelectionPayload>,
    pending_diagnostic_uris: BTreeSet<String>,
    diagnostics_flush_task: Option<Task<()>>,
    _tool_call_loop: Task<()>,
    _accept_loop: Task<Result<(), gpui_tokio::JoinError>>,
}

impl ClaudeCodeIdeServer {
    /// Binds a port, writes the lockfile, and spawns the WS accept loop on the
    /// shared Tokio runtime. Returns the owning entity. The accept loop and
    /// tool-call bridge are cancelled when the entity is dropped.
    pub fn try_spawn(
        workspace_folders: Vec<String>,
        dispatcher: Box<dyn ProjectToolDispatcher>,
        cx: &mut App,
    ) -> Result<Entity<Self>> {
        if !gpui_tokio::Tokio::is_initialized(cx) {
            return Err(anyhow!("gpui_tokio runtime is not initialized"))
                .context("cannot start claude-code-ide WS server");
        }
        let auth_token = Arc::new(uuid::Uuid::new_v4().to_string());
        let outbound: server::OutboundSender = Arc::new(tokio::sync::Mutex::new(None));

        let (std_listener, port) = server::bind_listener_sync()?;
        let dir = lockfile::ide_dir()?;
        let lockfile_path = dir.join(format!("{port}.lock"));
        lockfile::write(
            &lockfile_path,
            std::process::id(),
            &workspace_folders,
            &auth_token,
        )?;
        log::info!(
            "claude-code-ide listening on 127.0.0.1:{port}, lockfile {}",
            lockfile_path.display()
        );

        let (tool_call_tx, tool_call_rx) = mpsc::unbounded::<PendingToolCall>();

        Ok(cx.new(|cx| {
            let tool_call_loop = cx.spawn(async move |this, cx| {
                let mut tool_call_rx = tool_call_rx;
                while let Some(call) = tool_call_rx.next().await {
                    let task = this
                        .update(cx, |this: &mut Self, cx| {
                            this.dispatch_tool(&call.tool_name, &call.arguments, cx)
                        })
                        .ok();
                    let result = match task {
                        Some(task) => task.await,
                        None => ToolResult::error("server entity dropped"),
                    };
                    let _ = call.responder.send(result);
                }
            });

            let accept_loop = gpui_tokio::Tokio::spawn(cx, {
                let auth_token = auth_token.clone();
                let outbound = outbound.clone();
                let tool_call_tx = tool_call_tx.clone();
                async move {
                    let listener = match tokio::net::TcpListener::from_std(std_listener) {
                        Ok(listener) => listener,
                        Err(error) => {
                            log::warn!(
                                "claude-code-ide failed to register listener: {error}"
                            );
                            return;
                        }
                    };
                    if let Err(error) =
                        server::run(listener, auth_token, outbound, tool_call_tx).await
                    {
                        log::warn!("claude-code-ide accept loop ended: {error}");
                    }
                }
            });
            Self {
                port,
                lockfile_path,
                outbound,
                workspace_folders,
                auth_token,
                dispatcher,
                workspace_handler: None,
                latest_selection: None,
                pending_diagnostic_uris: BTreeSet::new(),
                diagnostics_flush_task: None,
                _tool_call_loop: tool_call_loop,
                _accept_loop: accept_loop,
            }
        }))
    }

    /// Installs the workspace-scoped tool handler. Called once per server
    /// instance — by `claude_code_ide` after a `Workspace` is created in local
    /// mode, and by `remote_server` immediately after the headless server
    /// starts. Replaces any previously-installed handler.
    pub fn set_workspace_tool_handler(
        &mut self,
        handler: Box<dyn WorkspaceToolHandler>,
        _cx: &mut Context<Self>,
    ) {
        self.workspace_handler = Some(handler);
    }

    /// Returns the port bound by the WS server.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the workspace folders stamped into the lockfile.
    pub fn workspace_folders(&self) -> &[String] {
        &self.workspace_folders
    }

    /// Rewrites the lockfile when the project's workspace folders change. The
    /// port and auth token are preserved.
    pub fn update_workspace_folders(&mut self, folders: Vec<String>, _cx: &mut Context<Self>) {
        if folders == self.workspace_folders {
            return;
        }
        self.workspace_folders = folders;
        if let Err(error) = lockfile::write(
            &self.lockfile_path,
            std::process::id(),
            &self.workspace_folders,
            &self.auth_token,
        ) {
            log::warn!("failed to refresh claude-code-ide lockfile: {error}");
        }
    }

    /// Pushes a `selection_changed` notification to the connected client and
    /// caches the payload for subsequent `getLatestSelection` tool calls. If
    /// no client is connected the WS notification is dropped — selection
    /// events are notifications, not requests — but the cache is still
    /// updated.
    pub fn notify_selection_changed(
        &mut self,
        payload: SelectionPayload,
        cx: &mut Context<Self>,
    ) {
        self.latest_selection = Some(payload.clone());
        let outbound = self.outbound.clone();
        gpui_tokio::Tokio::spawn(cx, async move {
            server::send_selection_changed(&outbound, payload).await;
        })
        .detach();
    }

    /// Returns the most recent selection payload received from the editor, if
    /// any. Updated on every `notify_selection_changed`.
    pub fn latest_selection(&self) -> Option<&SelectionPayload> {
        self.latest_selection.as_ref()
    }

    /// Pushes an `at_mentioned` notification to the connected client. Dropped
    /// silently when no client is connected. Triggered by the workspace action
    /// `claude_code_ide::SendSelectionAsAtMention` (no default keybinding).
    pub fn notify_at_mentioned(&self, payload: AtMentionPayload, cx: &App) {
        let outbound = self.outbound.clone();
        gpui_tokio::Tokio::spawn(cx, async move {
            server::send_at_mentioned(&outbound, payload).await;
        })
        .detach();
    }

    /// Buffers `diagnostics_changed` URIs and schedules a single flush after a
    /// 500ms quiet window. A burst of LSP updates collapses into one
    /// notification frame with deduplicated URIs.
    pub fn enqueue_diagnostics_changed(
        &mut self,
        uris: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        let mut added = false;
        for uri in uris {
            if self.pending_diagnostic_uris.insert(uri) {
                added = true;
            }
        }
        if !added {
            return;
        }
        if self.diagnostics_flush_task.is_some() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let Ok(payload) = this.update(cx, |this, _cx| {
                this.diagnostics_flush_task = None;
                let drained: Vec<String> =
                    std::mem::take(&mut this.pending_diagnostic_uris).into_iter().collect();
                if drained.is_empty() {
                    None
                } else {
                    Some(DiagnosticsChangedPayload { uris: drained })
                }
            }) else {
                return;
            };
            let Some(payload) = payload else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let outbound = this.outbound.clone();
                gpui_tokio::Tokio::spawn(cx, async move {
                    server::send_diagnostics_changed(&outbound, payload).await;
                })
                .detach();
            });
        });
        self.diagnostics_flush_task = Some(task);
    }

    /// Routes a tool call. Tools that depend on the server's own state
    /// (`getLatestSelection`) are answered here directly; workspace-scoped
    /// tools delegate to the optional workspace handler; everything else
    /// delegates to the project-scoped dispatcher provided at construction.
    fn dispatch_tool(
        &mut self,
        tool_name: &str,
        arguments: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        if tool_name == "getLatestSelection" {
            return Task::ready(latest_selection_tool_result(self.latest_selection.as_ref()));
        }
        if is_workspace_scoped_tool(tool_name) {
            return match self.workspace_handler.as_ref() {
                Some(handler) => handler.dispatch(tool_name, arguments, cx),
                None => Task::ready(ToolResult::error(
                    "no workspace bound to this project — open the project in a Zed window first",
                )),
            };
        }
        self.dispatcher.dispatch(tool_name, arguments, cx)
    }
}

fn latest_selection_tool_result(payload: Option<&SelectionPayload>) -> ToolResult {
    let value = match payload {
        Some(payload) => match serde_json::to_value(payload) {
            Ok(mut value) => {
                if let Some(object) = value.as_object_mut() {
                    object.insert("success".to_string(), serde_json::Value::Bool(true));
                }
                value
            }
            Err(error) => {
                return ToolResult::error(format!("failed to encode selection: {error}"));
            }
        },
        None => json!({ "success": false, "message": "No selection available" }),
    };
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::from("{}"));
    ToolResult::ok_text(text)
}

impl Drop for ClaudeCodeIdeServer {
    fn drop(&mut self) {
        lockfile::remove(&self.lockfile_path);
    }
}
