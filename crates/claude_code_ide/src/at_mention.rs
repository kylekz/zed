use claude_code_ide_server::AtMentionPayload;
use editor::Editor;
use gpui::{App, Window, actions};
use language::Point;
use project::{Project, ProjectPath};
use workspace::Workspace;

actions!(
    claude_code_ide,
    [
        /// Send the current editor selection to the connected `claude` CLI as
        /// an `@mention`, regardless of whether the selection has changed.
        /// Mirrors the VS Code extension's "send to Claude as mention"
        /// command. No default keybinding — bind in the keymap if you want
        /// one.
        SendSelectionAsAtMention,
    ]
);

/// Registers the at-mention action on a fresh `Workspace`. Called from the
/// workspace observer so the action is available everywhere a workspace is.
pub fn register(workspace: &mut Workspace) {
    workspace.register_action(
        |workspace, _: &SendSelectionAsAtMention, _window: &mut Window, cx| {
            send_at_mention(workspace, cx);
        },
    );
}

fn send_at_mention(workspace: &Workspace, cx: &mut App) {
    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        log::debug!("claude-code-ide at_mention: no active editor");
        return;
    };
    let project = workspace.project().clone();
    let payload = match build_payload(&editor, &project, cx) {
        Some(payload) => payload,
        None => return,
    };
    dispatch(&project, payload, cx);
}

fn build_payload(
    editor: &gpui::Entity<Editor>,
    project: &gpui::Entity<Project>,
    cx: &mut App,
) -> Option<AtMentionPayload> {
    let multi_buffer = editor.read(cx).buffer().clone();
    let snapshot = multi_buffer.read(cx).snapshot(cx);
    let display_snapshot = editor.update(cx, |editor, cx| editor.display_snapshot(cx));
    let newest = editor.read(cx).selections.newest::<Point>(&display_snapshot);
    let range = newest.start..newest.end;

    let buffer_entity = match snapshot.as_singleton() {
        Some(buffer_snapshot) => multi_buffer.read(cx).buffer(buffer_snapshot.remote_id()),
        None => return None,
    };
    let buffer_entity = buffer_entity?;

    let project_path = {
        let buffer = buffer_entity.read(cx);
        let file = buffer.file()?;
        ProjectPath {
            worktree_id: file.worktree_id(cx),
            path: file.path().clone(),
        }
    };
    let abs_path = project.read(cx).absolute_path(&project_path, cx)?;
    let file_path = abs_path.to_string_lossy().into_owned();

    let is_empty = newest.start == newest.end;
    let (line_start, line_end) = if is_empty {
        (None, None)
    } else {
        (Some(range.start.row), Some(range.end.row))
    };

    Some(AtMentionPayload {
        file_path,
        line_start,
        line_end,
    })
}

fn dispatch(project: &gpui::Entity<Project>, payload: AtMentionPayload, cx: &mut App) {
    let project_ref = project.read(cx);
    if project_ref.is_via_remote_server() {
        if let Some(remote_client) = project_ref.remote_client() {
            let proto_client = remote_client.read(cx).proto_client();
            let request = proto::ClaudeCodeIdeAtMentioned {
                project_id: proto::REMOTE_SERVER_PROJECT_ID,
                file_path: payload.file_path,
                line_start: payload.line_start,
                line_end: payload.line_end,
            };
            if let Err(error) = proto_client.send(request) {
                log::debug!("claude-code-ide at_mention: failed to forward: {error}");
            }
        }
        return;
    }

    if !project_ref.is_local() {
        return;
    }
    if let Some(server) = project_ref.claude_code_ide_server().cloned() {
        server.read(cx).notify_at_mentioned(payload, cx);
    }
}
