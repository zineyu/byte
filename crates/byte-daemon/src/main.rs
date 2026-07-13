//! Byte daemon — a WebSocket JSON-RPC server that hosts sessions, tools,
//! skills, and a model provider for the byte runtime.
#![deny(rustdoc::broken_intra_doc_links)]

use std::sync::Arc;

use anyhow::{Context, bail};
use async_trait::async_trait;
use byte_core::event_bus::{BroadcastEventBus, RuntimeEventBus};
use byte_core::runtime_services::RuntimeServices;
use byte_core::session_manager::SessionManager;
use byte_models::{ModelProvider, ProviderError, ProviderStream, create_provider, load_config};
use byte_protocol::{
    DaemonAddress, JsonRpcRequest, JsonRpcResponse, RuntimeEventKind, decode_json_line,
    encode_json_line,
};
use byte_session::SessionStore;
use byte_skills::{MvpSkillRegistry, SkillRegistry};
use byte_tools::{
    AllowAllPolicy, ApplyPatchTool, FindFilesTool, GrepTool, ListDirectoryTool, MvpToolRegistry,
    ReadFileTool, RunCommandTool, ToolRegistry, WriteFileTool,
};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, trace, warn};

/// JSON-RPC request handlers shared by the WebSocket server.
mod rpc;
use rpc::{RpcContext, handle_request};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let address = parse_rpc_websocket_arg(std::env::args())?;
    info!(%address, "starting byte daemon");
    run_websocket_server(&address).await
}

/// Parse the `--rpc-websocket <addr>` argument from the command line.
fn parse_rpc_websocket_arg(args: impl IntoIterator<Item = String>) -> anyhow::Result<DaemonAddress> {
    let mut args = args.into_iter();
    let _program = args.next();

    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--rpc-websocket"), Some(addr), None) => addr.parse().map_err(|error| {
            anyhow::anyhow!("invalid --rpc-websocket address '{addr}': {error}")
        }),
        _ => bail!("usage: byte-daemon --rpc-websocket <addr>"),
    }
}

/// Bind the WebSocket address and accept JSON-RPC connections until shutdown.
async fn run_websocket_server(address: &DaemonAddress) -> anyhow::Result<()> {
    let socket_addr = address.socket_addr();
    let listener = TcpListener::bind(socket_addr)
        .await
        .with_context(|| format!("failed to bind RPC WebSocket at {socket_addr}"))?;

    let event_bus: Arc<dyn RuntimeEventBus> = Arc::new(BroadcastEventBus::new());
    let session_store =
        Arc::new(SessionStore::with_default_dir().context("failed to initialize session store")?);
    let provider: Arc<dyn ModelProvider> = Arc::new(LazyConfigProvider::new());

    let mut registry = MvpToolRegistry::new();
    registry.register(
        "read_file".to_string(),
        Arc::new(ReadFileTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "write_file".to_string(),
        Arc::new(WriteFileTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "apply_patch".to_string(),
        Arc::new(ApplyPatchTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "list_directory".to_string(),
        Arc::new(ListDirectoryTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "grep".to_string(),
        Arc::new(GrepTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "find_files".to_string(),
        Arc::new(FindFilesTool),
        Arc::new(AllowAllPolicy),
    );
    registry.register(
        "run_command".to_string(),
        Arc::new(RunCommandTool),
        Arc::new(AllowAllPolicy),
    );
    let tool_registry: Arc<dyn ToolRegistry> = Arc::new(registry);
    let skill_registry: Arc<dyn SkillRegistry> = Arc::new(MvpSkillRegistry::new());

    let services = RuntimeServices::new(
        Arc::clone(&provider),
        Arc::clone(&session_store),
        Arc::clone(&event_bus),
        tool_registry,
        skill_registry,
    );
    let session_manager = SessionManager::new(services);
    let rpc_context = RpcContext { session_manager };

    info!("daemon ready, waiting for connections");

    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept RPC WebSocket client")?;
        info!(?peer, "accepted new RPC connection");
        let event_bus = Arc::clone(&event_bus);
        let rpc_context = rpc_context.clone();
        let _handle = tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, event_bus, rpc_context).await {
                error!(%error, "RPC WebSocket connection failed");
            }
        });
    }
}

/// Handle a single client connection on the WebSocket.
async fn handle_connection(
    stream: tokio::net::TcpStream,
    event_bus: Arc<dyn RuntimeEventBus>,
    rpc_context: RpcContext,
) -> anyhow::Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .context("failed to accept WebSocket handshake")?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<String>();

    let writer_task = tokio::spawn(async move {
        while let Some(line) = output_rx.recv().await {
            trace!(%line, "writing line to websocket");
            if ws_tx
                .send(WsMessage::Text(line.into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = ws_tx.close().await;
        anyhow::Ok(())
    });

    let event_output_tx = output_tx.clone();
    let mut event_stream = event_bus.subscribe_json_lines();
    let event_task = tokio::spawn(async move {
        while let Some(line) = event_stream.next().await {
            if event_output_tx.send(line).is_err() {
                break;
            }
        }
        anyhow::Ok(())
    });

    event_bus
        .emit(RuntimeEventKind::daemon_started(rpc::daemon_state()))
        .await;

    while let Some(message) = ws_rx.next().await {
        match message {
            Ok(WsMessage::Text(text)) => {
                let line = text.to_string();
                if line.trim().is_empty() {
                    continue;
                }

                let request = match decode_json_line::<JsonRpcRequest>(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        warn!(%error, "failed to decode JSON-RPC request");
                        let response =
                            JsonRpcResponse::failure(0, -32700, format!("parse error: {error}"));
                        let response_line = encode_json_line(&response)
                            .context("failed to encode JSON-RPC response")?;
                        if output_tx.send(response_line).is_err() {
                            break;
                        }
                        event_bus
                            .emit(RuntimeEventKind::error(
                                None,
                                format!("failed to parse JSON-RPC request: {error}"),
                            ))
                            .await;
                        continue;
                    }
                };

                debug!(method = %request.method, id = ?request.id, "received JSON-RPC request");

                let response = handle_request(&rpc_context, request).await;
                let response_line = encode_json_line(&response)
                    .context("failed to encode JSON-RPC response")?;
                if output_tx.send(response_line).is_err() {
                    break;
                }
            }
            Ok(WsMessage::Close(_)) | Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => {}
            Ok(WsMessage::Frame(_)) | Ok(WsMessage::Binary(_)) => {
                warn!("received unsupported WebSocket message type");
            }
            Err(error) => {
                warn!(%error, "WebSocket error");
                break;
            }
        }
    }

    drop(output_tx);
    event_task.abort();
    writer_task
        .await
        .context("RPC WebSocket writer task failed to join")??;
    info!("RPC WebSocket connection closed");

    Ok(())
}

/// A lazy provider that loads configuration on the first `send_message` call.
///
/// This preserves the original daemon behavior: `send_message` immediately
/// returns a run id, and any configuration error is reported through the
/// runtime event stream as a failed run.
struct LazyConfigProvider {
    /// Lazily-initialized model provider, populated on the first `send_message`.
    inner: Mutex<Option<Arc<dyn ModelProvider>>>,
}

impl LazyConfigProvider {
    /// Create a new, uninitialized lazy provider.
    fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ModelProvider for LazyConfigProvider {
    async fn send_message(
        &self,
        messages: Vec<byte_protocol::LlmMessage>,
        tools: Vec<byte_protocol::ToolDefinition>,
    ) -> Result<ProviderStream, ProviderError> {
        let provider = {
            let mut guard = self.inner.lock().await;
            if let Some(provider) = guard.as_ref() {
                provider.clone()
            } else {
                let initialized = build_provider()
                    .await
                    .map_err(|error| ProviderError::Configuration(format!("{error:#}")))?;
                let provider = initialized.clone();
                let _ = guard.replace(initialized);
                provider
            }
        };
        provider.send_message(messages, tools).await
    }
}

/// Build a concrete model provider from the on-disk configuration.
async fn build_provider() -> anyhow::Result<Arc<dyn ModelProvider>> {
    let config = load_config().await?;
    debug!(provider = %config.provider, model = %config.model, "loaded provider config");
    create_provider(config).map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;

    #[test]
    fn parses_rpc_websocket_arg() {
        let address = parse_rpc_websocket_arg([
            "byte-daemon".to_owned(),
            "--rpc-websocket".to_owned(),
            "127.0.0.1:8787".to_owned(),
        ])
        .expect("arg parses");

        assert_eq!(address, "127.0.0.1:8787".parse().unwrap());
    }

    #[test]
    fn rejects_non_local_websocket_arg() {
        let result = parse_rpc_websocket_arg([
            "byte-daemon".to_owned(),
            "--rpc-websocket".to_owned(),
            "192.168.1.1:8787".to_owned(),
        ]);
        assert!(result.is_err());
    }
}
