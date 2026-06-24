use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const JSON_RPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u16 = 1;
pub const RUNTIME_EVENT_METHOD: &str = "runtime_event";

pub mod session;
pub use session::{
    LoadSessionParams, LoadSessionResult, NewSessionParams, NewSessionResult, SessionEntry,
    SessionMessage, SessionMessageContent, SessionView,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(u64),
    String(String),
}

impl From<u64> for RpcId {
    fn from(value: u64) -> Self {
        Self::Number(value)
    }
}

impl From<&str> for RpcId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: RpcId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(
        id: impl Into<RpcId>,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: impl Into<RpcId>, result: impl Serialize) -> Result<Self, ProtocolError> {
        Ok(Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn failure(id: impl Into<RpcId>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn is_response_to(&self, request: &JsonRpcRequest) -> bool {
        self.id == request.id
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: method.into(),
            params,
        }
    }

    pub fn runtime_event(event: RuntimeEvent) -> Result<Self, ProtocolError> {
        Ok(Self::new(
            RUNTIME_EVENT_METHOD,
            Some(serde_json::to_value(event)?),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum MessageRole {
    Developer,
    Assistant,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::Developer => write!(f, "developer"),
            MessageRole::Assistant => write!(f, "assistant"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeEvent {
    pub sequence: u64,
    #[serde(flatten)]
    #[ts(flatten)]
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    pub fn daemon_started(sequence: u64, state: DaemonState) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::DaemonStarted { state },
        }
    }

    pub fn state_changed(sequence: u64, state: DaemonState) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::StateChanged { state },
        }
    }

    pub fn error(sequence: u64, run_id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::Error {
                run_id,
                message: message.into(),
            },
        }
    }

    pub fn run_started(sequence: u64, session_id: String, run_id: String) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::RunStarted { session_id, run_id },
        }
    }

    pub fn run_finished(
        sequence: u64,
        run_id: String,
        status: RunStatus,
        error: Option<String>,
    ) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::RunFinished {
                run_id,
                status,
                error,
            },
        }
    }

    pub fn message_started(
        sequence: u64,
        run_id: String,
        message_id: String,
        role: MessageRole,
    ) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::MessageStarted {
                run_id,
                message_id,
                role,
            },
        }
    }

    pub fn message_delta(sequence: u64, run_id: String, message_id: String, delta: String) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::MessageDelta {
                run_id,
                message_id,
                delta,
            },
        }
    }

    pub fn message_completed(sequence: u64, run_id: String, message_id: String) -> Self {
        Self {
            sequence,
            kind: RuntimeEventKind::MessageCompleted { run_id, message_id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum RuntimeEventKind {
    DaemonStarted {
        state: DaemonState,
    },
    StateChanged {
        state: DaemonState,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    RunStarted {
        session_id: String,
        run_id: String,
    },
    RunFinished {
        run_id: String,
        status: RunStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    MessageStarted {
        run_id: String,
        message_id: String,
        role: MessageRole,
    },
    MessageDelta {
        run_id: String,
        message_id: String,
        delta: String,
    },
    MessageCompleted {
        run_id: String,
        message_id: String,
    },
}

// JSON-RPC request/result types for model provider operations.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageParams {
    pub session_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageResult {
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DaemonState {
    pub status: DaemonStatus,
    pub daemon_version: String,
    pub protocol_version: u16,
}

impl DaemonState {
    pub fn ready(daemon_version: impl Into<String>) -> Self {
        Self {
            status: DaemonStatus::Ready,
            daemon_version: daemon_version.into(),
            protocol_version: PROTOCOL_VERSION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum DaemonStatus {
    Ready,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("failed to serialize JSON-RPC frame: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub fn encode_json_line<T: Serialize>(message: &T) -> Result<String, ProtocolError> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

pub fn decode_json_line<T: DeserializeOwned>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n']))
}

pub fn decode_json_lines<T: DeserializeOwned>(input: &str) -> Result<Vec<T>, serde_json::Error> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(decode_json_line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_one_json_value_per_lf_delimited_line() {
        let request = JsonRpcRequest::new(7, "get_state", None);

        let line = encode_json_line(&request).expect("request encodes");

        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(
            line.trim_end(),
            r#"{"jsonrpc":"2.0","id":7,"method":"get_state"}"#
        );
    }

    #[test]
    fn decodes_multiple_lf_delimited_json_rpc_frames() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"get_state"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":"two","method":"get_state"}"#,
            "\n"
        );

        let messages: Vec<JsonRpcRequest> = decode_json_lines(input).expect("frames decode");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, RpcId::Number(1));
        assert_eq!(messages[1].id, RpcId::String("two".to_owned()));
    }

    #[test]
    fn preserves_request_response_correlation_ids() {
        let request = JsonRpcRequest::new("state-1", "get_state", None);
        let state = DaemonState::ready("test-daemon");
        let response =
            JsonRpcResponse::success(request.id.clone(), state).expect("response encodes");

        assert!(response.is_response_to(&request));
        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[test]
    fn decodes_send_message_result_and_runtime_events() {
        let run_id = "run-test-1";
        let session_id = "session-test-1";
        let message_id = "msg-test-1";

        let events = vec![
            RuntimeEvent::run_started(2, session_id.into(), run_id.into()),
            RuntimeEvent::message_started(
                3,
                run_id.into(),
                message_id.into(),
                MessageRole::Assistant,
            ),
            RuntimeEvent::message_delta(4, run_id.into(), message_id.into(), "Hello".into()),
            RuntimeEvent::message_delta(5, run_id.into(), message_id.into(), " world".into()),
            RuntimeEvent::message_completed(6, run_id.into(), message_id.into()),
            RuntimeEvent::run_finished(7, run_id.into(), RunStatus::Succeeded, None),
        ];

        for event in events {
            let notification = JsonRpcNotification::runtime_event(event.clone())
                .expect("event notification encodes");
            let decoded: JsonRpcNotification =
                decode_json_line(&encode_json_line(&notification).unwrap()).unwrap();

            assert_eq!(decoded.method, RUNTIME_EVENT_METHOD);
            assert_eq!(decoded.params, Some(serde_json::to_value(event).unwrap()));
        }

        let result = SendMessageResult {
            run_id: run_id.into(),
        };
        let response = JsonRpcResponse::success(42, result.clone()).expect("response encodes");
        let decoded: JsonRpcResponse =
            decode_json_line(&encode_json_line(&response).unwrap()).unwrap();

        assert_eq!(decoded.result, Some(serde_json::to_value(result).unwrap()));
    }

    #[test]
    fn error_event_can_carry_run_id() {
        let event = RuntimeEvent::error(8, Some("run-test-1".into()), "Provider config not found");
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&event).unwrap()).unwrap();

        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::Error {
                run_id: Some(id),
                message,
            } if id == "run-test-1" && message == "Provider config not found"
        ));
    }

    #[test]
    fn decodes_response_and_notification_from_multiplexed_stream() {
        let response = JsonRpcResponse::success(1, DaemonState::ready("test-daemon"))
            .expect("response encodes");
        let notification = JsonRpcNotification::runtime_event(RuntimeEvent::daemon_started(
            1,
            DaemonState::ready("test-daemon"),
        ))
        .expect("notification encodes");
        let input = format!(
            "{}{}",
            encode_json_line(&response).expect("response line encodes"),
            encode_json_line(&notification).expect("notification line encodes")
        );

        let messages: Vec<JsonRpcMessage> = decode_json_lines(&input).expect("messages decode");

        assert!(matches!(messages[0], JsonRpcMessage::Response(_)));
        assert!(matches!(messages[1], JsonRpcMessage::Notification(_)));
    }
}
