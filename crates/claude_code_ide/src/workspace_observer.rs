use crate::workspace_tools;
use claude_code_ide_server::{
    GlobalRemoteWorkspaceDispatcher, RemoteWorkspaceDispatcher, ToolResult, WorkspaceToolHandler,
};
use gpui::{App, Context, EntityId, Task, WeakEntity};
use serde_json::Value;
use std::sync::Arc;
use workspace::{MultiWorkspace, Workspace};

/// Workspace-tool handler bound to a specific in-process `Workspace`. Returned
/// to the `ClaudeCodeIdeServer` entity by `register` below; lives as long as
/// the workspace lives.
pub(crate) struct LocalWorkspaceToolHandler {
    workspace: WeakEntity<Workspace>,
}

impl WorkspaceToolHandler for LocalWorkspaceToolHandler {
    fn dispatch(
        &self,
        tool_name: &str,
        _arguments: &Value,
        cx: &mut App,
    ) -> Task<ToolResult> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Task::ready(ToolResult::error("workspace dropped"));
        };
        let result = match tool_name {
            "getCurrentSelection" => workspace_tools::get_current_selection(&workspace, cx),
            "getOpenEditors" => workspace_tools::get_open_editors(&workspace, cx),
            other => ToolResult::error(format!("unknown workspace tool: {other}")),
        };
        Task::ready(result)
    }
}

/// UI-side dispatcher for proto-borne workspace tool calls. Resolves the
/// matching `Workspace` by walking `cx.windows()`, then runs the tool. Picks
/// the first match — multi-window-per-Project setups are rare; revisit if a
/// real workflow needs a deterministic disambiguation.
pub(crate) struct UiRemoteWorkspaceDispatcher;

impl RemoteWorkspaceDispatcher for UiRemoteWorkspaceDispatcher {
    fn dispatch(
        &self,
        project_entity_id: EntityId,
        tool_name: &str,
        _arguments: &Value,
        cx: &mut App,
    ) -> Task<ToolResult> {
        let workspace = lookup_workspace(project_entity_id, cx);
        let Some(workspace) = workspace else {
            return Task::ready(ToolResult::error("no workspace bound to project"));
        };
        let result = match tool_name {
            "getCurrentSelection" => workspace_tools::get_current_selection(&workspace, cx),
            "getOpenEditors" => workspace_tools::get_open_editors(&workspace, cx),
            other => ToolResult::error(format!("unknown workspace tool: {other}")),
        };
        Task::ready(result)
    }
}

fn lookup_workspace(
    project_entity_id: EntityId,
    cx: &mut App,
) -> Option<gpui::Entity<Workspace>> {
    for window in cx.windows() {
        let Some(handle) = window.downcast::<MultiWorkspace>() else {
            continue;
        };
        let multi = match handle.read(cx) {
            Ok(value) => value,
            Err(_) => continue,
        };
        for workspace in multi.workspaces() {
            if workspace.read(cx).project().entity_id() == project_entity_id {
                return Some(workspace.clone());
            }
        }
    }
    None
}

/// Called for every new `Workspace`. Registers the at-mention action and (in
/// local mode) installs a workspace-tool handler so the server can route
/// workspace-scoped tool calls back into this workspace.
pub fn register(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    crate::at_mention::register(workspace);

    let project = workspace.project().clone();
    let Some(server) = project.read(cx).claude_code_ide_server().cloned() else {
        return;
    };
    let handler = Box::new(LocalWorkspaceToolHandler {
        workspace: cx.weak_entity(),
    });
    server.update(cx, |server, cx| {
        server.set_workspace_tool_handler(handler, cx);
    });
}

/// Registers the UI-side dispatcher used by the inbound proto handler in
/// `project`. Called once at app init.
pub fn register_remote_dispatcher(cx: &mut App) {
    cx.set_global(GlobalRemoteWorkspaceDispatcher(Arc::new(
        UiRemoteWorkspaceDispatcher,
    )));
}
