#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcMessage, JsonRpcRequest, RpcId,
    RuntimeEventKind,
};

#[test]
fn daemon_returns_state_and_runtime_event_over_unix_socket_jsonl() {
    let socket_path = unique_socket_path();
    let mut child = Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-socket")
        .arg(&socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts");

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout is set");

    let request = JsonRpcRequest::new(42, "get_state", None);
    stream
        .write_all(
            encode_json_line(&request)
                .expect("request encodes")
                .as_bytes(),
        )
        .expect("request is written");
    stream.flush().expect("request is flushed");

    let mut reader = BufReader::new(stream);
    let mut saw_response = false;
    let mut saw_event = false;

    while !(saw_response && saw_event) {
        let mut line = String::new();
        reader.read_line(&mut line).expect("message is readable");
        assert!(!line.is_empty(), "daemon closed the socket unexpectedly");

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

fn unique_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-test-{}-{}.sock",
        std::process::id(),
        unique_suffix()
    ))
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after unix epoch")
        .as_nanos()
}
