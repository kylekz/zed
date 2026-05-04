use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub method: String,
    pub id: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification<'a, T: Serialize> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: T,
}

/// Builds a successful JSON-RPC response.
pub fn ok_response(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

/// Builds a JSON-RPC error response. Used for protocol-level failures (bad
/// params, unparseable arguments). Tool-level failures are reported via the
/// `ToolResult::error` shape and travel as a successful JSON-RPC response.
pub fn error_response(id: Value, code: i32, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
        }),
    }
}

/// Mirrors MCP's content-block shape. `kind` maps to the JSON `"type"` field.
#[derive(Debug, Clone, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text",
            text: text.into(),
        }
    }
}

/// MCP tool result body. Always serializes — `is_error` is omitted when `None`
/// to match the protocol's "absent means success" convention.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl ToolResult {
    pub fn ok(content: Vec<ContentBlock>) -> Self {
        Self {
            content,
            is_error: None,
        }
    }

    pub fn ok_text(text: impl Into<String>) -> Self {
        Self::ok(vec![ContentBlock::text(text)])
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(message)],
            is_error: Some(true),
        }
    }
}

/// Position payload mirrors `vscode.Position` (UTF-16 character offsets).
#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// Mirrors the VS Code extension's `selection_changed` payload shape.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPayload {
    pub text: String,
    pub file_path: String,
    pub file_url: String,
    pub selection: SelectionRange,
}

/// Mirrors the VS Code extension's `at_mentioned` notification payload. Sent
/// when the user explicitly invokes the "send selection as @mention" action;
/// `line_start` / `line_end` are omitted when the selection is empty
/// (matching the extension's behavior in `Y94` / `j94`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AtMentionPayload {
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
}

/// Mirrors the VS Code extension's `diagnostics_changed` notification.
/// Pushed after a 500ms quiet window so a burst of LSP updates collapses into
/// one frame.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsChangedPayload {
    pub uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
    pub is_empty: bool,
}
