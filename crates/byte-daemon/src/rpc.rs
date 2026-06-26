use byte_core::runner::RunnerError;
use byte_core::session_manager::SessionManager;
use byte_protocol::{
    CancelRunParams, DaemonState, DeleteSessionParams, JsonRpcRequest, JsonRpcResponse,
    LoadSessionParams, NewSessionParams, SendMessageParams, SendMessageResult,
};
use tracing::{debug, instrument, warn};

/// Context shared by all JSON-RPC handlers.
#[derive(Clone)]
pub struct RpcContext {
    pub session_manager: SessionManager,
}

/// Dispatch a JSON-RPC request to the appropriate handler.
///
/// Session and run management are delegated to [`SessionManager`]; only
/// `get_state` is handled locally by the daemon transport layer.
#[instrument(skip_all, fields(method = %request.method, id = ?request.id))]
pub async fn handle_request(context: &RpcContext, request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("handling JSON-RPC request");

    match request.method.as_str() {
        "get_state" => handle_get_state(&request),
        "new_session" => handle_new_session(context, &request).await,
        "load_session" => handle_load_session(context, &request).await,
        "list_sessions" => handle_list_sessions(context, &request).await,
        "delete_session" => handle_delete_session(context, &request).await,
        "send_message" => handle_send_message(context, &request).await,
        "cancel_run" => handle_cancel_run(context, &request).await,
        method => {
            JsonRpcResponse::failure(request.id, -32601, format!("method not found: {method}"))
        }
    }
}

fn handle_get_state(request: &JsonRpcRequest) -> JsonRpcResponse {
    let state = daemon_state();
    JsonRpcResponse::success(request.id.clone(), state).unwrap_or_else(|error| {
        JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
    })
}

async fn handle_new_session(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params: NewSessionParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    let session_id = uuid::Uuid::new_v4().to_string();

    match context
        .session_manager
        .new_session(&session_id, params.workspace.as_deref())
        .await
    {
        Ok(result) => {
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, None, error),
    }
}

async fn handle_load_session(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params: LoadSessionParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    let session_id = params.session_id.clone();
    match context.session_manager.load_session(&session_id).await {
        Ok(result) => {
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, Some(&session_id), error),
    }
}

async fn handle_list_sessions(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    match context.session_manager.list_sessions().await {
        Ok(result) => {
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, None, error),
    }
}

async fn handle_delete_session(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params: DeleteSessionParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    let session_id = params.session_id.clone();
    match context.session_manager.delete_session(&session_id).await {
        Ok(result) => {
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, Some(&session_id), error),
    }
}

async fn handle_send_message(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params: SendMessageParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    debug!(message_len = params.message.len(), "handling send_message");

    let session_id = params.session_id.clone();
    match context.session_manager.send_message(params).await {
        Ok(run_id) => {
            let result = SendMessageResult { run_id };
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, Some(&session_id), error),
    }
}

async fn handle_cancel_run(context: &RpcContext, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params: CancelRunParams = match parse_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };

    let session_id = params.session_id.clone();
    match context.session_manager.cancel_run(params).await {
        Ok(result) => {
            JsonRpcResponse::success(request.id.clone(), result).unwrap_or_else(|error| {
                JsonRpcResponse::failure(request.id.clone(), -32603, error.to_string())
            })
        }
        Err(error) => runner_error_response(request, Some(&session_id), error),
    }
}

fn runner_error_response(
    request: &JsonRpcRequest,
    session_id: Option<&str>,
    error: RunnerError,
) -> JsonRpcResponse {
    warn!(%error, "request failed");
    match error {
        RunnerError::Busy => {
            let id = session_id.unwrap_or("unknown");
            JsonRpcResponse::failure(request.id.clone(), -32000, format!("session {id} is busy"))
        }
        RunnerError::SessionStore(session_error) => JsonRpcResponse::failure(
            request.id.clone(),
            -32603,
            format!("session store error: {session_error}"),
        ),
        RunnerError::Provider(provider_error) => JsonRpcResponse::failure(
            request.id.clone(),
            -32603,
            format!("provider error: {provider_error}"),
        ),
    }
}

pub(crate) fn daemon_state() -> DaemonState {
    DaemonState::ready(env!("CARGO_PKG_VERSION"))
}

#[allow(clippy::result_large_err)] // JsonRpcResponse is large; this helper is internal and short-lived.
fn parse_params<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .ok_or_else(|| {
            JsonRpcResponse::failure(
                request.id.clone(),
                -32602,
                format!("invalid params for {}", request.method),
            )
        })
}
