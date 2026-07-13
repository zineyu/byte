//! Byte Agent Tauri desktop application.
//!
//! Connects to a manually-started local daemon over WebSocket JSON-RPC and
//! exposes commands to the React frontend.
#![deny(rustdoc::broken_intra_doc_links)]
#![allow(clippy::unreachable)]

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonAddress, DaemonConnectionView, DaemonState,
    DeleteSessionParams, DeleteSessionResult, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse,
    ListSessionsResult, LoadSessionResult, NewSessionParams, NewSessionResult, RpcId, RuntimeEvent,
    RuntimeEventKind, SessionSummary, SessionView, RUNTIME_EVENT_METHOD,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::warn;

/// Shared application state managed by Tauri.
struct AppState {
    /// Supervisor that maintains the daemon connection and reconnects on demand.
    daemon: Mutex<DaemonSupervisor>,
}

/// Lifecycle manager for the daemon WebSocket connection.
struct DaemonSupervisor {
    /// Active daemon client, if connected.
    client: Option<DaemonClient>,
    /// Most recent error encountered when connecting to the daemon.
    last_error: Option<String>,
    /// Persisted daemon address, if any.
    address: Option<DaemonAddress>,
}

impl DaemonSupervisor {
    /// Creates a new supervisor with no connection and loads the saved address.
    fn new() -> Self {
        Self {
            client: None,
            last_error: None,
            address: load_daemon_address().ok().flatten(),
        }
    }

    /// Ensures a daemon client is connected, returning a mutable reference to it.
    async fn ensure_client(&mut self, app_handle: AppHandle) -> Result<&mut DaemonClient, String> {
        if self.client.is_none() {
            let address = self.address.ok_or_else(|| {
                "未配置 daemon 地址。请在设置中输入本地 daemon WebSocket 地址，例如 127.0.0.1:8787。"
                    .to_owned()
            })?;

            match DaemonClient::connect(address, app_handle).await {
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
                .unwrap_or_else(|| "daemon 未连接".to_owned())
        })
    }

    /// Returns the current daemon connection state, connecting if a saved address exists.
    async fn get_state(&mut self, app_handle: AppHandle) -> Result<DaemonConnectionView, String> {
        match self.ensure_client(app_handle).await {
            Ok(client) => match client.get_state().await {
                Ok(state) => {
                    self.last_error = None;
                    Ok(DaemonConnectionView::connected(state))
                }
                Err(error) => {
                    self.client = None;
                    self.last_error = Some(error.clone());
                    Ok(DaemonConnectionView::disconnected(error))
                }
            },
            Err(error) => Ok(DaemonConnectionView::disconnected(error)),
        }
    }

    /// Sets the daemon address, validates it, persists it, and connects.
    async fn set_address(
        &mut self,
        app_handle: AppHandle,
        address: DaemonAddress,
    ) -> Result<DaemonConnectionView, String> {
        self.address = Some(address);
        if let Err(error) = save_daemon_address(address) {
            return Err(format!("保存 daemon 地址失败: {error}"));
        }
        self.client = None;
        self.get_state(app_handle).await
    }
}

/// Connection to a running daemon over a WebSocket.
struct DaemonClient {
    /// WebSocket writer channel for sending JSON-RPC request frames.
    writer: mpsc::UnboundedSender<String>,
    /// Outstanding RPC requests awaiting responses.
    pending: Arc<Mutex<HashMap<RpcId, oneshot::Sender<JsonRpcResponse>>>>,
    /// Background task that reads frames from the daemon WebSocket.
    reader_task: JoinHandle<()>,
    /// Background task that writes frames to the daemon WebSocket.
    writer_task: JoinHandle<()>,
    /// Monotonically increasing counter for JSON-RPC request ids.
    next_request_id: u64,
}

impl DaemonClient {
    /// Connects to the daemon at the given WebSocket address.
    async fn connect(address: DaemonAddress, app_handle: AppHandle) -> Result<Self, String> {
        let url = address.websocket_url();
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|error| format!("无法连接到 daemon {url}: {error}"))?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();
        let (writer, mut writer_rx) = mpsc::unbounded_channel::<String>();
        let pending = Arc::new(Mutex::new(
            HashMap::<RpcId, oneshot::Sender<JsonRpcResponse>>::new(),
        ));

        let writer_task = tokio::spawn(async move {
            while let Some(line) = writer_rx.recv().await {
                if ws_tx.send(WsMessage::Text(line.into())).await.is_err() {
                    break;
                }
            }
            let _ = ws_tx.close().await;
        });

        let reader_app_handle = app_handle.clone();
        let reader_pending = Arc::clone(&pending);
        let reader_task = tokio::spawn(async move {
            loop {
                match ws_rx.next().await {
                    Some(Ok(WsMessage::Text(text))) => {
                        handle_daemon_message(&reader_app_handle, &reader_pending, text.as_str())
                            .await;
                    }
                    Some(Ok(WsMessage::Close(_) | WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                    Some(Ok(WsMessage::Binary(_) | WsMessage::Frame(_))) => {
                        warn!("received unsupported WebSocket message type from daemon");
                    }
                    Some(Err(error)) => {
                        let _ = reader_app_handle.emit(
                            "daemon-event",
                            RuntimeEvent {
                                sequence: 0,
                                kind: RuntimeEventKind::error(
                                    None,
                                    format!("daemon WebSocket error: {error}"),
                                ),
                            },
                        );
                        break;
                    }
                    None => break,
                }
            }
        });

        Ok(Self {
            writer,
            pending,
            reader_task,
            writer_task,
            next_request_id: 1,
        })
    }

    /// Fetches the daemon's current runtime state.
    async fn get_state(&mut self) -> Result<DaemonState, String> {
        let response = self.request("get_state", None).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon get_state 响应未包含 result".to_owned())?;
        serde_json::from_value(result).map_err(|error| format!("无法解析 daemon 状态: {error}"))
    }

    /// Creates a new session in `workspace` and returns its generated session id.
    async fn new_session(&mut self, workspace: String) -> Result<String, String> {
        let params = serde_json::to_value(NewSessionParams { workspace })
            .map_err(|error| format!("无法编码 new_session 参数: {error}"))?;
        let response = self.request("new_session", Some(params)).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon new_session 响应未包含 result".to_owned())?;
        serde_json::from_value::<NewSessionResult>(result)
            .map(|result| result.session_id)
            .map_err(|error| format!("无法解析 new_session 结果: {error}"))
    }

    /// Lists all sessions known to the daemon.
    async fn list_sessions(&mut self) -> Result<Vec<SessionSummary>, String> {
        let response = self.request("list_sessions", None).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon list_sessions 响应未包含 result".to_owned())?;
        serde_json::from_value::<ListSessionsResult>(result)
            .map(|result| result.sessions)
            .map_err(|error| format!("无法解析会话列表: {error}"))
    }

    /// Deletes the session with the given id.
    async fn delete_session(&mut self, session_id: String) -> Result<String, String> {
        let params = serde_json::to_value(DeleteSessionParams { session_id })
            .map_err(|error| format!("无法编码 delete_session 参数: {error}"))?;
        let response = self.request("delete_session", Some(params)).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon delete_session 响应未包含 result".to_owned())?;
        serde_json::from_value::<DeleteSessionResult>(result)
            .map(|result| result.session_id)
            .map_err(|error| format!("无法解析 delete_session 结果: {error}"))
    }

    /// Loads the full view for the session with the given id.
    async fn load_session(&mut self, session_id: String) -> Result<SessionView, String> {
        let params = serde_json::to_value(byte_protocol::LoadSessionParams { session_id })
            .map_err(|error| format!("无法编码 load_session 参数: {error}"))?;
        let response = self.request("load_session", Some(params)).await?;
        let result = response
            .result
            .ok_or_else(|| "daemon load_session 响应未包含 result".to_owned())?;
        serde_json::from_value::<LoadSessionResult>(result)
            .map(|result| result.session)
            .map_err(|error| format!("无法解析 session view: {error}"))
    }

    /// Sends a JSON-RPC request to the daemon and waits for its response.
    async fn request(
        &mut self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, String> {
        let request_id = self.next_id();
        let request = JsonRpcRequest::new(request_id.clone(), method, params);
        let request_line = encode_json_line(&request).map_err(|error| error.to_string())?;
        let (response_tx, response_rx) = oneshot::channel();

        let _ = self
            .pending
            .lock()
            .await
            .insert(request_id.clone(), response_tx);

        if self.writer.send(request_line).is_err() {
            let _ = self.pending.lock().await.remove(&request_id);
            return Err("daemon WebSocket 写入端已关闭".to_owned());
        }

        let response = match tokio::time::timeout(Duration::from_secs(5), response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                return Err(format!("daemon 响应通道已关闭，请求 id: {request_id:?}"));
            }
            Err(_) => {
                let _ = self.pending.lock().await.remove(&request_id);
                return Err(format!("daemon 未在超时时间内响应请求 {request_id:?}"));
            }
        };

        if let Some(error) = response.error {
            return Err(format!("daemon 返回错误 {}: {}", error.code, error.message));
        }

        Ok(response)
    }

    /// Generates the next JSON-RPC request id.
    const fn next_id(&mut self) -> RpcId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RpcId::Number(id)
    }
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

/// Decodes a frame from the daemon and routes it to pending requests or frontend events.
async fn handle_daemon_message(
    app_handle: &AppHandle,
    pending: &Arc<Mutex<HashMap<RpcId, oneshot::Sender<JsonRpcResponse>>>>,
    text: &str,
) {
    match decode_json_line::<JsonRpcMessage>(text) {
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
                            RuntimeEvent {
                                sequence: 0,
                                kind: RuntimeEventKind::error(
                                    None,
                                    format!("无法解析 daemon runtime event: {error}"),
                                ),
                            },
                        );
                    }
                }
            }
        }
        Ok(JsonRpcMessage::Notification(_) | JsonRpcMessage::Request(_)) => {}
        Err(error) => {
            let _ = app_handle.emit(
                "daemon-event",
                RuntimeEvent {
                    sequence: 0,
                    kind: RuntimeEventKind::error(None, format!("无法解析 daemon RPC 帧: {error}")),
                },
            );
        }
    }
}

/// Persisted daemon connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonConfigFile {
    /// Daemon WebSocket address.
    address: DaemonAddress,
}

/// Resolve the path to `~/.config/byte/daemon.toml`.
fn resolve_daemon_config_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME").map_or_else(
        |_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".config")
        },
        PathBuf::from,
    );
    config_dir.join("byte").join("daemon.toml")
}

/// Load the saved daemon address, if any.
fn load_daemon_address() -> Result<Option<DaemonAddress>, String> {
    let path = resolve_daemon_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取 daemon 配置失败 {}: {error}", path.display()))?;
    let file: DaemonConfigFile = toml::from_str(&contents)
        .map_err(|error| format!("解析 daemon 配置失败 {}: {error}", path.display()))?;
    Ok(Some(file.address))
}

/// Save the daemon address to disk.
fn save_daemon_address(address: DaemonAddress) -> Result<(), String> {
    let path = resolve_daemon_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建配置目录失败 {}: {error}", parent.display()))?;
    }
    let file = DaemonConfigFile { address };
    let contents =
        toml::to_string(&file).map_err(|error| format!("序列化 daemon 配置失败: {error}"))?;
    std::fs::write(&path, contents)
        .map_err(|error| format!("写入 daemon 配置失败 {}: {error}", path.display()))
}

/// Returns the currently saved daemon address, if any.
#[tauri::command]
async fn get_daemon_address(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let supervisor = state.daemon.lock().await;
    Ok(supervisor.address.map(|addr| addr.to_string()))
}

/// Sets and persists the daemon address, then connects and returns the new state.
#[tauri::command]
async fn set_daemon_address(
    address: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<DaemonConnectionView, String> {
    let parsed = address
        .parse::<DaemonAddress>()
        .map_err(|error| error.to_string())?;
    let mut supervisor = state.daemon.lock().await;
    supervisor.set_address(app_handle, parsed).await
}

/// Returns the current daemon connection state, attempting to connect if a saved address exists.
#[tauri::command]
async fn get_daemon_state(
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<DaemonConnectionView, String> {
    let mut supervisor = state.daemon.lock().await;
    supervisor.get_state(app_handle).await
}

/// Sends a message to the daemon for the given session.
#[tauri::command]
async fn send_message(
    session_id: String,
    message: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut supervisor = state.daemon.lock().await;
    let client = supervisor.ensure_client(app_handle).await?;
    let params = serde_json::to_value(byte_protocol::SendMessageParams {
        session_id,
        message,
    })
    .map_err(|error| error.to_string())?;
    let _ = client.request("send_message", Some(params)).await?;
    Ok(())
}

/// Creates a new session in `workspace` and returns its generated session id.
#[tauri::command]
async fn new_session(
    workspace: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let mut supervisor = state.daemon.lock().await;
    let client = supervisor.ensure_client(app_handle).await?;
    client.new_session(workspace).await
}

/// Lists all sessions known to the daemon.
#[tauri::command]
async fn list_sessions(
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SessionSummary>, String> {
    let mut supervisor = state.daemon.lock().await;
    let client = supervisor.ensure_client(app_handle).await?;
    client.list_sessions().await
}

/// Deletes the session with the given id.
#[tauri::command]
async fn delete_session(
    session_id: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let mut supervisor = state.daemon.lock().await;
    let client = supervisor.ensure_client(app_handle).await?;
    client.delete_session(session_id).await
}

/// Loads the full view for the session with the given id.
#[tauri::command]
async fn load_session(
    session_id: String,
    app_handle: AppHandle,
    state: State<'_, AppState>,
) -> Result<SessionView, String> {
    let mut supervisor = state.daemon.lock().await;
    let client = supervisor.ensure_client(app_handle).await?;
    client.load_session(session_id).await
}

/// Starts the Tauri desktop application.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
#[allow(clippy::expect_used)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let _ = app.manage(AppState {
                daemon: Mutex::new(DaemonSupervisor::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_daemon_address,
            set_daemon_address,
            get_daemon_state,
            send_message,
            new_session,
            list_sessions,
            delete_session,
            load_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running Byte Agent desktop app");
}
