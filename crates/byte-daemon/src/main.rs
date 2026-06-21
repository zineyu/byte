#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[cfg(unix)]
use anyhow::{bail, Context};
#[cfg(unix)]
use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RuntimeEvent,
};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::sync::{broadcast, mpsc};

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket_path = parse_rpc_socket_arg(std::env::args())?;
    run_socket_server(&socket_path).await
}

#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("byte-daemon currently supports Unix Domain Socket RPC on Unix platforms only")
}

#[cfg(unix)]
fn parse_rpc_socket_arg(args: impl IntoIterator<Item = String>) -> anyhow::Result<PathBuf> {
    let mut args = args.into_iter();
    let _program = args.next();

    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--rpc-socket"), Some(path), None) => Ok(PathBuf::from(path)),
        _ => bail!("usage: byte-daemon --rpc-socket <path>"),
    }
}

#[cfg(unix)]
async fn run_socket_server(socket_path: &Path) -> anyhow::Result<()> {
    remove_stale_socket(socket_path)?;
    let _socket_file = SocketFile::new(socket_path.to_path_buf());
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind RPC socket at {socket_path:?}"))?;
    let (event_tx, _) = broadcast::channel::<RuntimeEvent>(64);
    let event_sequence = Arc::new(AtomicU64::new(0));

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept RPC socket client")?;
        let event_tx = event_tx.clone();
        let event_sequence = Arc::clone(&event_sequence);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, event_tx, event_sequence).await {
                eprintln!("RPC socket connection failed: {error:#}");
            }
        });
    }
}

#[cfg(unix)]
async fn handle_connection(
    stream: UnixStream,
    event_tx: broadcast::Sender<RuntimeEvent>,
    event_sequence: Arc<AtomicU64>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<String>();

    let writer_task = tokio::spawn(async move {
        while let Some(line) = output_rx.recv().await {
            write_half.write_all(line.as_bytes()).await?;
            write_half.flush().await?;
        }
        anyhow::Ok(())
    });

    let event_output_tx = output_tx.clone();
    let mut event_rx = event_tx.subscribe();
    let event_task = tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let notification = JsonRpcNotification::runtime_event(event)?;
                    let line = encode_json_line(&notification)?;
                    if event_output_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        anyhow::Ok(())
    });

    let _ = event_tx.send(RuntimeEvent::daemon_started(
        next_sequence(&event_sequence),
        daemon_state(),
    ));

    let mut reader = BufReader::new(read_half).lines();
    while let Some(line) = reader
        .next_line()
        .await
        .context("failed to read JSON-RPC frame from RPC socket")?
    {
        if line.trim().is_empty() {
            continue;
        }

        let (response, event) = match decode_json_line::<JsonRpcRequest>(&line) {
            Ok(request) => handle_request(request, next_sequence(&event_sequence)),
            Err(error) => (
                JsonRpcResponse::failure(0, -32700, format!("parse error: {error}")),
                Some(RuntimeEvent::error(
                    next_sequence(&event_sequence),
                    format!("failed to parse JSON-RPC request: {error}"),
                )),
            ),
        };

        let response_line =
            encode_json_line(&response).context("failed to encode JSON-RPC response")?;
        if output_tx.send(response_line).is_err() {
            break;
        }

        if let Some(event) = event {
            let _ = event_tx.send(event);
        }
    }

    drop(output_tx);
    event_task.abort();
    writer_task
        .await
        .context("RPC socket writer task failed to join")??;

    Ok(())
}

#[cfg(unix)]
fn handle_request(
    request: JsonRpcRequest,
    sequence: u64,
) -> (JsonRpcResponse, Option<RuntimeEvent>) {
    match request.method.as_str() {
        "get_state" => {
            let state = daemon_state();
            let response = JsonRpcResponse::success(request.id, state.clone())
                .unwrap_or_else(|error| JsonRpcResponse::failure(0, -32603, error.to_string()));
            (response, Some(RuntimeEvent::state_changed(sequence, state)))
        }
        method => (
            JsonRpcResponse::failure(request.id, -32601, format!("method not found: {method}")),
            None,
        ),
    }
}

#[cfg(unix)]
fn daemon_state() -> DaemonState {
    DaemonState::ready(env!("CARGO_PKG_VERSION"))
}

#[cfg(unix)]
fn next_sequence(sequence: &AtomicU64) -> u64 {
    sequence.fetch_add(1, Ordering::SeqCst) + 1
}

#[cfg(unix)]
fn remove_stale_socket(socket_path: &Path) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale RPC socket at {socket_path:?}"))?;
    }
    Ok(())
}

#[cfg(unix)]
struct SocketFile {
    path: PathBuf,
}

#[cfg(unix)]
impl SocketFile {
    fn new(path: PathBuf) -> Self {
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
