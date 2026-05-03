mod lockfile;
mod mcp_stubs;
mod protocol;
mod server;

pub use protocol::{Position, SelectionPayload, SelectionRange};

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, AppContext as _, Context, Entity, Task};
use std::path::PathBuf;
use std::sync::Arc;

/// Per-project Claude Code IDE bridge. Owns the WS server lifecycle, lockfile,
/// and outbound channel to the connected `claude` CLI client.
pub struct ClaudeCodeIdeServer {
    port: u16,
    lockfile_path: PathBuf,
    outbound: server::OutboundSender,
    workspace_folders: Vec<String>,
    auth_token: Arc<String>,
    _accept_loop: Task<Result<(), gpui_tokio::JoinError>>,
}

impl ClaudeCodeIdeServer {
    /// Binds a port, writes the lockfile, and spawns the WS accept loop on the
    /// shared Tokio runtime. Returns the owning entity. The accept loop is
    /// cancelled when the entity is dropped.
    pub fn try_spawn(
        workspace_folders: Vec<String>,
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

        Ok(cx.new(|cx| {
            let accept_loop = gpui_tokio::Tokio::spawn(cx, {
                let auth_token = auth_token.clone();
                let outbound = outbound.clone();
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
                    if let Err(error) = server::run(listener, auth_token, outbound).await {
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
                _accept_loop: accept_loop,
            }
        }))
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

    /// Pushes a `selection_changed` notification to the connected client. If
    /// no client is connected the message is dropped — selection events are
    /// notifications, not requests.
    pub fn notify_selection_changed(&self, payload: SelectionPayload, cx: &App) {
        let outbound = self.outbound.clone();
        gpui_tokio::Tokio::spawn(cx, async move {
            server::send_selection_changed(&outbound, payload).await;
        })
        .detach();
    }
}

impl Drop for ClaudeCodeIdeServer {
    fn drop(&mut self) {
        lockfile::remove(&self.lockfile_path);
    }
}
