mod payload;
mod selection_observer;

use editor::Editor;
use gpui::App;

/// Subscribes to every newly-created editor and dispatches selection changes to
/// the Claude Code IDE bridge — locally to the in-process WS server, or over
/// proto to `remote_server` when the project is remote.
pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, window, cx| {
        selection_observer::register(editor, window, cx);
    })
    .detach();
}
