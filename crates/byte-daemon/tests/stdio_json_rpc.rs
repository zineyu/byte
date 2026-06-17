use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use byte_protocol::{
    decode_json_line, encode_json_line, DaemonState, JsonRpcRequest, JsonRpcResponse, RpcId,
};

#[test]
fn daemon_returns_state_with_matching_request_id_over_stdio_jsonl() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_byte-daemon"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("daemon starts");

    let mut stdin = child.stdin.take().expect("daemon stdin is piped");
    let stdout = child.stdout.take().expect("daemon stdout is piped");
    let mut stdout = BufReader::new(stdout);

    let request = JsonRpcRequest::new(42, "get_state", None);
    stdin
        .write_all(
            encode_json_line(&request)
                .expect("request encodes")
                .as_bytes(),
        )
        .expect("request is written");
    stdin.flush().expect("request is flushed");

    let mut response_line = String::new();
    stdout
        .read_line(&mut response_line)
        .expect("response is readable");

    let response: JsonRpcResponse = decode_json_line(&response_line).expect("response decodes");
    assert_eq!(response.id, RpcId::Number(42));
    assert!(response.is_response_to(&request));

    let state: DaemonState = serde_json::from_value(response.result.expect("response has result"))
        .expect("state result decodes");
    assert_eq!(state.protocol_version, byte_protocol::PROTOCOL_VERSION);

    drop(stdin);
    child.wait().expect("daemon exits after stdin closes");
}
