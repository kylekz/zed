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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
    pub is_empty: bool,
}
