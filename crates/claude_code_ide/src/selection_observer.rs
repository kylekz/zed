use crate::payload;
use claude_code_ide_server::SelectionPayload;
use editor::{Editor, EditorEvent};
use gpui::{App, Context, Subscription, Task, Window};
use language::Point;
use project::{Project, ProjectPath};
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Per-editor state attached as an Editor `Addon`. The subscription auto-drops
/// when the editor is dropped.
pub(crate) struct SelectionObserverAddon {
    _subscription: Subscription,
    debounce: Option<Task<()>>,
    last_signature: Option<Signature>,
}

#[derive(Clone, PartialEq, Eq)]
struct Signature {
    file_path: String,
    text: String,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
}

impl editor::Addon for SelectionObserverAddon {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Registers the selection observer on a freshly-created editor. Skips
/// editors that aren't in the editing modes the CLI cares about.
pub fn register(editor: &mut Editor, _window: Option<&mut Window>, cx: &mut Context<Editor>) {
    if !editor.mode().is_full() || editor.read_only(cx) {
        return;
    }
    let subscription = cx.subscribe(&cx.entity(), move |editor, _, event, cx| {
        if let EditorEvent::SelectionsChanged { local: true } = event {
            on_selections_changed(editor, cx);
        }
    });
    editor.register_addon(SelectionObserverAddon {
        _subscription: subscription,
        debounce: None,
        last_signature: None,
    });
}

fn on_selections_changed(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(project) = editor.project().cloned() else {
        return;
    };
    let multi_buffer = editor.buffer().clone();
    let snapshot = multi_buffer.read(cx).snapshot(cx);
    let display_snapshot = editor.display_snapshot(cx);
    let newest = editor.selections.newest::<Point>(&display_snapshot);
    let range = newest.start..newest.end;

    let buffer_entity = match snapshot.as_singleton() {
        Some(buffer_snapshot) => multi_buffer
            .read(cx)
            .buffer(buffer_snapshot.remote_id()),
        None => return,
    };
    let Some(buffer_entity) = buffer_entity else {
        return;
    };

    let buffer = buffer_entity.read(cx);
    let Some(file) = buffer.file() else {
        return;
    };
    let project_path = ProjectPath {
        worktree_id: file.worktree_id(cx),
        path: file.path().clone(),
    };
    let Some(abs_path) = project.read(cx).absolute_path(&project_path, cx) else {
        return;
    };

    let start_utf16 = snapshot.point_to_point_utf16(range.start);
    let end_utf16 = snapshot.point_to_point_utf16(range.end);
    let text: String = snapshot.text_for_range(range).collect();

    let payload = payload::build(
        text,
        &abs_path,
        start_utf16.row,
        start_utf16.column,
        end_utf16.row,
        end_utf16.column,
    );

    let Some(addon) = editor.addon_mut::<SelectionObserverAddon>() else {
        return;
    };
    let signature = Signature {
        file_path: payload.file_path.clone(),
        text: payload.text.clone(),
        start_line: payload.selection.start.line,
        start_character: payload.selection.start.character,
        end_line: payload.selection.end.line,
        end_character: payload.selection.end.character,
    };
    if addon.last_signature.as_ref() == Some(&signature) {
        return;
    }
    addon.last_signature = Some(signature);

    addon.debounce = Some(cx.spawn(async move |_editor, cx| {
        cx.background_executor().timer(DEBOUNCE).await;
        cx.update(|cx| dispatch(&project, payload, cx));
    }));
}

fn dispatch(project: &gpui::Entity<Project>, payload: SelectionPayload, cx: &mut App) {
    let project_ref = project.read(cx);
    if project_ref.is_via_remote_server() {
        if let Some(remote_client) = project_ref.remote_client() {
            let proto_client = remote_client.read(cx).proto_client();
            let request = proto::ClaudeCodeIdeSelectionChanged {
                project_id: proto::REMOTE_SERVER_PROJECT_ID,
                file_path: payload.file_path,
                file_url: payload.file_url,
                text: payload.text,
                start_line: payload.selection.start.line,
                start_character: payload.selection.start.character,
                end_line: payload.selection.end.line,
                end_character: payload.selection.end.character,
                is_empty: payload.selection.is_empty,
            };
            if let Err(error) = proto_client.send(request) {
                log::debug!("claude-code-ide: failed to forward selection: {error}");
            }
        }
        return;
    }

    if !project_ref.is_local() {
        return;
    }
    if let Some(server) = project_ref.claude_code_ide_server().cloned() {
        server.read(cx).notify_selection_changed(payload, cx);
    }
}
