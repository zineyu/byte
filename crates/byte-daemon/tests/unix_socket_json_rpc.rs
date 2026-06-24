#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcMessage, JsonRpcRequest,
    LoadSessionParams, LoadSessionResult, RpcId, RunStatus, RuntimeEventKind, SendMessageParams,
};

#[test]
fn daemon_returns_state_and_runtime_event_over_unix_socket_jsonl() {
    let socket_path = unique_socket_path();
    let child = start_daemon(&socket_path);

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let request = JsonRpcRequest::new(42, "get_state", None);
    write_request(&mut stream, &request);

    let mut reader = BufReader::new(stream);
    let mut saw_response = false;
    let mut saw_event = false;

    while !(saw_response && saw_event) {
        let line = read_line(&mut reader);

        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.id, RpcId::Number(42));
                assert!(response.is_response_to(&request));
                let state: DaemonState =
                    serde_json::from_value(response.result.expect("response has result"))
                        .expect("state result decodes");
                assert_eq!(state.protocol_version, byte_protocol::PROTOCOL_VERSION);
                saw_response = true;
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                assert!(matches!(
                    event.kind,
                    RuntimeEventKind::DaemonStarted { .. } | RuntimeEventKind::StateChanged { .. }
                ));
                saw_event = true;
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    stop_daemon(child, &socket_path);
}

#[test]
fn send_message_with_missing_config_emits_visible_error_event() {
    let socket_path = unique_socket_path();
    let config_path = unique_config_path();
    let child = start_daemon_with_config(&socket_path, &config_path);

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    // Wait for daemon_started so we know the broadcast channel is ready.
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let params = serde_json::to_value(SendMessageParams {
        session_id: "default".to_owned(),
        message: "hello".to_owned(),
    })
    .expect("params encode");
    let request = JsonRpcRequest::new(1, "send_message", Some(params));
    write_request(&mut stream, &request);

    let mut saw_run_id_response = false;
    let mut saw_error = false;
    let mut saw_run_finished_failed = false;

    while !(saw_run_id_response && saw_error && saw_run_finished_failed) {
        let line = read_line(&mut reader);

        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.id, RpcId::Number(1));
                assert!(
                    response.error.is_none(),
                    "send_message should return run_id"
                );
                let result: serde_json::Value = response.result.expect("response has result");
                assert!(result.get("run_id").is_some());
                saw_run_id_response = true;
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                match event.kind {
                    RuntimeEventKind::Error {
                        run_id: Some(_), ..
                    } => {
                        saw_error = true;
                    }
                    RuntimeEventKind::RunFinished {
                        status: RunStatus::Failed,
                        ..
                    } => {
                        saw_run_finished_failed = true;
                    }
                    _ => {}
                }
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path);
}

#[test]
fn send_message_with_echo_provider_streams_assistant_message() {
    let socket_path = unique_socket_path();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let child = start_daemon_with_config(&socket_path, &config_path);

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let params = serde_json::to_value(SendMessageParams {
        session_id: "default".to_owned(),
        message: "world".to_owned(),
    })
    .expect("params encode");
    let request = JsonRpcRequest::new(2, "send_message", Some(params));
    write_request(&mut stream, &request);

    let mut saw_run_started = false;
    let mut saw_message_started = false;
    let mut saw_delta = false;
    let mut saw_message_completed = false;
    let mut saw_run_finished_success = false;

    while !(saw_run_started
        && saw_message_started
        && saw_delta
        && saw_message_completed
        && saw_run_finished_success)
    {
        let line = read_line(&mut reader);

        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                match event.kind {
                    RuntimeEventKind::RunStarted { .. } => saw_run_started = true,
                    RuntimeEventKind::MessageStarted { role, .. } => {
                        assert_eq!(role, byte_protocol::MessageRole::Assistant);
                        saw_message_started = true;
                    }
                    RuntimeEventKind::MessageDelta { delta, .. } => {
                        assert!(!delta.is_empty());
                        saw_delta = true;
                    }
                    RuntimeEventKind::MessageCompleted { .. } => saw_message_completed = true,
                    RuntimeEventKind::RunFinished {
                        status: RunStatus::Succeeded,
                        error: None,
                        ..
                    } => saw_run_finished_success = true,
                    _ => {}
                }
            }
            JsonRpcMessage::Response(_) => {}
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path);
}

#[test]
fn send_message_persists_messages_to_session() {
    let socket_path = unique_socket_path();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config_and_data_dir(&socket_path, &config_path, &data_dir);

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let params = serde_json::to_value(SendMessageParams {
        session_id: "default".to_owned(),
        message: "world".to_owned(),
    })
    .expect("params encode");
    let request = JsonRpcRequest::new(3, "send_message", Some(params));
    write_request(&mut stream, &request);

    wait_for_event_type(&mut reader, |kind| {
        matches!(
            kind,
            RuntimeEventKind::RunFinished {
                status: RunStatus::Succeeded,
                error: None,
                ..
            }
        )
    });

    let load_params = serde_json::to_value(LoadSessionParams {
        session_id: "default".to_owned(),
    })
    .expect("load params encode");
    let load_request = JsonRpcRequest::new(4, "load_session", Some(load_params));
    write_request(&mut stream, &load_request);

    let session = loop {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            if response.id == RpcId::Number(4) {
                let result: LoadSessionResult =
                    serde_json::from_value(response.result.expect("response has result"))
                        .expect("load_session result decodes");
                break result.session;
            }
        }
    };

    assert_eq!(session.session_id, "default");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(
        session.messages[0].role,
        byte_protocol::MessageRole::Developer
    );
    assert_eq!(session.messages[0].content, "world");
    assert_eq!(
        session.messages[1].role,
        byte_protocol::MessageRole::Assistant
    );
    assert_eq!(session.messages[1].content, "Echo: world");
    assert_eq!(
        session.messages[1].parent_id,
        Some(session.messages[0].id.clone())
    );

    drop(stream);
    stop_daemon(child, &socket_path);
}

fn start_daemon(socket_path: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-socket")
        .arg(socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts")
}

fn start_daemon_with_config(socket_path: &Path, config_path: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-socket")
        .arg(socket_path)
        .env("BYTE_CONFIG_PATH", config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts")
}

fn start_daemon_with_config_and_data_dir(
    socket_path: &Path,
    config_path: &Path,
    data_dir: &Path,
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-socket")
        .arg(socket_path)
        .env("BYTE_CONFIG_PATH", config_path)
        .env("XDG_DATA_HOME", data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts")
}

fn stop_daemon(mut child: std::process::Child, socket_path: &Path) {
    child.kill().expect("daemon can be killed after test");
    child.wait().expect("daemon exits after kill");
    let _ = std::fs::remove_file(socket_path);
}

fn connect_with_retry(socket_path: &Path) -> UnixStream {
    let started = Instant::now();
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return stream,
            Err(error) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(20));
                let _ = error;
            }
            Err(error) => panic!("failed to connect to daemon socket {socket_path:?}: {error}"),
        }
    }
}

fn write_request(stream: &mut UnixStream, request: &JsonRpcRequest) {
    stream
        .write_all(
            encode_json_line(request)
                .expect("request encodes")
                .as_bytes(),
        )
        .expect("request is written");
    stream.flush().expect("request is flushed");
}

fn read_line(reader: &mut BufReader<UnixStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("message is readable");
    assert!(!line.is_empty(), "daemon closed the socket unexpectedly");
    line
}

fn wait_for_event_type(
    reader: &mut BufReader<UnixStream>,
    predicate: impl Fn(RuntimeEventKind) -> bool,
) {
    loop {
        let line = read_line(reader);
        if let JsonRpcMessage::Notification(notification) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
            let event: byte_protocol::RuntimeEvent =
                serde_json::from_value(notification.params.expect("notification has params"))
                    .expect("runtime event decodes");
            if predicate(event.kind) {
                return;
            }
        }
    }
}

fn unique_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-test-{}-{}.sock",
        std::process::id(),
        unique_suffix()
    ))
}

fn unique_config_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-config-test-{}-{}.toml",
        std::process::id(),
        unique_suffix()
    ))
}

fn unique_data_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-data-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn write_config(contents: &str) -> PathBuf {
    let path = unique_config_path();
    std::fs::write(&path, contents).expect("config file writes");
    path
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after unix epoch")
        .as_nanos()
}
