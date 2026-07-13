//! Integration tests for the byte daemon over a WebSocket.

#![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use byte_protocol::{
    BlockDelta, CancelRunParams, DaemonState, DeleteSessionParams, JsonRpcMessage, JsonRpcRequest,
    ListSessionsResult, LoadSessionParams, LoadSessionResult, NewSessionParams, NewSessionResult,
    RpcId, RunStatus, RuntimeEventKind, SendMessageParams, decode_json_line, encode_json_line,
};
use tungstenite::{Message, WebSocket};
use tungstenite::stream::MaybeTlsStream;

fn message_text(message: &byte_protocol::Message) -> &str {
    match &message.body.0[..] {
        [byte_protocol::MessageBlock::Text { text }] => text.as_str(),
        _ => "",
    }
}

#[test]
fn daemon_returns_state_and_runtime_event_over_websocket() {
    let addr = unique_address();
    let child = start_daemon(&addr);

    let mut socket = connect_with_retry(&addr);
    let request = JsonRpcRequest::new(42, "get_state", None);
    write_request(&mut socket, &request);

    let mut saw_response = false;
    let mut saw_event = false;

    while !(saw_response && saw_event) {
        let line = read_line(&mut socket);

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

    stop_daemon(child, None);
}

#[test]
fn send_message_with_missing_config_emits_visible_error_event() {
    let addr = unique_address();
    let config_path = unique_config_path();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(&addr, &config_path, &data_dir);

    let mut socket = connect_with_retry(&addr);
    // Wait for daemon_started so we know the broadcast channel is ready.
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let workspace_dir = unique_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(0, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut socket);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(0));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            session_id = Some(result.session_id);
        }
    }
    let session_id = session_id.expect("session was created");

    let params = serde_json::to_value(SendMessageParams {
        session_id,
        message: "hello".to_owned(),
    })
    .expect("params encode");
    let request = JsonRpcRequest::new(1, "send_message", Some(params));
    write_request(&mut socket, &request);

    let mut saw_run_id_response = false;
    let mut saw_error = false;
    let mut saw_run_finished_failed = false;

    while !(saw_run_id_response && saw_error && saw_run_finished_failed) {
        let line = read_line(&mut socket);

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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
}

#[test]
fn send_message_with_echo_provider_streams_assistant_message() {
    let addr = unique_address();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(&addr, &config_path, &data_dir);

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let workspace_dir = unique_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(1, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut socket);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(1));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            session_id = Some(result.session_id);
        }
    }
    let session_id = session_id.expect("session was created");

    let params = serde_json::to_value(SendMessageParams {
        session_id,
        message: "world".to_owned(),
    })
    .expect("params encode");
    let request = JsonRpcRequest::new(2, "send_message", Some(params));
    write_request(&mut socket, &request);

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
        let line = read_line(&mut socket);

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
                    RuntimeEventKind::MessageDelta {
                        delta: BlockDelta::TextDelta { delta },
                        ..
                    } => {
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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
}

#[test]
#[allow(clippy::too_many_lines)]
fn send_message_persists_messages_to_session() {
    let addr = unique_address();
    let config_path =
        write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'");
    let data_dir = unique_data_dir();
    let workspace_dir = unique_workspace_dir();
    let child = start_daemon_with_config(&addr, &config_path, &data_dir);

    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    std::fs::write(workspace_dir.join("main.rs"), "fn main() {}").expect("main.rs writes");

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(20, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut socket);
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
    write_request(&mut socket, &request);

    wait_for_event_type(&mut socket, |kind| {
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
    write_request(&mut socket, &load_request);

    let session = loop {
        let line = read_line(&mut socket);
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
    assert_eq!(message_text(&session.messages[0]), "world");
    assert_eq!(
        session.messages[1].role,
        byte_protocol::MessageRole::Assistant
    );
    assert_eq!(session.messages[2].role, byte_protocol::MessageRole::Tool);
    assert_eq!(message_text(&session.messages[2]), "fn main() {}");
    assert_eq!(
        session.messages[3].role,
        byte_protocol::MessageRole::Assistant
    );
    assert_eq!(message_text(&session.messages[3]), "Echo: world");
    assert_eq!(
        session.messages[1].parent_id,
        Some(session.messages[0].id.clone())
    );
    assert_eq!(
        session.messages[3].parent_id,
        Some(session.messages[2].id.clone())
    );

    drop(socket);
    stop_daemon(child, Some(&data_dir));
    let _ = std::fs::remove_dir_all(&workspace_dir);
}

#[test]
fn session_operations_emit_session_changed_event() {
    let addr = unique_address();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &addr,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let workspace_dir = unique_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(5, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut created_id: Option<String> = None;
    let mut saw_created = false;
    while !saw_created {
        let line = read_line(&mut socket);
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
    write_request(&mut socket, &load_request);

    let mut saw_loaded = false;
    while !saw_loaded {
        let line = read_line(&mut socket);
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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
}

#[test]
#[allow(clippy::too_many_lines)]
fn list_sessions_and_delete_session() {
    let addr = unique_address();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &addr,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    // Create a session.
    let workspace_dir = unique_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(7, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut created_id: Option<String> = None;
    while created_id.is_none() {
        let line = read_line(&mut socket);
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
    write_request(&mut socket, &list_request);

    let mut saw_list = false;
    while !saw_list {
        let line = read_line(&mut socket);
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
                result.sessions[0].workspace,
                workspace_dir.to_string_lossy().into_owned()
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
    write_request(&mut socket, &delete_request);

    let mut saw_deleted = false;
    while !saw_deleted {
        let line = read_line(&mut socket);
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
    write_request(&mut socket, &list_request);

    let mut saw_empty_list = false;
    while !saw_empty_list {
        let line = read_line(&mut socket);
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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
}

#[test]
fn cancel_run_emits_run_cancelled_and_run_finished_cancelled() {
    let addr = unique_address();
    let data_dir = unique_data_dir();
    let child = start_daemon_with_config(
        &addr,
        &write_config(
            "provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'\necho_chunk_size = 1\necho_delay_ms = 20",
        ),
        &data_dir,
    );

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let workspace_dir = unique_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(10, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut socket);
        if let JsonRpcMessage::Response(response) =
            decode_json_line::<JsonRpcMessage>(&line).expect("message decodes")
        {
            assert_eq!(response.id, RpcId::Number(10));
            assert!(response.error.is_none(), "new_session should succeed");
            let result: NewSessionResult =
                serde_json::from_value(response.result.expect("response has result"))
                    .expect("new_session result decodes");
            session_id = Some(result.session_id);
        }
    }
    let session_id = session_id.expect("session was created");

    let send_params = serde_json::to_value(SendMessageParams {
        session_id: session_id.clone(),
        message: "hello world".to_owned(),
    })
    .expect("send params encode");
    let send_request = JsonRpcRequest::new(11, "send_message", Some(send_params));
    write_request(&mut socket, &send_request);

    // Wait until the assistant message has started so the run is active.
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::MessageStarted { .. })
    });

    let cancel_params =
        serde_json::to_value(CancelRunParams { session_id }).expect("cancel params encode");
    let cancel_request = JsonRpcRequest::new(12, "cancel_run", Some(cancel_params));
    write_request(&mut socket, &cancel_request);

    let mut saw_cancel_response = false;
    let mut saw_run_cancelled = false;
    let mut saw_run_finished_cancelled = false;

    while !(saw_cancel_response && saw_run_cancelled && saw_run_finished_cancelled) {
        let line = read_line(&mut socket);

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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
}

#[test]
fn per_session_workspace_resolves_relative_read_file_path() {
    let addr = unique_address();
    let data_dir = unique_data_dir();
    let workspace_dir = unique_workspace_dir();
    let child = start_daemon_with_config(
        &addr,
        &write_config("provider = 'echo'\nbase_url = ''\napi_key = ''\nmodel = 'echo'"),
        &data_dir,
    );

    std::fs::create_dir_all(&workspace_dir).expect("workspace dir creates");
    std::fs::write(workspace_dir.join("main.rs"), "fn main() {}").expect("main.rs writes");

    let mut socket = connect_with_retry(&addr);
    wait_for_event_type(&mut socket, |kind| {
        matches!(kind, RuntimeEventKind::DaemonStarted { .. })
    });

    let new_params = serde_json::to_value(NewSessionParams {
        workspace: workspace_dir.to_string_lossy().into_owned(),
    })
    .expect("new session params encode");
    let new_request = JsonRpcRequest::new(13, "new_session", Some(new_params));
    write_request(&mut socket, &new_request);

    let mut session_id: Option<String> = None;
    while session_id.is_none() {
        let line = read_line(&mut socket);
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
    write_request(&mut socket, &send_request);

    let mut saw_tool_finished = false;
    let mut saw_run_finished_success = false;

    while !(saw_tool_finished && saw_run_finished_success) {
        let line = read_line(&mut socket);

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

    drop(socket);
    stop_daemon(child, Some(&data_dir));
    let _ = std::fs::remove_dir_all(&workspace_dir);
}

fn unique_workspace_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "byte-daemon-workspace-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn start_daemon(addr: &SocketAddr) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-websocket")
        .arg(addr.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon starts")
}

fn start_daemon_with_config(
    addr: &SocketAddr,
    config_path: &Path,
    data_dir: &Path,
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .arg("--rpc-websocket")
        .arg(addr.to_string())
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

fn stop_daemon(mut child: std::process::Child, data_dir: Option<&Path>) {
    child.kill().expect("daemon can be killed after test");
    child.wait().expect("daemon exits after kill");
    if let Some(dir) = data_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn connect_with_retry(addr: &SocketAddr) -> WebSocket<MaybeTlsStream<TcpStream>> {
    let started = Instant::now();
    loop {
        match tungstenite::client::connect(format!("ws://{}/", addr)) {
            Ok((socket, _)) => return socket,
            Err(error) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(20));
                let _ = error;
            }
            Err(error) => panic!("failed to connect to daemon WebSocket at {addr}: {error}"),
        }
    }
}

fn write_request(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, request: &JsonRpcRequest) {
    let line = encode_json_line(request).expect("request encodes");
    socket.send(Message::Text(line.into())).expect("request is sent");
    socket.flush().expect("request is flushed");
}

fn read_line(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> String {
    match socket.read().expect("message is readable") {
        Message::Text(text) => text.to_string(),
        Message::Close(_) => {
            panic!("daemon closed the WebSocket unexpectedly")
        }
        other => panic!("unexpected WebSocket message: {other:?}"),
    }
}

fn wait_for_event_type(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    predicate: impl Fn(RuntimeEventKind) -> bool,
) {
    loop {
        let line = read_line(socket);
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

fn unique_address() -> SocketAddr {
    format!("127.0.0.1:{}", unique_port()).parse().unwrap()
}

fn unique_port() -> u16 {
    // Use a high ephemeral port derived from the process id and timestamp to
    // avoid collisions between concurrent tests.
    let base = (std::process::id() as u128 + unique_suffix()) % 16384;
    49152 + base as u16
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
