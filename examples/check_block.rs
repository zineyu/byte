use byte_protocol::{MessageBody, MessageBlock, ToolCall};
fn main() {
    let body = MessageBody(vec![
        MessageBlock::Text { text: "hello".into() },
        MessageBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "main.rs"}),
        }),
    ]);
    println!("{}", serde_json::to_string(&body).unwrap());
}
