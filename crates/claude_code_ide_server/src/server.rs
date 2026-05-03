use crate::mcp_stubs;
use crate::protocol::{JsonRpcNotification, JsonRpcRequest, SelectionPayload};
use anyhow::{Context as _, Result, anyhow, bail};
use futures::{SinkExt as _, StreamExt as _};
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
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, auth_token, outbound).await {
                log::warn!("claude-code-ide connection ended: {error}");
            }
        });
    }
}

async fn serve_connection(
    stream: TcpStream,
    auth_token: Arc<String>,
    outbound: OutboundSender,
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
                if let Err(error) = handle_inbound(&text, &tx) {
                    log::debug!("claude-code-ide handler error: {error}");
                }
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

fn handle_inbound(text: &str, tx: &mpsc::UnboundedSender<Message>) -> Result<()> {
    let request: JsonRpcRequest = serde_json::from_str(text)
        .with_context(|| format!("parsing JSON-RPC frame: {text}"))?;
    if let Some(response) = mcp_stubs::handle_request(&request) {
        let json = serde_json::to_string(&response)?;
        tx.send(Message::Text(json))
            .map_err(|_| anyhow!("outbound channel closed"))?;
    }
    Ok(())
}

/// Sends a `selection_changed` notification to the connected client, if any.
/// Drops the message if no client is connected.
pub async fn send_selection_changed(outbound: &OutboundSender, payload: SelectionPayload) {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0",
        method: "selection_changed",
        params: payload,
    };
    let json = match serde_json::to_string(&notification) {
        Ok(json) => json,
        Err(error) => {
            log::warn!("failed to encode selection_changed: {error}");
            return;
        }
    };
    let slot = outbound.lock().await;
    if let Some(tx) = slot.as_ref() {
        let _ = tx.send(Message::Text(json));
    }
}
