use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const JSON_RPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
}
