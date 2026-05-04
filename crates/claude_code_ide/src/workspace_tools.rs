use claude_code_ide_server::ToolResult;
use editor::Editor;
use gpui::{App, Entity};
use language::Point;
use serde_json::{Value, json};
use workspace::{Item as _, Workspace};

/// Implements the `getCurrentSelection` MCP tool. Returns the selection of the
/// active editor in this workspace, or a `success: false` payload when no
/// editor is focused.
pub fn get_current_selection(workspace: &Entity<Workspace>, cx: &mut App) -> ToolResult {
    let (editor, project) = {
        let workspace_ref = workspace.read(cx);
        let Some(editor) = workspace_ref.active_item_as::<Editor>(cx) else {
            return text_payload(&json!({
                "success": false,
                "message": "No active editor found",
            }));
        };
        (editor, workspace_ref.project().clone())
    };

    let multi_buffer = editor.read(cx).buffer().clone();
    let snapshot = multi_buffer.read(cx).snapshot(cx);
    let display_snapshot = editor.update(cx, |editor, cx| editor.display_snapshot(cx));
    let newest = editor.read(cx).selections.newest::<Point>(&display_snapshot);
    let range = newest.start..newest.end;

    let buffer_entity = match snapshot.as_singleton() {
        Some(buffer_snapshot) => multi_buffer.read(cx).buffer(buffer_snapshot.remote_id()),
        None => {
            return text_payload(&json!({
                "success": false,
                "message": "Active editor is a multibuffer",
            }));
        }
    };
    let Some(buffer_entity) = buffer_entity else {
        return text_payload(&json!({
            "success": false,
            "message": "Active editor has no backing buffer",
        }));
    };

    let project_path = {
        let buffer = buffer_entity.read(cx);
        let Some(file) = buffer.file() else {
            return text_payload(&json!({
                "success": false,
                "message": "Active buffer has no file",
            }));
        };
        project::ProjectPath {
            worktree_id: file.worktree_id(cx),
            path: file.path().clone(),
        }
    };
    let Some(abs_path) = project.read(cx).absolute_path(&project_path, cx) else {
        return text_payload(&json!({
            "success": false,
            "message": "Could not resolve absolute path for active buffer",
        }));
    };
    let file_path = abs_path.to_string_lossy().into_owned();
    let file_url = url::Url::from_file_path(&abs_path)
        .map(|url| url.to_string())
        .unwrap_or_default();

    let start_utf16 = snapshot.point_to_point_utf16(range.start);
    let end_utf16 = snapshot.point_to_point_utf16(range.end);
    let text: String = snapshot.text_for_range(range).collect();
    let is_empty = newest.start == newest.end;

    text_payload(&json!({
        "success": true,
        "text": text,
        "filePath": file_path,
        "fileUrl": file_url,
        "selection": {
            "start": { "line": start_utf16.row, "character": start_utf16.column },
            "end": { "line": end_utf16.row, "character": end_utf16.column },
            "isEmpty": is_empty,
        }
    }))
}

/// Implements the `getOpenEditors` MCP tool. Walks every pane in the workspace
/// and emits one entry per `Editor` item with a backing file. Items without a
/// file (search results, terminals, panels) are skipped to match VS Code's
/// `vscode.window.tabGroups.all → TabInputText` filter.
pub fn get_open_editors(workspace: &Entity<Workspace>, cx: &mut App) -> ToolResult {
    let workspace_ref = workspace.read(cx);
    let project = workspace_ref.project();
    let active_pane = workspace_ref.active_pane();
    let active_pane_id = active_pane.entity_id();
    let active_item_id = active_pane.read(cx).active_item().map(|item| item.item_id());

    let mut tabs: Vec<Value> = Vec::new();
    for (group_index, pane) in workspace_ref.panes().iter().enumerate() {
        let pane_ref = pane.read(cx);
        let pane_active_item_id = pane_ref.active_item().map(|item| item.item_id());
        let is_group_active = pane.entity_id() == active_pane_id;
        let pinned_count = pane_ref.pinned_count();
        for (item_index, editor) in pane_ref.items_of_type::<Editor>().enumerate() {
            let editor_ref = editor.read(cx);
            let multi_buffer = editor_ref.buffer().clone();
            let snapshot = multi_buffer.read(cx).snapshot(cx);
            let buffer_entity = match snapshot.as_singleton() {
                Some(buffer_snapshot) => multi_buffer.read(cx).buffer(buffer_snapshot.remote_id()),
                None => continue,
            };
            let Some(buffer_entity) = buffer_entity else {
                continue;
            };
            let buffer = buffer_entity.read(cx);
            let Some(file) = buffer.file() else {
                continue;
            };
            let project_path = project::ProjectPath {
                worktree_id: file.worktree_id(cx),
                path: file.path().clone(),
            };
            let Some(abs_path) = project.read(cx).absolute_path(&project_path, cx) else {
                continue;
            };
            let uri = url::Url::from_file_path(&abs_path)
                .map(|url| url.to_string())
                .unwrap_or_default();
            let label = editor.read(cx).tab_content_text(0, cx).to_string();
            let language_id = buffer
                .language()
                .map(|language| language.name().to_string());
            let is_active =
                Some(editor.entity_id()) == pane_active_item_id && is_group_active
                    || Some(editor.entity_id()) == active_item_id;
            let is_pinned = item_index < pinned_count;
            tabs.push(json!({
                "uri": uri,
                "fileName": abs_path.to_string_lossy(),
                "label": label,
                "isActive": is_active,
                "isPinned": is_pinned,
                "isPreview": false,
                "isDirty": buffer.is_dirty(),
                "isUntitled": false,
                "languageId": language_id,
                "lineCount": snapshot.max_point().row + 1,
                "groupIndex": group_index,
                "viewColumn": (group_index + 1),
                "isGroupActive": is_group_active,
            }));
        }
    }

    text_payload(&json!({ "tabs": tabs }))
}

fn text_payload(value: &Value) -> ToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"));
    ToolResult::ok_text(text)
}
