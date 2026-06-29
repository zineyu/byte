//! Integration tests for the byte daemon over a Unix domain socket.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use byte_protocol::{
    CancelRunParams, DaemonState, DeleteSessionParams, JsonRpcMessage, JsonRpcRequest,
    ListSessionsResult, LoadSessionParams, LoadSessionResult, NewSessionParams, NewSessionResult,
    RpcId, RunStatus, RuntimeEventKind, SendMessageParams, decode_json_line, encode_json_line,
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
                assert!(matches!(event.kind, RuntimeEventKind::DaemonStarted { .. }));
                saw_event = true;
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    stop_daemon(child, &socket_path, None);
}

#[test]
fn send_message_with_missing_config_emits_visible_error_event() {
    let socket_path = unique_socket_path();
    let config_path = unique_config_path();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(&socket_path, &config_path, &data_dir);

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
    stop_daemon(child, &socket_path, Some(&data_dir));
}

#[test]
fn send_message_with_echo_provider_streams_assistant_message() {
    let socket_path = unique_socket_path();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(&socket_path, &config_path, &data_dir);

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
    stop_daemon(child, &socket_path, Some(&data_dir));
}

#[test]
#[allow(clippy::too_many_lines)]
fn send_message_persists_messages_to_session() {
    let socket_path = unique_socket_path();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let data_dir = unique_data_dir();
    let workspace_dir = unique_workspace_dir();
    let child = start_daemon_with_config(&socket_path, &config_path, &data_dir);

    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    std::fs::write(workspace_dir.join("main.rs"), "fn main() {}").expect("main.rs writes");

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let new_params = serde_json::to_value(NewSessionParams {
        workspace: Some(workspace_dir.to_string_lossy().into_owned()),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(20, "new_session", Some(new_params));
    write_request(&mut stream, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(20));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            session_id = Some(result.session_id);
        }
    }
    let session_id = session_id.expect("session was created");

    let params = serde_json::to_value(SendMessageParams {
        session_id: session_id.clone(),
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
        session_id: session_id.clone(),
    })
    .expect("load params encode");
    let load_request = JsonRpcRequest::new(4, "load_session", Some(load_params));
    write_request(&mut stream, &load_request);

    let session = loop {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
            && response.id == RpcId::Number(4)
        {
            let result: LoadSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("load_session result decodes");
            break result.session;
        }
    };

    assert_eq!(session.session_id, session_id);
    assert_eq!(
        session.messages.len(),
        4,
        "developer + assistant tool_call + tool result + final assistant"
    );
    assert_eq!(
        session.messages[0].role,
        byte_protocol::MessageRole::Developer
    );
    assert_eq!(session.messages[0].content, "world");
    assert_eq!(
        session.messages[1].role,
        byte_protocol::MessageRole::Assistant
    );
    assert!(session.messages[1].tool_calls.is_some());
    assert_eq!(session.messages[2].role, byte_protocol::MessageRole::Tool);
    assert_eq!(session.messages[2].content, "fn main() {}");
    assert_eq!(
        session.messages[3].role,
        byte_protocol::MessageRole::Assistant
    );
    assert_eq!(session.messages[3].content, "Echo: world");
    assert_eq!(
        session.messages[1].parent_id,
        Some(session.messages[0].id.clone())
    );
    assert_eq!(
        session.messages[3].parent_id,
        Some(session.messages[2].id.clone())
    );

    drop(stream);
    stop_daemon(child, &socket_path, Some(&data_dir));
    let _ = std::fs::remove_dir_all(&workspace_dir);
}

#[test]
fn session_operations_emit_session_changed_event() {
    let socket_path = unique_socket_path();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &socket_path,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let new_params = serde_json::to_value(NewSessionParams { workspace: None })
        .expect("new session params encode");
    let new_request = JsonRpcRequest::new(5, "new_session", Some(new_params));
    write_request(&mut stream, &new_request);

    let mut created_id: Option<String> = None;
    let mut saw_created = false;
    while !saw_created {
        let line = read_line(&mut reader);
        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.id, RpcId::Number(5));
                assert!(response.error.is_none(), "new_session should succeed");
                let result: NewSessionResult =
                    serde_json::from_value(response.result.expect("response has result"))
                        .expect("new_session result decodes");
                created_id = Some(result.session_id);
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                if let RuntimeEventKind::SessionChanged {
                    session_id: ref id,
                    action: byte_protocol::SessionChangeAction::Created,
                } = event.kind
                    && Some(id) == created_id.as_ref()
                {
                    saw_created = true;
                }
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    let session_id = created_id.expect("session was created");
    let load_params = serde_json::to_value(LoadSessionParams {
        session_id: session_id.clone(),
    })
    .expect("load session params encode");
    let load_request = JsonRpcRequest::new(6, "load_session", Some(load_params));
    write_request(&mut stream, &load_request);

    let mut saw_loaded = false;
    while !saw_loaded {
        let line = read_line(&mut reader);
        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.id, RpcId::Number(6));
                assert!(response.error.is_none(), "load_session should succeed");
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                if matches!(
                    event.kind,
                    RuntimeEventKind::SessionChanged {
                        session_id: ref id,
                        action: byte_protocol::SessionChangeAction::Loaded,
                    } if *id == session_id
                ) {
                    saw_loaded = true;
                }
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path, Some(&data_dir));
}

#[test]
#[allow(clippy::too_many_lines)]
fn list_sessions_and_delete_session() {
    let socket_path = unique_socket_path();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &socket_path,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    // Create a session.
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: Some("/workspace/project".to_owned()),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(7, "new_session", Some(new_params));
    write_request(&mut stream, &new_request);

    let mut created_id: Option<String> = None;
    while created_id.is_none() {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(7));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            created_id = Some(result.session_id);
        }
    }
    let session_id = created_id.expect("session was created");

    // List sessions.
    let list_request = JsonRpcRequest::new(8, "list_sessions", None);
    write_request(&mut stream, &list_request);

    let mut saw_list = false;
    while !saw_list {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
            && response.id == RpcId::Number(8)
        {
            assert!(response.error.is_none(), "list_sessions should succeed");
            let result: ListSessionsResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("list_sessions result decodes");
            assert_eq!(result.sessions.len(), 1);
            assert_eq!(result.sessions[0].session_id, session_id);
            assert_eq!(
                result.sessions[0].workspace.as_deref(),
                Some("/workspace/project")
            );
            saw_list = true;
        }
    }

    // Delete the session.
    let delete_params = serde_json::to_value(DeleteSessionParams {
        session_id: session_id.clone(),
    })
    .expect("delete session params encode");
    let delete_request = JsonRpcRequest::new(9, "delete_session", Some(delete_params));
    write_request(&mut stream, &delete_request);

    let mut saw_deleted = false;
    while !saw_deleted {
        let line = read_line(&mut reader);
        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                assert_eq!(response.id, RpcId::Number(9));
                assert!(response.error.is_none(), "delete_session should succeed");
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                if matches!(
                    event.kind,
                    RuntimeEventKind::SessionChanged {
                        session_id: ref id,
                        action: byte_protocol::SessionChangeAction::Deleted,
                    } if *id == session_id
                ) {
                    saw_deleted = true;
                }
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    // List again to confirm it is gone.
    let list_request = JsonRpcRequest::new(10, "list_sessions", None);
    write_request(&mut stream, &list_request);

    let mut saw_empty_list = false;
    while !saw_empty_list {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
            && response.id == RpcId::Number(10)
        {
            assert!(response.error.is_none(), "list_sessions should succeed");
            let result: ListSessionsResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("list_sessions result decodes");
            assert!(result.sessions.is_empty());
            saw_empty_list = true;
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path, Some(&data_dir));
}

#[test]
fn cancel_run_emits_run_cancelled_and_run_finished_cancelled() {
    let socket_path = unique_socket_path();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &socket_path,
        &write_config(
            "provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'\necho_chunk_size = 1\necho_delay_ms = 20",
        ),
        &data_dir,
    );

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let send_params = serde_json::to_value(SendMessageParams {
        session_id: "cancel-session".to_owned(),
        message: "hello world".to_owned(),
    })
    .expect("send params encode");
    let send_request = JsonRpcRequest::new(11, "send_message", Some(send_params));
    write_request(&mut stream, &send_request);

    // Wait until the assistant message has started so the run is active.
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::MessageStarted { .. })
    });

    let cancel_params = serde_json::to_value(CancelRunParams {
        session_id: "cancel-session".to_owned(),
    })
    .expect("cancel params encode");
    let cancel_request = JsonRpcRequest::new(12, "cancel_run", Some(cancel_params));
    write_request(&mut stream, &cancel_request);

    let mut saw_cancel_response = false;
    let mut saw_run_cancelled = false;
    let mut saw_run_finished_cancelled = false;

    while !(saw_cancel_response && saw_run_cancelled && saw_run_finished_cancelled) {
        let line = read_line(&mut reader);

        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Response(response) => {
                if response.id == RpcId::Number(12) {
                    assert!(response.error.is_none(), "cancel_run should succeed");
                    saw_cancel_response = true;
                }
            }
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                match event.kind {
                    RuntimeEventKind::RunCancelled { .. } => {
                        saw_run_cancelled = true;
                    }
                    RuntimeEventKind::RunFinished {
                        status: RunStatus::Cancelled,
                        error: None,
                        ..
                    } => {
                        saw_run_finished_cancelled = true;
                    }
                    _ => {}
                }
            }
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path, Some(&data_dir));
}
#[test]
fn per_session_workspace_resolves_relative_read_file_path() {
    let socket_path = unique_socket_path();
    let data_dir = unique_data_dir();
    let workspace_dir = unique_workspace_dir();
    let child = start_daemon_with_config(
        &socket_path,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    std::fs::write(workspace_dir.join("main.rs"), "fn main() {}").expect("main.rs writes");

    let mut stream = connect_with_retry(&socket_path);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is set");

    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    wait_for_event_type(&mut reader, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let new_params = serde_json::to_value(NewSessionParams {
        workspace: Some(workspace_dir.to_string_lossy().into_owned()),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(13, "new_session", Some(new_params));
    write_request(&mut stream, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut reader);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(13));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            session_id = Some(result.session_id);
        }
    }
    let session_id = session_id.expect("session was created");

    let send_params = serde_json::to_value(SendMessageParams {
        session_id,
        message: "read main.rs".to_owned(),
    })
    .expect("send params encode");
    let send_request = JsonRpcRequest::new(14, "send_message", Some(send_params));
    write_request(&mut stream, &send_request);

    let mut saw_tool_finished = false;
    let mut saw_run_finished_success = false;

    while !(saw_tool_finished && saw_run_finished_success) {
        let line = read_line(&mut reader);

        match decode_json_line::<JsonRpcMessage>(&line).expect("message decodes") {
            JsonRpcMessage::Notification(notification) => {
                assert_eq!(notification.method, byte_protocol::RUNTIME_EVENT_METHOD);
                let event: byte_protocol::RuntimeEvent =
                    serde_json::from_value(notification.params.expect("notification has params"))
                        .expect("runtime event decodes");
                match event.kind {
                    RuntimeEventKind::ToolFinished {
                        output,
                        is_error: false,
                        ..
                    } => {
                        assert_eq!(
                            output, "fn main() {}",
                            "read_file should resolve relative path against session workspace"
                        );
                        saw_tool_finished = true;
                    }
                    RuntimeEventKind::RunFinished {
                        status: RunStatus::Succeeded,
                        error: None,
                        ..
                    } => {
                        saw_run_finished_success = true;
                    }
                    _ => {}
                }
            }
            JsonRpcMessage::Response(_) => {}
            JsonRpcMessage::Request(_) => panic!("daemon must not send requests to clients"),
        }
    }

    drop(stream);
    stop_daemon(child, &socket_path, Some(&data_dir));
    let _ = std::fs::remove_dir_all(&workspace_dir);
}

fn unique_workspace_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-workspace-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
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

fn start_daemon_with_config(
    socket_path: &Path,
    config_path: &Path,
    data_dir: &Path,
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-socket")
        .arg(socket_path)
        .env("BYTE_CONFIG_PATH", config_path)
        // Production commonly has no XDG_DATA_HOME; exercise the HOME fallback
        // while still isolating integration-test session files.
        .env_remove("XDG_DATA_HOME")
        .env("HOME", data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts")
}

fn stop_daemon(mut child: std::process::Child, socket_path: &Path, data_dir: Option<&Path>) {
    child.kill().expect("daemon can be killed after test");
    child.wait().expect("daemon exits after kill");
    let _ = std::fs::remove_file(socket_path);
    if let Some(dir) = data_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
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
            Err(error) => panic!(
                "failed to connect to daemon socket {}: {error}",
                socket_path.display()
            ),
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
