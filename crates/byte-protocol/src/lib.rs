use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::PathBuf;

pub const JSON_RPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u16 = 1;
pub const RUNTIME_EVENT_METHOD: &str = "runtime_event";

pub mod session;
pub use session::{
    CompactionSummary, DeleteSessionParams, DeleteSessionResult, ListSessionsResult,
    LoadSessionParams, LoadSessionResult, NewSessionParams, NewSessionResult, SessionEntry,
    SessionMessage, SessionMessageContent, SessionSummary, SessionView,
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
/// Definition of a tool available to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The result of executing a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

/// A skill available for activation by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
}

/// The full definition of a skill, including its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub content: String,
}

/// A skill that has been activated for the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ActivatedSkill {
    pub name: String,
    pub content: String,
}

/// Runtime context supplied to tool invocations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionContext {
    pub session_id: Option<String>,
    pub workspace_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum MessageRole {
    System,
    Developer,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::System => write!(f, "system"),
            MessageRole::Developer => write!(f, "developer"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::Tool => write!(f, "tool"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SessionChangeAction {
    Created,
    Loaded,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
    Cancelled,
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

impl RuntimeEventKind {
    pub fn daemon_started(state: DaemonState) -> Self {
        Self::DaemonStarted { state }
    }

    pub fn state_changed(state: DaemonState) -> Self {
        Self::StateChanged { state }
    }

    pub fn error(run_id: Option<String>, message: impl Into<String>) -> Self {
        Self::Error {
            run_id,
            message: message.into(),
        }
    }

    pub fn run_started(session_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self::RunStarted {
            session_id: session_id.into(),
            run_id: run_id.into(),
        }
    }

    pub fn run_finished(
        run_id: impl Into<String>,
        status: RunStatus,
        error: Option<String>,
    ) -> Self {
        Self::RunFinished {
            run_id: run_id.into(),
            status,
            error,
        }
    }

    pub fn message_started(
        run_id: impl Into<String>,
        message_id: impl Into<String>,
        role: MessageRole,
    ) -> Self {
        Self::MessageStarted {
            run_id: run_id.into(),
            message_id: message_id.into(),
            role,
        }
    }

    pub fn message_delta(
        run_id: impl Into<String>,
        message_id: impl Into<String>,
        delta: impl Into<String>,
    ) -> Self {
        Self::MessageDelta {
            run_id: run_id.into(),
            message_id: message_id.into(),
            delta: delta.into(),
        }
    }

    pub fn message_completed(
        run_id: impl Into<String>,
        message_id: impl Into<String>,
        tool_calls: Option<Vec<ToolCall>>,
    ) -> Self {
        Self::MessageCompleted {
            run_id: run_id.into(),
            message_id: message_id.into(),
            tool_calls,
        }
    }

    pub fn tool_started(tool_call_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::ToolStarted {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
        }
    }

    pub fn tool_finished(
        tool_call_id: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolFinished {
            tool_call_id: tool_call_id.into(),
            output: output.into(),
            is_error,
        }
    }

    pub fn run_cancelled(run_id: impl Into<String>) -> Self {
        Self::RunCancelled {
            run_id: run_id.into(),
        }
    }

    pub fn session_changed(session_id: impl Into<String>, action: SessionChangeAction) -> Self {
        Self::SessionChanged {
            session_id: session_id.into(),
            action,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    ToolStarted {
        tool_call_id: String,
        name: String,
    },
    ToolFinished {
        tool_call_id: String,
        output: String,
        is_error: bool,
    },
    RunCancelled {
        run_id: String,
    },
    SessionChanged {
        session_id: String,
        action: SessionChangeAction,
    },
}
// JSON-RPC request/result types for session and model-provider operations.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl RunMessage {
    /// Create a simple text message with the given role.
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Create an assistant message that may carry tool calls.
    pub fn assistant(content: impl Into<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls,
        }
    }
}
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
pub struct CancelRunParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRunResult {}

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

/// A view of the daemon connection exposed by the desktop shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DaemonConnectionView {
    pub connected: bool,
    pub state: Option<DaemonState>,
    pub error: Option<String>,
}
impl DaemonConnectionView {
    pub fn connected(state: DaemonState) -> Self {
        Self {
            connected: true,
            state: Some(state),
            error: None,
        }
    }

    pub fn disconnected(error: String) -> Self {
        Self {
            connected: false,
            state: None,
            error: Some(error),
        }
    }
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
            RuntimeEvent {
                sequence: 2,
                kind: RuntimeEventKind::run_started(session_id, run_id),
            },
            RuntimeEvent {
                sequence: 3,
                kind: RuntimeEventKind::message_started(run_id, message_id, MessageRole::Assistant),
            },
            RuntimeEvent {
                sequence: 4,
                kind: RuntimeEventKind::message_delta(run_id, message_id, "Hello"),
            },
            RuntimeEvent {
                sequence: 5,
                kind: RuntimeEventKind::message_delta(run_id, message_id, " world"),
            },
            RuntimeEvent {
                sequence: 6,
                kind: RuntimeEventKind::message_completed(run_id, message_id, None),
            },
            RuntimeEvent {
                sequence: 7,
                kind: RuntimeEventKind::run_finished(run_id, RunStatus::Succeeded, None),
            },
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
        let event = RuntimeEvent {
            sequence: 8,
            kind: RuntimeEventKind::error(Some("run-test-1".into()), "Provider config not found"),
        };
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
        let notification = JsonRpcNotification::runtime_event(RuntimeEvent {
            sequence: 1,
            kind: RuntimeEventKind::daemon_started(DaemonState::ready("test-daemon")),
        })
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

    #[test]
    fn session_changed_event_roundtrips() {
        let event = RuntimeEvent {
            sequence: 9,
            kind: RuntimeEventKind::session_changed("session-test-1", SessionChangeAction::Created),
        };
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&event).unwrap()).unwrap();

        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::SessionChanged {
                session_id,
                action: SessionChangeAction::Created,
            } if session_id == "session-test-1"
        ));
    }

    #[test]
    fn run_cancelled_event_roundtrips() {
        let event = RuntimeEvent {
            sequence: 10,
            kind: RuntimeEventKind::run_cancelled("run-test-cancel"),
        };
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&event).unwrap()).unwrap();

        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::RunCancelled { run_id } if run_id == "run-test-cancel"
        ));
    }

    #[test]
    fn run_status_cancelled_roundtrips() {
        let event = RuntimeEvent {
            sequence: 11,
            kind: RuntimeEventKind::run_finished("run-test-cancel", RunStatus::Cancelled, None),
        };
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&event).unwrap()).unwrap();

        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::RunFinished {
                status: RunStatus::Cancelled,
                error: None,
                ..
            }
        ));
    }

    #[test]
    fn cancel_run_params_and_result_roundtrip() {
        let params = CancelRunParams {
            session_id: "session-cancel-1".into(),
        };
        let decoded: CancelRunParams =
            decode_json_line(&encode_json_line(&params).unwrap()).unwrap();
        assert_eq!(decoded, params);

        let result = CancelRunResult {};
        let response = JsonRpcResponse::success(7, result).expect("response encodes");
        let decoded: JsonRpcResponse =
            decode_json_line(&encode_json_line(&response).unwrap()).unwrap();
        assert_eq!(
            decoded.result,
            Some(serde_json::to_value(CancelRunResult {}).unwrap())
        );
    }

    #[test]
    fn tool_definition_roundtrips() {
        let def = ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        };
        let decoded: ToolDefinition = decode_json_line(&encode_json_line(&def).unwrap()).unwrap();
        assert_eq!(decoded, def);
    }

    #[test]
    fn tool_call_and_result_roundtrip() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };
        let decoded: ToolCall = decode_json_line(&encode_json_line(&call).unwrap()).unwrap();
        assert_eq!(decoded, call);

        let result = ToolResult {
            tool_call_id: "call-1".into(),
            content: "contents".into(),
            is_error: false,
        };
        let decoded: ToolResult = decode_json_line(&encode_json_line(&result).unwrap()).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn session_context_roundtrips() {
        let ctx = SessionContext {
            session_id: Some("session-1".into()),
            workspace_root: Some(PathBuf::from("/tmp/workspace")),
        };
        let decoded: SessionContext = decode_json_line(&encode_json_line(&ctx).unwrap()).unwrap();
        assert_eq!(decoded, ctx);
    }

    #[test]
    fn run_message_with_tool_call_id_roundtrips() {
        let message = RunMessage::tool_result("call-1", "contents");
        let decoded: RunMessage = decode_json_line(&encode_json_line(&message).unwrap()).unwrap();
        assert_eq!(decoded.role, MessageRole::Tool);
        assert_eq!(decoded.tool_call_id, Some("call-1".into()));
        assert_eq!(decoded.content, "contents");
    }
    #[test]
    fn run_message_without_tool_call_id_roundtrips() {
        let message = RunMessage::text(MessageRole::Developer, "hello");
        let decoded: RunMessage = decode_json_line(&encode_json_line(&message).unwrap()).unwrap();
        assert_eq!(decoded.role, MessageRole::Developer);
        assert_eq!(decoded.tool_call_id, None);
        assert_eq!(decoded.content, "hello");
    }

    #[test]
    fn session_message_content_text_roundtrips() {
        let content = SessionMessageContent::text(MessageRole::Assistant, "hello");
        let decoded: SessionMessageContent =
            decode_json_line(&encode_json_line(&content).unwrap()).unwrap();
        assert_eq!(decoded.role, MessageRole::Assistant);
        assert_eq!(decoded.text, Some("hello".into()));
        assert_eq!(decoded.tool_calls, None);
    }

    #[test]
    fn message_role_tool_serializes_as_snake_case() {
        let value = serde_json::to_value(MessageRole::Tool).unwrap();
        assert_eq!(value, serde_json::json!("tool"));
    }

    #[test]
    fn tool_lifecycle_events_roundtrip() {
        let started = RuntimeEvent {
            sequence: 12,
            kind: RuntimeEventKind::tool_started("call-1", "read_file"),
        };
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&started).unwrap()).unwrap();
        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::ToolStarted { tool_call_id, name }
            if tool_call_id == "call-1" && name == "read_file"
        ));

        let finished = RuntimeEvent {
            sequence: 13,
            kind: RuntimeEventKind::tool_finished("call-1", "contents", false),
        };
        let decoded: RuntimeEvent =
            decode_json_line(&encode_json_line(&finished).unwrap()).unwrap();
        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::ToolFinished { tool_call_id, output, is_error }
            if tool_call_id == "call-1" && output == "contents" && !is_error
        ));
    }

    #[test]
    fn message_completed_with_tool_calls_roundtrips() {
        let event = RuntimeEvent {
            sequence: 14,
            kind: RuntimeEventKind::message_completed(
                "run-1",
                "msg-1",
                Some(vec![ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }]),
            ),
        };
        let decoded: RuntimeEvent = decode_json_line(&encode_json_line(&event).unwrap()).unwrap();
        assert!(matches!(
            decoded.kind,
            RuntimeEventKind::MessageCompleted { run_id, message_id, tool_calls }
            if run_id == "run-1" && message_id == "msg-1" && tool_calls.as_ref().map(std::vec::Vec::len) == Some(1)
        ));
    }
}
