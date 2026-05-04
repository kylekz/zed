use crate::mcp_stubs;
use crate::protocol::{
    JsonRpcNotification, JsonRpcRequest, ToolResult, error_response, ok_response,
};
use crate::protocol::{AtMentionPayload, DiagnosticsChangedPayload, SelectionPayload};
use anyhow::{Context as _, Result, anyhow, bail};
use futures::SinkExt as _;
use futures::StreamExt as _;
use futures::channel::{mpsc as fut_mpsc, oneshot};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;

const AUTH_HEADER: &str = "x-claude-code-ide-authorization";

/// Outbound channel for sending text frames to the currently-connected client.
/// `None` when no client is connected. Wrapped in `Arc<Mutex<...>>` so the
/// accept loop can swap it as connections come and go, while other tasks (like
/// `notify_selection_changed`) push into whatever sender is current.
pub type OutboundSender = Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>>;

/// Bridge envelope from the WS reader (tokio-side) to the foreground tool-call
/// loop. The loop runs the dispatcher under `&mut App` and sends the result
/// back via `responder`.
pub struct PendingToolCall {
    pub tool_name: String,
    pub arguments: Value,
    pub responder: oneshot::Sender<ToolResult>,
}

/// Picks a port in [10000, 65535] and binds a TCP listener synchronously.
/// Retries up to 50 times before giving up. Returns the bound listener (in
/// non-blocking mode, ready to be wrapped in `tokio::net::TcpListener`) and
/// its port.
pub fn bind_listener_sync() -> Result<(std::net::TcpListener, u16)> {
    use rand::Rng as _;
    let mut rng = rand::rng();
    for _ in 0..50 {
        let port = rng.random_range(10_000..=u16::MAX);
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            listener.set_nonblocking(true)?;
            return Ok((listener, port));
        }
    }
    bail!("failed to bind a port in [10000, 65535] after 50 attempts");
}

/// Accept loop for the WS server. Runs until the surrounding task is dropped.
/// Single-client model: a new connection replaces any prior one.
pub async fn run(
    listener: TcpListener,
    auth_token: Arc<String>,
    outbound: OutboundSender,
    tool_call_tx: fut_mpsc::UnboundedSender<PendingToolCall>,
) -> Result<()> {
    loop {
        let (stream, peer_addr) = match listener.accept().await {
            Ok(value) => value,
            Err(error) => {
                log::warn!("claude-code-ide accept error: {error}");
                continue;
            }
        };
        log::debug!("claude-code-ide incoming connection from {peer_addr}");
        let auth_token = auth_token.clone();
        let outbound = outbound.clone();
        let tool_call_tx = tool_call_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, auth_token, outbound, tool_call_tx).await
            {
                log::warn!("claude-code-ide connection ended: {error}");
            }
        });
    }
}

async fn serve_connection(
    stream: TcpStream,
    auth_token: Arc<String>,
    outbound: OutboundSender,
    tool_call_tx: fut_mpsc::UnboundedSender<PendingToolCall>,
) -> Result<()> {
    let auth_for_callback = auth_token.clone();
    #[allow(clippy::result_large_err)]
    let callback = move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        match request.headers().get(AUTH_HEADER) {
            Some(value) if value.as_bytes() == auth_for_callback.as_bytes() => Ok(response),
            _ => {
                let mut resp = ErrorResponse::new(Some("unauthorized".into()));
                *resp.status_mut() = StatusCode::UNAUTHORIZED;
                Err(resp)
            }
        }
    };

    let ws = tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        .context("WebSocket upgrade failed")?;
    let (mut sink, mut source) = ws.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    {
        let mut slot = outbound.lock().await;
        if let Some(prior) = slot.take() {
            log::debug!("claude-code-ide replacing prior client connection");
            drop(prior);
        }
        *slot = Some(tx.clone());
    }

    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    while let Some(message) = source.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                log::debug!("claude-code-ide read error: {error}");
                break;
            }
        };
        match message {
            Message::Text(text) => {
                let tx = tx.clone();
                let tool_call_tx = tool_call_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_inbound(&text, &tx, &tool_call_tx).await {
                        log::debug!("claude-code-ide handler error: {error}");
                    }
                });
            }
            Message::Ping(payload) => {
                let _ = tx.send(Message::Pong(payload));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    {
        let mut slot = outbound.lock().await;
        if slot
            .as_ref()
            .map(|current| current.same_channel(&tx))
            .unwrap_or(false)
        {
            *slot = None;
        }
    }
    drop(tx);
    writer.await.ok();
    Ok(())
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn handle_inbound(
    text: &str,
    tx: &mpsc::UnboundedSender<Message>,
    tool_call_tx: &fut_mpsc::UnboundedSender<PendingToolCall>,
) -> Result<()> {
    let request: JsonRpcRequest = serde_json::from_str(text)
        .with_context(|| format!("parsing JSON-RPC frame: {text}"))?;

    if request.method == "tools/call" {
        let Some(id) = request.id.clone() else {
            log::debug!("claude-code-ide ignoring tools/call without id");
            return Ok(());
        };
        let response = match handle_tools_call(&request.params, tool_call_tx).await {
            Ok(result) => {
                let value = serde_json::to_value(&result)
                    .context("serialising ToolResult")?;
                ok_response(id, value)
            }
            Err(message) => error_response(id, -32602, message),
        };
        let json = serde_json::to_string(&response)?;
        tx.send(Message::Text(json))
            .map_err(|_| anyhow!("outbound channel closed"))?;
        return Ok(());
    }

    if let Some(response) = mcp_stubs::handle_request(&request) {
        let json = serde_json::to_string(&response)?;
        tx.send(Message::Text(json))
            .map_err(|_| anyhow!("outbound channel closed"))?;
    }
    Ok(())
}

async fn handle_tools_call(
    raw_params: &Value,
    tool_call_tx: &fut_mpsc::UnboundedSender<PendingToolCall>,
) -> Result<ToolResult, String> {
    let params: ToolCallParams = serde_json::from_value(raw_params.clone())
        .map_err(|error| format!("invalid tools/call params: {error}"))?;
    log::debug!(
        "claude-code-ide tools/call invoked: tool={} args={}",
        params.name,
        params.arguments
    );
    let (responder, response_rx) = oneshot::channel();
    if tool_call_tx
        .unbounded_send(PendingToolCall {
            tool_name: params.name,
            arguments: params.arguments,
            responder,
        })
        .is_err()
    {
        return Ok(ToolResult::error("server unavailable"));
    }
    match response_rx.await {
        Ok(result) => Ok(result),
        Err(_) => Ok(ToolResult::error("server dropped tool call")),
    }
}

/// Sends a `selection_changed` notification to the connected client, if any.
/// Drops the message if no client is connected.
pub async fn send_selection_changed(outbound: &OutboundSender, payload: SelectionPayload) {
    send_notification(outbound, "selection_changed", payload).await;
}

/// Sends an `at_mentioned` notification to the connected client, if any.
pub async fn send_at_mentioned(outbound: &OutboundSender, payload: AtMentionPayload) {
    send_notification(outbound, "at_mentioned", payload).await;
}

/// Sends a `diagnostics_changed` notification to the connected client, if any.
pub async fn send_diagnostics_changed(
    outbound: &OutboundSender,
    payload: DiagnosticsChangedPayload,
) {
    send_notification(outbound, "diagnostics_changed", payload).await;
}

async fn send_notification<T: serde::Serialize>(
    outbound: &OutboundSender,
    method: &'static str,
    payload: T,
) {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0",
        method,
        params: payload,
    };
    let json = match serde_json::to_string(&notification) {
        Ok(json) => json,
        Err(error) => {
            log::warn!("failed to encode {method} notification: {error}");
            return;
        }
    };
    let slot = outbound.lock().await;
    if let Some(tx) = slot.as_ref() {
        let _ = tx.send(Message::Text(json));
    }
}
