//! Byte daemon — a Unix-socket JSON-RPC server that hosts sessions, tools,
//! skills, and a model provider for the byte runtime.
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use anyhow::{Context, bail};
#[cfg(unix)]
use async_trait::async_trait;
use byte_core::event_bus::{BroadcastEventBus, RuntimeEventBus};
#[cfg(unix)]
use byte_core::runtime_services::RuntimeServices;
#[cfg(unix)]
use byte_core::session_manager::SessionManager;
#[cfg(unix)]
use byte_models::{ModelProvider, ProviderError, ProviderStream, create_provider, load_config};
#[cfg(unix)]
use byte_protocol::{
    JsonRpcRequest, JsonRpcResponse, RuntimeEventKind, decode_json_line, encode_json_line,
};
#[cfg(unix)]
use byte_session::SessionStore;
#[cfg(unix)]
use byte_skills::{MvpSkillRegistry, SkillRegistry};
#[cfg(unix)]
use byte_tools::{
    AllowAllPolicy, ApplyPatchTool, FindFilesTool, GrepTool, ListDirectoryTool, MvpToolRegistry,
    ReadFileTool, RunCommandTool, ToolRegistry, WriteFileTool,
};
#[cfg(unix)]
use futures::StreamExt;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::sync::{Mutex, mpsc};
#[cfg(unix)]
use tracing::{debug, error, info, trace, warn};

/// JSON-RPC request handlers shared by the socket server.
#[cfg(unix)]
mod rpc;
#[cfg(unix)]
use rpc::{RpcContext, handle_request};

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let socket_path = parse_rpc_socket_arg(std::env::args())?;
    info!(?socket_path, "starting byte daemon");
    run_socket_server(&socket_path).await
}

#[cfg(unix)]
/// Parse the `--rpc-socket <path>` argument from the command line.
fn parse_rpc_socket_arg(args: impl IntoIterator<Item = String>) -> anyhow::Result<PathBuf> {
    let mut args = args.into_iter();
    let _program = args.next();

    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--rpc-socket"), Some(path), None) => Ok(PathBuf::from(path)),
        _ => bail!("usage: byte-daemon --rpc-socket <path>"),
    }
}

#[cfg(unix)]
/// Bind the Unix socket and accept JSON-RPC connections until shutdown.
async fn run_socket_server(socket_path: &Path) -> anyhow::Result<()> {
    remove_stale_socket(socket_path)?;
    let _socket_file = SocketFile::new(socket_path.to_path_buf());
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind RPC socket at {}", socket_path.display()))?;

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
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept RPC socket client")?;
        info!("accepted new RPC connection");
        let event_bus = Arc::clone(&event_bus);
        let rpc_context = rpc_context.clone();
        let _handle = tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, event_bus, rpc_context).await {
                error!(%error, "RPC socket connection failed");
            }
        });
    }
}

#[cfg(unix)]
/// Handle a single client connection on the RPC socket.
async fn handle_connection(
    stream: UnixStream,
    event_bus: Arc<dyn RuntimeEventBus>,
    rpc_context: RpcContext,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<String>();

    let writer_task = tokio::spawn(async move {
        while let Some(line) = output_rx.recv().await {
            trace!(%line, "writing line to socket");
            write_half.write_all(line.as_bytes()).await?;
            write_half.flush().await?;
        }
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

    let mut reader = BufReader::new(read_half).lines();
    while let Some(line) = reader
        .next_line()
        .await
        .context("failed to read JSON-RPC frame from RPC socket")?
    {
        if line.trim().is_empty() {
            continue;
        }

        let request = match decode_json_line::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                warn!(%error, "failed to decode JSON-RPC request");
                let response = JsonRpcResponse::failure(0, -32700, format!("parse error: {error}"));
                let response_line =
                    encode_json_line(&response).context("failed to encode JSON-RPC response")?;
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
        let response_line =
            encode_json_line(&response).context("failed to encode JSON-RPC response")?;
        if output_tx.send(response_line).is_err() {
            break;
        }
    }

    drop(output_tx);
    event_task.abort();
    writer_task
        .await
        .context("RPC socket writer task failed to join")??;
    info!("RPC connection closed");

    Ok(())
}

/// A lazy provider that loads configuration on the first `send_message` call.
///
/// This preserves the original daemon behavior: `send_message` immediately
/// returns a run id, and any configuration error is reported through the
/// runtime event stream as a failed run.
#[cfg(unix)]
struct LazyConfigProvider {
    /// Lazily-initialized model provider, populated on the first `send_message`.
    inner: Mutex<Option<Arc<dyn ModelProvider>>>,
}

#[cfg(unix)]
impl LazyConfigProvider {
    /// Create a new, uninitialized lazy provider.
    fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
/// Build a concrete model provider from the on-disk configuration.
async fn build_provider() -> anyhow::Result<Arc<dyn ModelProvider>> {
    let config = load_config().await?;
    debug!(provider = %config.provider, model = %config.model, "loaded provider config");
    create_provider(config).map_err(anyhow::Error::from)
}

#[cfg(unix)]
/// Remove a leftover socket file from a previous daemon run, if present.
fn remove_stale_socket(socket_path: &Path) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path).with_context(|| {
            format!(
                "failed to remove stale RPC socket at {}",
                socket_path.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
/// RAII guard that deletes the Unix socket file when dropped.
struct SocketFile {
    /// Path to the bound Unix domain socket.
    path: PathBuf,
}

#[cfg(unix)]
impl SocketFile {
    /// Create a guard for the socket at `path`.
    const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[cfg(unix)]
impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;

    #[test]
    fn parses_rpc_socket_arg() {
        let socket_path = parse_rpc_socket_arg([
            "byte-daemon".to_owned(),
            "--rpc-socket".to_owned(),
            "/tmp/byte.sock".to_owned(),
        ])
        .expect("arg parses");

        assert_eq!(socket_path, PathBuf::from("/tmp/byte.sock"));
    }
}
