#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use anyhow::{bail, Context};
#[cfg(unix)]
use byte_models::{
    load_config, normalize_base_url, EchoProvider, ModelProvider, OpenAiCompatibleProvider,
    ProviderEvent,
};
#[cfg(unix)]
use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LoadSessionParams, LoadSessionResult, MessageRole, NewSessionParams,
    NewSessionResult, RunMessage, RunStatus, RuntimeEvent, SendMessageParams, SendMessageResult,
};
#[cfg(unix)]
use byte_session::SessionStore;
#[cfg(unix)]
use futures::StreamExt;
#[cfg(unix)]
use serde::de::DeserializeOwned;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::sync::{broadcast, mpsc, Mutex};

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
    let active_runs = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let session_store =
        Arc::new(SessionStore::with_default_dir().context("failed to initialize session store")?);

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept RPC socket client")?;
        let event_tx = event_tx.clone();
        let event_sequence = Arc::clone(&event_sequence);
        let active_runs = Arc::clone(&active_runs);
        let session_store = Arc::clone(&session_store);
        tokio::spawn(async move {
            if let Err(error) =
                handle_connection(stream, event_tx, event_sequence, active_runs, session_store)
                    .await
            {
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
    active_runs: Arc<Mutex<HashMap<String, String>>>,
    session_store: Arc<SessionStore>,
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

        let request = match decode_json_line::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                let response = JsonRpcResponse::failure(0, -32700, format!("parse error: {error}"));
                let response_line =
                    encode_json_line(&response).context("failed to encode JSON-RPC response")?;
                if output_tx.send(response_line).is_err() {
                    break;
                }
                let _ = event_tx.send(RuntimeEvent::error(
                    next_sequence(&event_sequence),
                    None,
                    format!("failed to parse JSON-RPC request: {error}"),
                ));
                continue;
            }
        };

        let response = if request.method == "send_message" {
            handle_send_message(
                &request,
                &event_tx,
                Arc::clone(&event_sequence),
                &active_runs,
                Arc::clone(&session_store),
            )
            .await
        } else if request.method == "new_session" || request.method == "load_session" {
            handle_session_request(&request, &session_store).await
        } else {
            handle_request(request)
        };
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

    Ok(())
}

#[cfg(unix)]
fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    match request.method.as_str() {
        "get_state" => {
            let state = daemon_state();
            JsonRpcResponse::success(request.id, state.clone())
                .unwrap_or_else(|error| JsonRpcResponse::failure(0, -32603, error.to_string()))
        }
        method => {
            JsonRpcResponse::failure(request.id, -32601, format!("method not found: {method}"))
        }
    }
}

#[cfg(unix)]
async fn handle_session_request(
    request: &JsonRpcRequest,
    session_store: &SessionStore,
) -> JsonRpcResponse {
    match request.method.as_str() {
        "new_session" => {
            let params: NewSessionParams = match parse_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };

            if let Err(error) = session_store
                .new_session(&params.session_id, params.workspace.as_deref())
                .await
            {
                return JsonRpcResponse::failure(
                    request.id.clone(),
                    -32603,
                    format!("failed to create session: {error}"),
                );
            }

            let result = NewSessionResult {
                session_id: params.session_id,
            };
            JsonRpcResponse::success(request.id.clone(), result)
                .unwrap_or_else(|error| JsonRpcResponse::failure(0, -32603, error.to_string()))
        }
        "load_session" => {
            let params: LoadSessionParams = match parse_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };

            match session_store.load_session(&params.session_id).await {
                Ok(session) => {
                    let result = LoadSessionResult { session };
                    JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                        JsonRpcResponse::failure(0, -32603, error.to_string())
                    })
                }
                Err(error) => JsonRpcResponse::failure(
                    request.id.clone(),
                    -32603,
                    format!("failed to load session: {error}"),
                ),
            }
        }
        _ => JsonRpcResponse::failure(
            request.id.clone(),
            -32601,
            format!("method not found: {}", request.method),
        ),
    }
}

#[cfg(unix)]
#[allow(clippy::result_large_err)] // JsonRpcResponse is large; this helper is internal and short-lived.
fn parse_params<T: DeserializeOwned>(request: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .ok_or_else(|| {
            JsonRpcResponse::failure(
                request.id.clone(),
                -32602,
                format!("invalid params for {}", request.method),
            )
        })
}

async fn handle_send_message(
    request: &JsonRpcRequest,
    event_tx: &broadcast::Sender<RuntimeEvent>,
    event_sequence: Arc<AtomicU64>,
    active_runs: &Arc<Mutex<HashMap<String, String>>>,
    session_store: Arc<SessionStore>,
) -> JsonRpcResponse {
    let params: SendMessageParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    let run_id = uuid::Uuid::new_v4().to_string();
    let session_id = params.session_id.clone();

    {
        let mut active = active_runs.lock().await;
        if active.contains_key(&session_id) {
            return JsonRpcResponse::failure(
                request.id.clone(),
                -32000,
                format!("session {session_id} already has an active run"),
            );
        }
        active.insert(session_id.clone(), run_id.clone());
    }

    if let Err(error) = session_store.new_session(&session_id, None).await {
        active_runs.lock().await.remove(&session_id);
        return JsonRpcResponse::failure(
            request.id.clone(),
            -32603,
            format!("failed to prepare session: {error}"),
        );
    }

    let parent_id = match session_store.load_session(&session_id).await {
        Ok(view) => view.messages.last().map(|message| message.id.clone()),
        Err(error) => {
            active_runs.lock().await.remove(&session_id);
            return JsonRpcResponse::failure(
                request.id.clone(),
                -32603,
                format!("failed to load session state: {error}"),
            );
        }
    };

    let developer_message_id = match session_store
        .append_message(
            &session_id,
            None,
            parent_id.as_deref(),
            MessageRole::Developer,
            &params.message,
        )
        .await
    {
        Ok(id) => id,
        Err(error) => {
            active_runs.lock().await.remove(&session_id);
            return JsonRpcResponse::failure(
                request.id.clone(),
                -32603,
                format!("failed to append developer message: {error}"),
            );
        }
    };

    let result = SendMessageResult {
        run_id: run_id.clone(),
    };
    let response = JsonRpcResponse::success(request.id.clone(), result.clone())
        .unwrap_or_else(|error| JsonRpcResponse::failure(0, -32603, error.to_string()));

    let run_id_for_task = run_id.clone();
    let session_id_for_task = session_id.clone();
    let event_tx_for_task = event_tx.clone();
    let event_sequence_for_task = Arc::clone(&event_sequence);
    let active_runs_for_task = Arc::clone(active_runs);
    let session_store_for_task = Arc::clone(&session_store);

    tokio::spawn(async move {
        run_model(
            run_id_for_task,
            session_id_for_task,
            params.message,
            developer_message_id,
            event_tx_for_task,
            event_sequence_for_task,
            active_runs_for_task,
            session_store_for_task,
        )
        .await;
    });

    response
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)] // run_model bundles the full run context; grouping would not simplify callers.
async fn run_model(
    run_id: String,
    session_id: String,
    message: String,
    developer_message_id: String,
    event_tx: broadcast::Sender<RuntimeEvent>,
    event_sequence: Arc<AtomicU64>,
    active_runs: Arc<Mutex<HashMap<String, String>>>,
    session_store: Arc<SessionStore>,
) {
    let session_id_for_cleanup = session_id.clone();
    let active_runs_for_cleanup = Arc::clone(&active_runs);
    let _cleanup_guard = scopeguard::guard(session_id_for_cleanup, move |session_id| {
        active_runs_for_cleanup.blocking_lock().remove(&session_id);
    });

    let _ = event_tx.send(RuntimeEvent::run_started(
        next_sequence(&event_sequence),
        session_id.clone(),
        run_id.clone(),
    ));
    let config = match load_config().await {
        Ok(config) => config,
        Err(error) => {
            emit_run_error(
                &event_tx,
                &event_sequence,
                &active_runs,
                &session_id,
                &run_id,
                error.to_string(),
            )
            .await;
            return;
        }
    };

    let messages = vec![RunMessage {
        role: MessageRole::Developer,
        content: message,
    }];

    let provider: Box<dyn ModelProvider> = match config.provider.as_str() {
        "openai" | "openai-compatible" => Box::new(OpenAiCompatibleProvider::new(
            byte_models::ModelProviderConfig {
                provider: config.provider,
                base_url: normalize_base_url(&config.base_url),
                api_key: config.api_key,
                model: config.model,
            },
        )),
        "echo" => Box::new(EchoProvider::default()),
        other => {
            emit_run_error(
                &event_tx,
                &event_sequence,
                &active_runs,
                &session_id,
                &run_id,
                format!("unknown provider: {other}"),
            )
            .await;
            return;
        }
    };

    let mut stream = match provider.send_message(messages).await {
        Ok(stream) => stream,
        Err(error) => {
            emit_run_error(
                &event_tx,
                &event_sequence,
                &active_runs,
                &session_id,
                &run_id,
                error.to_string(),
            )
            .await;
            return;
        }
    };

    let mut message_id: Option<String> = None;
    let mut assistant_content = String::new();
    let mut delta_buffer = String::new();
    let mut last_flush = tokio::time::Instant::now();
    const DELTA_BATCH_INTERVAL: Duration = Duration::from_millis(16);
    const DELTA_BATCH_MAX_LEN: usize = 64;

    while let Some(event) = stream.next().await {
        match event {
            Ok(ProviderEvent::MessageStarted { message_id: id }) => {
                message_id = Some(id.clone());
                assistant_content.clear();
                delta_buffer.clear();
                last_flush = tokio::time::Instant::now();
                let _ = event_tx.send(RuntimeEvent::message_started(
                    next_sequence(&event_sequence),
                    run_id.clone(),
                    id,
                    byte_protocol::MessageRole::Assistant,
                ));
            }
            Ok(ProviderEvent::TextDelta {
                message_id: id,
                delta,
            }) => {
                if message_id.as_ref() == Some(&id) {
                    assistant_content.push_str(&delta);
                    delta_buffer.push_str(&delta);
                    let should_flush = delta_buffer.len() >= DELTA_BATCH_MAX_LEN
                        || last_flush.elapsed() >= DELTA_BATCH_INTERVAL;
                    if should_flush {
                        let _ = event_tx.send(RuntimeEvent::message_delta(
                            next_sequence(&event_sequence),
                            run_id.clone(),
                            id,
                            std::mem::take(&mut delta_buffer),
                        ));
                        last_flush = tokio::time::Instant::now();
                    }
                }
            }
            Ok(ProviderEvent::MessageCompleted { message_id: id }) => {
                if message_id.as_ref() == Some(&id) {
                    if !delta_buffer.is_empty() {
                        let _ = event_tx.send(RuntimeEvent::message_delta(
                            next_sequence(&event_sequence),
                            run_id.clone(),
                            id.clone(),
                            std::mem::take(&mut delta_buffer),
                        ));
                    }
                    let _ = event_tx.send(RuntimeEvent::message_completed(
                        next_sequence(&event_sequence),
                        run_id.clone(),
                        id.clone(),
                    ));
                    if let Err(error) = session_store
                        .append_message(
                            &session_id,
                            Some(&id),
                            Some(&developer_message_id),
                            MessageRole::Assistant,
                            &assistant_content,
                        )
                        .await
                    {
                        emit_run_error(
                            &event_tx,
                            &event_sequence,
                            &active_runs,
                            &session_id,
                            &run_id,
                            format!("failed to append assistant message: {error}"),
                        )
                        .await;
                        return;
                    }
                }
            }
            Err(error) => {
                if !delta_buffer.is_empty() {
                    if let Some(id) = message_id.clone() {
                        let _ = event_tx.send(RuntimeEvent::message_delta(
                            next_sequence(&event_sequence),
                            run_id.clone(),
                            id,
                            std::mem::take(&mut delta_buffer),
                        ));
                    }
                }
                emit_run_error(
                    &event_tx,
                    &event_sequence,
                    &active_runs,
                    &session_id,
                    &run_id,
                    error.to_string(),
                )
                .await;
                return;
            }
        }
    }

    if !delta_buffer.is_empty() {
        if let Some(id) = message_id.clone() {
            let _ = event_tx.send(RuntimeEvent::message_delta(
                next_sequence(&event_sequence),
                run_id.clone(),
                id,
                std::mem::take(&mut delta_buffer),
            ));
        }
    }

    let _ = event_tx.send(RuntimeEvent::run_finished(
        next_sequence(&event_sequence),
        run_id.clone(),
        RunStatus::Succeeded,
        None,
    ));

    active_runs.lock().await.remove(&session_id);
}

#[cfg(unix)]
async fn emit_run_error(
    event_tx: &broadcast::Sender<RuntimeEvent>,
    event_sequence: &AtomicU64,
    active_runs: &Mutex<HashMap<String, String>>,
    session_id: &str,
    run_id: &str,
    message: String,
) {
    let _ = event_tx.send(RuntimeEvent::error(
        next_sequence(event_sequence),
        Some(run_id.to_owned()),
        message.clone(),
    ));
    let _ = event_tx.send(RuntimeEvent::run_finished(
        next_sequence(event_sequence),
        run_id.to_owned(),
        RunStatus::Failed,
        Some(message),
    ));
    active_runs.lock().await.remove(session_id);
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
