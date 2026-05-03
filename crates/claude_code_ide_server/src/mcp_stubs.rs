use crate::protocol::{JsonRpcRequest, JsonRpcResponse, ok_response};
use serde_json::{Value, json};

/// Dispatches a single inbound MCP request and returns the response, if any.
/// Returns `None` for notifications (no `id` field) and unknown methods, both
/// of which the protocol treats as silent — matches the permissiveness
/// recommendation in the plan.
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
        "tools/list" => json!({ "tools": [] }),
        "prompts/list" => json!({ "prompts": [] }),
        "resources/list" => json!({ "resources": [] }),
        "tools/call" => json!({
            "isError": true,
            "content": [{ "type": "text", "text": "no tools" }],
        }),
        "ping" => Value::Object(Default::default()),
        method => {
            log::debug!("ignoring unknown MCP method: {method}");
            return None;
        }
    };
    Some(ok_response(id, result))
}
