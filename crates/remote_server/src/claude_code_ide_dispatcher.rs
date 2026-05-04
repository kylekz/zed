use crate::headless_project::HeadlessProject;
use claude_code_ide_server::{ProjectToolDispatcher, ToolResult, WorkspaceToolHandler};
use gpui::{App, AppContext as _, Task, WeakEntity};
use project::claude_code_ide_dispatcher::dispatch_project_scoped_tool;
use rpc::AnyProtoClient;
use rpc::proto::{self, REMOTE_SERVER_PROJECT_ID};
use serde_json::Value;

/// MCP tool dispatcher backed by a `HeadlessProject`. Reuses the project-scoped
/// tool functions from the `project` crate — both hosts share the same
/// `WorktreeStore`/`BufferStore`/`LspStore` types, so the tool logic is host
/// agnostic. Workspace-scoped tools are routed to the UI over proto by
/// `RemoteWorkspaceToolHandler` (set on the server entity at startup).
pub struct RemoteDispatcher {
    project: WeakEntity<HeadlessProject>,
}

impl RemoteDispatcher {
    pub fn new(project: WeakEntity<HeadlessProject>) -> Self {
        Self { project }
    }
}

impl ProjectToolDispatcher for RemoteDispatcher {
    fn dispatch(&self, tool_name: &str, arguments: &Value, cx: &mut App) -> Task<ToolResult> {
        let Some(project) = self.project.upgrade() else {
            return Task::ready(ToolResult::error("project entity dropped"));
        };
        let project = project.read(cx);
        let result = dispatch_project_scoped_tool(
            tool_name,
            arguments,
            &project.worktree_store,
            &project.buffer_store,
            &project.lsp_store,
            cx,
        );
        Task::ready(result)
    }
}

/// Workspace-scoped tool handler that round-trips a typed proto request to the
/// UI side. The UI's Project handler resolves the matching `Workspace` and
/// runs the tool; the response carries the MCP `content` text back. Headless
/// session has no in-process Workspace, so this is the only path.
pub struct RemoteWorkspaceToolHandler {
    session: AnyProtoClient,
}

impl RemoteWorkspaceToolHandler {
    pub fn new(session: AnyProtoClient) -> Self {
        Self { session }
    }
}

impl WorkspaceToolHandler for RemoteWorkspaceToolHandler {
    fn dispatch(&self, tool_name: &str, arguments: &Value, cx: &mut App) -> Task<ToolResult> {
        let session = self.session.clone();
        let tool_name = tool_name.to_string();
        let params_json = serde_json::to_string(arguments).unwrap_or_else(|_| String::from("{}"));
        cx.background_spawn(async move {
            let request = session.request(proto::ClaudeCodeIdeWorkspaceToolCall {
                project_id: REMOTE_SERVER_PROJECT_ID,
                tool_name,
                params_json,
            });
            match request.await {
                Ok(response) => {
                    if response.is_error {
                        ToolResult::error(response.content_json)
                    } else {
                        ToolResult::ok_text(response.content_json)
                    }
                }
                Err(error) => ToolResult::error(format!(
                    "workspace tool round-trip failed: {error}"
                )),
            }
        })
    }
}
