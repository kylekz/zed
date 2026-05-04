use crate::{Project, ProjectPath, buffer_store::BufferStore, lsp_store::LspStore, worktree_store::WorktreeStore};
use claude_code_ide_server::{ProjectToolDispatcher, ToolResult};
use gpui::{App, Entity, Task, WeakEntity};
use language::Buffer;
use serde_json::{Value, json};
use std::path::PathBuf;

/// MCP tool dispatcher backed by a local `Project`. Wires the read-only
/// project-scoped tool surface into Project's stores. Workspace-scoped tools
/// (getCurrentSelection, getOpenEditors) are routed elsewhere — the local
/// `claude_code_ide` UI crate handles them via the workspace tool handler in a
/// later step.
pub struct LocalDispatcher {
    project: WeakEntity<Project>,
}

impl LocalDispatcher {
    pub fn new(project: WeakEntity<Project>) -> Self {
        Self { project }
    }
}

impl ProjectToolDispatcher for LocalDispatcher {
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

/// Dispatches a project-scoped tool against the underlying stores. Both the
/// local `Project` and the headless `HeadlessProject` hold the same store
/// types, so this single function serves both hosts.
pub fn dispatch_project_scoped_tool(
    tool_name: &str,
    arguments: &Value,
    worktree_store: &Entity<WorktreeStore>,
    buffer_store: &Entity<BufferStore>,
    lsp_store: &Entity<LspStore>,
    cx: &App,
) -> ToolResult {
    match tool_name {
        "getWorkspaceFolders" => workspace_folders_tool(worktree_store, cx),
        "checkDocumentDirty" => match parse_file_path(arguments) {
            Ok(path) => check_document_dirty_tool(worktree_store, buffer_store, &path, cx),
            Err(message) => ToolResult::error(message),
        },
        "getDiagnostics" => {
            let uri = parse_optional_uri(arguments);
            get_diagnostics_tool(worktree_store, buffer_store, lsp_store, uri.as_deref(), cx)
        }
        // CLI calls these at session start / during edits even though we
        // don't advertise them. Return the success shape the CLI's VS Code
        // extension would have returned so we don't spam error logs. Real
        // diff-tab integration is a deferred follow-up.
        "closeAllDiffTabs" => ToolResult::ok_text("CLOSED_0_DIFF_TABS"),
        "close_tab" => ToolResult::ok_text("TAB_CLOSED"),
        _ => ToolResult::error(format!("unknown tool: {tool_name}")),
    }
}

fn workspace_folders_tool(worktree_store: &Entity<WorktreeStore>, cx: &App) -> ToolResult {
    let folders: Vec<Value> = worktree_store
        .read(cx)
        .visible_worktrees(cx)
        .enumerate()
        .map(|(index, worktree)| {
            let worktree = worktree.read(cx);
            let abs_path = worktree.abs_path();
            let name = abs_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path_string = abs_path.to_string_lossy().into_owned();
            let uri = url::Url::from_file_path(abs_path.as_ref())
                .map(|url| url.to_string())
                .unwrap_or_else(|_| String::new());
            json!({
                "name": name,
                "uri": uri,
                "path": path_string,
                "index": index,
            })
        })
        .collect();
    let root_path = folders
        .first()
        .and_then(|folder| folder.get("path").and_then(Value::as_str))
        .map(str::to_owned);
    let payload = json!({
        "success": true,
        "folders": folders,
        "rootPath": root_path,
        "workspaceFile": Value::Null,
    });
    text_payload(&payload)
}

fn check_document_dirty_tool(
    worktree_store: &Entity<WorktreeStore>,
    buffer_store: &Entity<BufferStore>,
    file_path: &str,
    cx: &App,
) -> ToolResult {
    let abs_path = PathBuf::from(file_path);
    let Some(project_path) = worktree_store
        .read(cx)
        .project_path_for_absolute_path(&abs_path, cx)
    else {
        return text_payload(&json!({
            "success": false,
            "message": format!("Document not open: {file_path}"),
        }));
    };
    let Some(buffer) = buffer_store.read(cx).get_by_path(&project_path) else {
        return text_payload(&json!({
            "success": false,
            "message": format!("Document not open: {file_path}"),
        }));
    };
    let buffer_ref = buffer.read(cx);
    let is_untitled = buffer_ref.file().is_none();
    text_payload(&json!({
        "success": true,
        "filePath": file_path,
        "isDirty": buffer_ref.is_dirty(),
        "isUntitled": is_untitled,
    }))
}

fn get_diagnostics_tool(
    worktree_store: &Entity<WorktreeStore>,
    buffer_store: &Entity<BufferStore>,
    lsp_store: &Entity<LspStore>,
    uri_filter: Option<&str>,
    cx: &App,
) -> ToolResult {
    let target_path = uri_filter.and_then(uri_to_abs_path);
    let mut entries: Vec<Value> = Vec::new();

    if let Some(path) = target_path.as_ref() {
        if let Some(value) =
            diagnostics_entry_for_path(worktree_store, buffer_store, path, cx)
        {
            entries.push(value);
        }
        return text_payload(&Value::Array(entries));
    }

    let mut seen_paths = std::collections::HashSet::new();
    for (project_path, _, _) in lsp_store.read(cx).diagnostic_summaries(false, cx) {
        let abs_path = match abs_path_for(worktree_store, &project_path, cx) {
            Some(path) => path,
            None => continue,
        };
        if !seen_paths.insert(abs_path.clone()) {
            continue;
        }
        if let Some(value) =
            diagnostics_entry_for_path(worktree_store, buffer_store, &abs_path, cx)
        {
            entries.push(value);
        }
    }
    text_payload(&Value::Array(entries))
}

fn diagnostics_entry_for_path(
    worktree_store: &Entity<WorktreeStore>,
    buffer_store: &Entity<BufferStore>,
    abs_path: &PathBuf,
    cx: &App,
) -> Option<Value> {
    let project_path = worktree_store
        .read(cx)
        .project_path_for_absolute_path(abs_path, cx)?;
    let buffer = buffer_store.read(cx).get_by_path(&project_path)?;
    Some(diagnostics_entry_for_buffer(&buffer, abs_path, cx))
}

fn diagnostics_entry_for_buffer(buffer: &Entity<Buffer>, abs_path: &PathBuf, cx: &App) -> Value {
    let buffer = buffer.read(cx);
    let snapshot = buffer.snapshot();
    let max = snapshot.len();
    let diagnostics: Vec<Value> = snapshot
        .diagnostics_in_range::<_, language::Point>(0..max, false)
        .map(|entry| {
            let severity = severity_to_string(entry.diagnostic.severity);
            json!({
                "message": entry.diagnostic.message,
                "severity": severity,
                "range": {
                    "start": {
                        "line": entry.range.start.row,
                        "character": entry.range.start.column,
                    },
                    "end": {
                        "line": entry.range.end.row,
                        "character": entry.range.end.column,
                    },
                },
                "source": entry.diagnostic.source.clone(),
                "code": entry.diagnostic.code.clone().map(|code| code.to_string()),
            })
        })
        .collect();
    let uri = url::Url::from_file_path(abs_path)
        .map(|url| url.to_string())
        .unwrap_or_default();
    json!({
        "uri": uri,
        "linesInFile": snapshot.max_point().row + 1,
        "diagnostics": diagnostics,
    })
}

fn severity_to_string(severity: lsp::DiagnosticSeverity) -> &'static str {
    match severity {
        lsp::DiagnosticSeverity::ERROR => "Error",
        lsp::DiagnosticSeverity::WARNING => "Warning",
        lsp::DiagnosticSeverity::INFORMATION => "Information",
        lsp::DiagnosticSeverity::HINT => "Hint",
        _ => "Information",
    }
}

fn abs_path_for(
    worktree_store: &Entity<WorktreeStore>,
    project_path: &ProjectPath,
    cx: &App,
) -> Option<PathBuf> {
    let worktree = worktree_store.read(cx).worktree_for_id(project_path.worktree_id, cx)?;
    let worktree = worktree.read(cx);
    Some(worktree.abs_path().join(project_path.path.as_std_path()))
}

fn parse_file_path(arguments: &Value) -> Result<String, String> {
    let path = arguments
        .get("filePath")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("missing required argument: filePath"))?;
    if path.is_empty() {
        return Err(String::from("filePath must not be empty"));
    }
    Ok(path.to_owned())
}

fn parse_optional_uri(arguments: &Value) -> Option<String> {
    arguments
        .get("uri")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn uri_to_abs_path(uri: &str) -> Option<PathBuf> {
    if let Ok(url) = url::Url::parse(uri) {
        url.to_file_path().ok()
    } else {
        Some(PathBuf::from(uri))
    }
}

fn text_payload(value: &Value) -> ToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"));
    ToolResult::ok_text(text)
}
