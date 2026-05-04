use crate::protocol::ToolResult;
use gpui::{App, EntityId, Global, Task};
use serde_json::Value;
use std::sync::Arc;

/// Dispatches MCP tool calls into project/workspace primitives. Implemented by
/// the host crates (`project` for the local Project, `remote_server` for the
/// headless project) — the server crate cannot depend on either without
/// creating a cyclic import.
///
/// The trait method always succeeds at the protocol layer; tool-level failures
/// are encoded into `ToolResult` (`is_error = Some(true)`).
pub trait ProjectToolDispatcher: 'static {
    fn dispatch(
        &self,
        tool_name: &str,
        arguments: &Value,
        cx: &mut App,
    ) -> Task<ToolResult>;
}

/// Dispatches MCP tool calls that need access to a `Workspace` (e.g.
/// `getCurrentSelection`, `getOpenEditors`). In local mode this is set by the
/// `claude_code_ide` UI crate after observing a new `Workspace`; in remote
/// mode it is set by `remote_server` and round-trips a proto request to the UI
/// side, where the same UI logic resolves the matching workspace and runs the
/// tool. Either way, the server entity stays oblivious to the host kind.
pub trait WorkspaceToolHandler: 'static {
    fn dispatch(
        &self,
        tool_name: &str,
        arguments: &Value,
        cx: &mut App,
    ) -> Task<ToolResult>;
}

/// Returns true for tools that require workspace context (active editor,
/// open tabs, focus). Driven by the static tool inventory; new workspace tools
/// must be added here as well as in `mcp_stubs::tool_definitions`.
pub fn is_workspace_scoped_tool(tool_name: &str) -> bool {
    matches!(tool_name, "getCurrentSelection" | "getOpenEditors")
}

/// UI-side dispatcher for workspace tool calls that arrived over proto from
/// `remote_server`. Looks up the in-process `Workspace` for the given Project
/// `EntityId` and runs the requested tool. Implemented by the `claude_code_ide`
/// UI crate and registered as a GPUI global at app init.
pub trait RemoteWorkspaceDispatcher: Send + Sync + 'static {
    fn dispatch(
        &self,
        project_entity_id: EntityId,
        tool_name: &str,
        arguments: &Value,
        cx: &mut App,
    ) -> Task<ToolResult>;
}

/// GPUI global wrapper so other crates can register their dispatcher
/// implementation without a reverse dependency on `claude_code_ide`.
pub struct GlobalRemoteWorkspaceDispatcher(pub Arc<dyn RemoteWorkspaceDispatcher>);

impl Global for GlobalRemoteWorkspaceDispatcher {}

/// Convenience entry point for the inbound proto handler in the `project`
/// crate. Returns an error `ToolResult` if no dispatcher has been registered
/// (which happens in headless test contexts that never run `claude_code_ide::init`).
pub fn dispatch_remote_workspace_tool(
    project_entity_id: EntityId,
    tool_name: &str,
    arguments: &Value,
    cx: &mut App,
) -> Task<ToolResult> {
    let dispatcher = match cx.try_global::<GlobalRemoteWorkspaceDispatcher>() {
        Some(global) => global.0.clone(),
        None => {
            return Task::ready(ToolResult::error(
                "remote workspace tool dispatcher not initialized",
            ));
        }
    };
    dispatcher.dispatch(project_entity_id, tool_name, arguments, cx)
}
