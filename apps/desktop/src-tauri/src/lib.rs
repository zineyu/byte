use std::path::PathBuf;
use std::process::Stdio;

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcRequest, JsonRpcResponse, RpcId,
};
use serde::Serialize;
use tauri::{Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

struct AppState {
    daemon: Mutex<DaemonSupervisor>,
}

struct DaemonSupervisor {
    client: Option<DaemonClient>,
    last_error: Option<String>,
}

impl DaemonSupervisor {
    fn start() -> Self {
        match DaemonClient::spawn() {
            Ok(client) => Self {
                client: Some(client),
                last_error: None,
            },
            Err(error) => Self {
                client: None,
                last_error: Some(error),
            },
        }
    }

    async fn get_state(&mut self) -> DaemonConnectionView {
        if self.client.is_none() {
            match DaemonClient::spawn() {
                Ok(client) => {
                    self.client = Some(client);
                    self.last_error = None;
                }
                Err(error) => {
                    self.last_error = Some(error.clone());
                    return DaemonConnectionView::disconnected(error);
                }
            }
        }

        let Some(client) = self.client.as_mut() else {
            return DaemonConnectionView::disconnected(
                self.last_error
                    .clone()
                    .unwrap_or_else(|| "daemon is not running".to_owned()),
            );
        };

        match client.get_state().await {
            Ok(state) => {
                self.last_error = None;
                DaemonConnectionView::connected(state)
            }
            Err(error) => {
                self.client = None;
                self.last_error = Some(error.clone());
                DaemonConnectionView::disconnected(error)
            }
        }
    }
}

struct DaemonClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
}

impl DaemonClient {
    fn spawn() -> Result<Self, String> {
        let daemon_path = resolve_daemon_path();
        let mut command = Command::new(&daemon_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to launch daemon at {daemon_path:?}: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open daemon stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to open daemon stdout".to_owned())?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            next_request_id: 1,
        })
    }

    async fn get_state(&mut self) -> Result<DaemonState, String> {
        let request_id = self.next_id();
        let request = JsonRpcRequest::new(request_id.clone(), "get_state", None);
        let request_line = encode_json_line(&request).map_err(|error| error.to_string())?;

        self.stdin
            .write_all(request_line.as_bytes())
            .await
            .map_err(|error| format!("failed to write get_state request: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("failed to flush get_state request: {error}"))?;

        let mut response_line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut response_line)
            .await
            .map_err(|error| format!("failed to read get_state response: {error}"))?;
        if bytes_read == 0 {
            return Err("daemon closed stdout before responding".to_owned());
        }

        let response: JsonRpcResponse = decode_json_line(&response_line)
            .map_err(|error| format!("failed to decode get_state response: {error}"))?;
        if response.id != request_id {
            return Err(format!(
                "daemon response id mismatch: expected {request_id:?}, got {:?}",
                response.id
            ));
        }
        if let Some(error) = response.error {
            return Err(format!(
                "daemon returned error {}: {}",
                error.code, error.message
            ));
        }

        let result = response
            .result
            .ok_or_else(|| "daemon get_state response did not include a result".to_owned())?;
        serde_json::from_value(result)
            .map_err(|error| format!("failed to decode daemon state: {error}"))
    }

    fn next_id(&mut self) -> RpcId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RpcId::Number(id)
    }
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
async fn get_daemon_state(state: State<'_, AppState>) -> Result<DaemonConnectionView, String> {
    Ok(state.daemon.lock().await.get_state().await)
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState {
                daemon: Mutex::new(DaemonSupervisor::start()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_daemon_state])
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
