mod at_mention;
mod payload;
mod selection_observer;
mod workspace_observer;
mod workspace_tools;

pub use workspace_observer::register as register_workspace;

use editor::Editor;
use gpui::App;
use workspace::Workspace;

/// Subscribes to every newly-created editor and dispatches selection changes to
/// the Claude Code IDE bridge — locally to the in-process WS server, or over
/// proto to `remote_server` when the project is remote. Also installs a
/// workspace-tool handler on each new `Workspace` so the server can answer
/// `getCurrentSelection` / `getOpenEditors` synchronously without a proto
/// round-trip when running in local mode.
pub fn init(cx: &mut App) {
    workspace_observer::register_remote_dispatcher(cx);

    cx.observe_new(|editor: &mut Editor, window, cx| {
        selection_observer::register(editor, window, cx);
    })
    .detach();

    cx.observe_new(|workspace: &mut Workspace, _window, cx| {
        workspace_observer::register(workspace, cx);
    })
    .detach();
}
