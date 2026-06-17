use std::io::{self, BufRead, Write};

use anyhow::Context;
use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcRequest, JsonRpcResponse,
};

fn main() -> anyhow::Result<()> {
    run_stdio_server(io::stdin().lock(), io::stdout().lock())
}

fn run_stdio_server<R, W>(reader: R, mut writer: W) -> anyhow::Result<()>
where
    R: BufRead,
    W: Write,
{
    for line in reader.lines() {
        let line = line.context("failed to read JSON-RPC frame from stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match decode_json_line::<JsonRpcRequest>(&line) {
            Ok(request) => handle_request(request),
            Err(error) => JsonRpcResponse::failure(0, -32700, format!("parse error: {error}")),
        };

        let encoded = encode_json_line(&response).context("failed to encode JSON-RPC response")?;
        writer
            .write_all(encoded.as_bytes())
            .context("failed to write JSON-RPC response to stdout")?;
        writer
            .flush()
            .context("failed to flush JSON-RPC response")?;
    }

    Ok(())
}

fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    match request.method.as_str() {
        "get_state" => {
            JsonRpcResponse::success(request.id, DaemonState::ready(env!("CARGO_PKG_VERSION")))
                .unwrap_or_else(|error| JsonRpcResponse::failure(0, -32603, error.to_string()))
        }
        method => {
            JsonRpcResponse::failure(request.id, -32601, format!("method not found: {method}"))
        }
    }
}
