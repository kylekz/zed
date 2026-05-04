use crate::protocol::{JsonRpcRequest, JsonRpcResponse, ok_response};
use serde_json::{Value, json};

/// Dispatches a single inbound MCP request that has a synchronous answer and
/// returns the response, if any. `tools/call` is *not* handled here — the
/// async path in `server.rs::handle_inbound` routes it through the foreground
/// dispatcher. Returns `None` for notifications (no `id` field) and unknown
/// methods, both of which the protocol treats as silent — matches the
/// permissiveness recommendation in the original plan.
pub fn handle_request(request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    let id = request.id.clone()?;
    let result = match request.method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "prompts": {},
                "resources": {},
            },
            "serverInfo": {
                "name": "zed-claude-code-ide",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
        "tools/list" => {
            let tools = tool_definitions();
            log::debug!(
                "claude-code-ide tools/list responding with {} tools",
                tools.len()
            );
            json!({ "tools": tools })
        }
        "prompts/list" => json!({ "prompts": [] }),
        "resources/list" => json!({ "resources": [] }),
        "ping" => Value::Object(Default::default()),
        method => {
            log::debug!("ignoring unknown MCP method: {method}");
            return None;
        }
    };
    Some(ok_response(id, result))
}

/// The static MCP tool surface advertised to the connected `claude` CLI. Each
/// entry mirrors the schema the VS Code extension publishes (verified in
/// `extension.js` line 865's `q.tool(...)` calls). Tool names that are not yet
/// wired through to a handler are intentionally absent — adding a name here
/// without a backing dispatcher arm will surface as
/// `ToolResult::error("unknown tool: ...")` to the CLI.
fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "getDiagnostics",
            "description": "Get language diagnostics from Zed",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "uri": {
                        "type": "string",
                        "description": "Optional file URI to get diagnostics for. If not provided, gets diagnostics for all files with loaded buffers."
                    }
                }
            }
        }),
        json!({
            "name": "getWorkspaceFolders",
            "description": "Get all workspace folders currently open in the IDE",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "checkDocumentDirty",
            "description": "Check if a document has unsaved changes (is dirty)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "Path to the file to check"
                    }
                },
                "required": ["filePath"]
            }
        }),
        json!({
            "name": "getLatestSelection",
            "description": "Get the most recent text selection (even if not in the active editor)",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "getCurrentSelection",
            "description": "Get the current text selection in the active editor",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "getOpenEditors",
            "description": "Get information about currently open editors",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
    ]
}
