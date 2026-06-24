use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcMessage, JsonRpcRequest,
    JsonRpcResponse, LoadSessionResult, NewSessionParams, RpcId, RuntimeEvent, SessionView,
    RUNTIME_EVENT_METHOD,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::sleep;

struct AppState {
    daemon: Mutex<DaemonSupervisor>,
}

struct DaemonSupervisor {
    client: Option<DaemonClient>,
    last_error: Option<String>,
}

impl DaemonSupervisor {
    fn new() -> Self {
        Self {
            client: None,
            last_error: None,
        }
    }

    async fn ensure_client(&mut self, app_handle: AppHandle) -> Result<&mut DaemonClient, String> {
        if self.client.is_none() {
            match DaemonClient::spawn(app_handle).await {
                Ok(client) => {
                    self.client = Some(client);
                    self.last_error = None;
                }
                Err(error) => {
                    self.last_error = Some(error.clone());
                    return Err(error);
                }
            }
        }

        self.client.as_mut().ok_or_else(|| {
            self.last_error
                .clone()
                .unwrap_or_else(|| "daemon is not running".to_owned())
        })
    }

    async fn get_state(&mut self, app_handle: AppHandle) -> DaemonConnectionView {
        match self.ensure_client(app_handle).await {
            Ok(client) => match client.get_state().await {
                Ok(state) => {
                    self.last_error = None;
                    DaemonConnectionView::connected(state)
                }
                Err(error) => {
                    self.client = None;
                    self.last_error = Some(error.clone());
                    DaemonConnectionView::disconnected(error)
                }
            },
            Err(error) => DaemonConnectionView::disconnected(error),
        }
    }
}

struct DaemonClient {
    child: Child,
    socket_dir: PathBuf,
    writer: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<RpcId, oneshot::Sender<JsonRpcResponse>>>>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    next_request_id: u64,
}

impl DaemonClient {
    async fn spawn(app_handle: AppHandle) -> Result<Self, String> {
        spawn_daemon_client(app_handle).await
    }

    async fn get_state(&mut self) -> Result<DaemonState, String> {
        let response = self.request("get_state", None).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon get_state response did not include a result".to_owned())?;
        serde_json::from_value(result)
            .map_err(|error| format!("failed to decode daemon state: {error}"))
    }

    async fn new_session(
        &mut self,
        session_id: String,
        workspace: Option<String>,
    ) -> Result<(), String> {
        let params = serde_json::to_value(NewSessionParams {
            session_id,
            workspace,
        })
        .map_err(|error| format!("failed to encode new_session params: {error}"))?;
        self.request("new_session", Some(params)).await?;
        Ok(())
    }

    async fn load_session(&mut self, session_id: String) -> Result<SessionView, String> {
        let params = serde_json::to_value(byte_protocol::LoadSessionParams { session_id })
            .map_err(|error| format!("failed to encode load_session params: {error}"))?;
        let response = self.request("load_session", Some(params)).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon load_session response did not include a result".to_owned())?;
        serde_json::from_value::<LoadSessionResult>(result)
            .map(|result| result.session)
            .map_err(|error| format!("failed to decode session view: {error}"))
    }

    async fn request(
        &mut self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, String> {
        let request_id = self.next_id();
        let request = JsonRpcRequest::new(request_id.clone(), method, params);
        let request_line = encode_json_line(&request).map_err(|error| error.to_string())?;
        let (response_tx, response_rx) = oneshot::channel();

        self.pending
            .lock()
            .await
            .insert(request_id.clone(), response_tx);

        if self.writer.send(request_line).is_err() {
            self.pending.lock().await.remove(&request_id);
            return Err("daemon RPC writer is not running".to_owned());
        }

        let response = match tokio::time::timeout(Duration::from_secs(5), response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                return Err(format!(
                    "daemon response channel closed for request {request_id:?}"
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                return Err(format!(
                    "daemon did not respond to request {request_id:?} before timeout"
                ));
            }
        };

        if let Some(error) = response.error {
            return Err(format!(
                "daemon returned error {}: {}",
                error.code, error.message
            ));
        }

        Ok(response)
    }

    fn next_id(&mut self) -> RpcId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RpcId::Number(id)
    }
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.socket_dir);
    }
}

#[cfg(unix)]
async fn spawn_daemon_client(app_handle: AppHandle) -> Result<DaemonClient, String> {
    let daemon_path = resolve_daemon_path();
    let (socket_dir, socket_path) = create_rpc_socket_path()?;

    let mut command = Command::new(&daemon_path);
    command
        .arg("--rpc-socket")
        .arg(&socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to launch daemon at {daemon_path:?}: {error}"))?;

    let stream = match connect_daemon_socket(&socket_path).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = child.start_kill();
            let _ = std::fs::remove_dir_all(&socket_dir);
            return Err(error);
        }
    };

    let (read_half, mut write_half) = stream.into_split();
    let (writer, mut writer_rx) = mpsc::unbounded_channel::<String>();
    let pending = Arc::new(Mutex::new(
        HashMap::<RpcId, oneshot::Sender<JsonRpcResponse>>::new(),
    ));

    let writer_task = tokio::spawn(async move {
        while let Some(line) = writer_rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if write_half.flush().await.is_err() {
                break;
            }
        }
    });

    let reader_pending = Arc::clone(&pending);
    let reader_task = tokio::spawn(async move {
        let mut reader = BufReader::new(read_half).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) if line.trim().is_empty() => continue,
                Ok(Some(line)) => handle_daemon_message(&app_handle, &reader_pending, &line).await,
                Ok(None) => break,
                Err(error) => {
                    let _ = app_handle.emit(
                        "daemon-event",
                        RuntimeEvent::error(0, None, format!("failed to read daemon RPC frame: {error}")),
                    );
                    break;
                }
            }
        }
    });

    Ok(DaemonClient {
        child,
        socket_dir,
        writer,
        pending,
        reader_task,
        writer_task,
        next_request_id: 1,
    })
}

#[cfg(not(unix))]
async fn spawn_daemon_client(_app_handle: AppHandle) -> Result<DaemonClient, String> {
    Err("byte-daemon currently supports Unix Domain Socket RPC on Unix platforms only".to_owned())
}

async fn handle_daemon_message(
    app_handle: &AppHandle,
    pending: &Arc<Mutex<HashMap<RpcId, oneshot::Sender<JsonRpcResponse>>>>,
    line: &str,
) {
    match decode_json_line::<JsonRpcMessage>(line) {
        Ok(JsonRpcMessage::Response(response)) => {
            if let Some(response_tx) = pending.lock().await.remove(&response.id) {
                let _ = response_tx.send(response);
            }
        }
        Ok(JsonRpcMessage::Notification(notification))
            if notification.method == RUNTIME_EVENT_METHOD =>
        {
            if let Some(params) = notification.params {
                match serde_json::from_value::<RuntimeEvent>(params) {
                    Ok(event) => {
                        let _ = app_handle.emit("daemon-event", event);
                    }
                    Err(error) => {
                        let _ = app_handle.emit(
                            "daemon-event",
                            RuntimeEvent::error(
                                0,
                                None,
                                format!("failed to decode daemon runtime event: {error}"),
                            ),
                        );
                    }
                }
            }
        }
        Ok(JsonRpcMessage::Notification(_)) | Ok(JsonRpcMessage::Request(_)) => {}
        Err(error) => {
            let _ = app_handle.emit(
                "daemon-event",
                RuntimeEvent::error(0, None, format!("failed to decode daemon RPC frame: {error}")),
            );
        }
    }
}

#[cfg(unix)]
async fn connect_daemon_socket(socket_path: &Path) -> Result<UnixStream, String> {
    let mut last_error = None;

    for _ in 0..80 {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                sleep(Duration::from_millis(25)).await;
            }
        }
    }

    Err(format!(
        "failed to connect daemon RPC socket at {socket_path:?}: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "timed out".to_owned())
    ))
}

#[cfg(unix)]
fn create_rpc_socket_path() -> Result<(PathBuf, PathBuf), String> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let socket_dir =
        std::env::temp_dir().join(format!("byte-daemon-{}-{suffix}", std::process::id()));

    std::fs::create_dir(&socket_dir).map_err(|error| {
        format!("failed to create private daemon RPC directory {socket_dir:?}: {error}")
    })?;
    std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700)).map_err(
        |error| {
            let _ = std::fs::remove_dir_all(&socket_dir);
            format!("failed to secure daemon RPC directory {socket_dir:?}: {error}")
        },
    )?;

    let socket_path = socket_dir.join("rpc.sock");
    Ok((socket_dir, socket_path))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonConnectionView {
    connected: bool,
    state: Option<DaemonState>,
    error: Option<String>,
}

impl DaemonConnectionView {
    fn connected(state: DaemonState) -> Self {
        Self {
            connected: true,
            state: Some(state),
            error: None,
        }
    }

    fn disconnected(error: String) -> Self {
        Self {
            connected: false,
            state: None,
            error: Some(error),
        }
    }
}

#[tauri::command]
async fn get_daemon_state(
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<DaemonConnectionView, String> {
    Ok(state.daemon.lock().await.get_state(app_handle).await)
}

#[tauri::command]
async fn send_message(
    session_id: String,
    message: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut daemon = state.daemon.lock().await;
    let client = daemon.ensure_client(app_handle).await?;
    let params = serde_json::to_value(byte_protocol::SendMessageParams { session_id, message })
        .map_err(|error| error.to_string())?;
    client.request("send_message", Some(params)).await?;
    Ok(())
}

#[tauri::command]
async fn new_session(
    session_id: String,
    workspace: Option<String>,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut daemon = state.daemon.lock().await;
    let client = daemon.ensure_client(app_handle).await?;
    client.new_session(session_id, workspace).await
}

#[tauri::command]
async fn load_session(
    session_id: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<SessionView, String> {
    let mut daemon = state.daemon.lock().await;
    let client = daemon.ensure_client(app_handle).await?;
    client.load_session(session_id).await
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState {
                daemon: Mutex::new(DaemonSupervisor::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_daemon_state,
            send_message,
            new_session,
            load_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running Byte Agent desktop app");
}

fn resolve_daemon_path() -> PathBuf {
    if let Ok(path) = std::env::var("BYTE_DAEMON_PATH") {
        return PathBuf::from(path);
    }

    let executable_name = if cfg!(windows) {
        "byte-daemon.exe"
    } else {
        "byte-daemon"
    };

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let candidate = parent.join(executable_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(PathBuf::from);

    if let Some(workspace_root) = workspace_root {
        for profile in ["debug", "release"] {
            let candidate = workspace_root
                .join("target")
                .join(profile)
                .join(executable_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    PathBuf::from(executable_name)
}
