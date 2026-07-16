# Contract: Core Coding Loop

**Feature**: Core Coding Loop
**Date**: 2026-07-16

## Model Provider Contract

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn send_message(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ProviderStream, ProviderError>;
}
```

### Invariants

- The returned `ProviderStream` must yield exactly one `MessageStarted` event and one `MessageCompleted` event.
- `TextDelta` events, if any, must be emitted only between the `MessageStarted` and `MessageCompleted` events for the same `message_id`.
- `MessageCompleted::tool_calls` is `Some` when the model requests tool calls; it is `None` when the model provides a final response.
- `LlmMessage` roles in the `messages` argument will be `system`, `developer`, `assistant`, or `tool`. The provider adapter maps `developer` to `user` and `summary` to `system` before sending to the external service.
- Assistant messages containing tool calls are followed by one or more `tool` role messages with matching `tool_call_id` values.

### Failure Modes

- `ProviderError::Configuration`: the provider is misconfigured and cannot make a request.
- `ProviderError::Request`: the request failed at the transport or service level.
- `ProviderError::InvalidResponse`: the response could not be parsed. The runner treats this as a fatal Run failure.

## Tool Contract

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn invoke(
        &self,
        call: &ToolCall,
        session_ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<ToolOutputStream, ToolError>;
}
```

### Invariants

- `invoke` must return `Ok(stream)` to indicate the tool started; `Err(ToolError)` indicates invocation could not start (unknown tool, bad arguments, policy rejection).
- The `ToolOutputStream` must emit zero or more `Chunk` events and exactly one terminal `Done { result }` event.
- A `Chunk` after `Done`, or the absence of `Done`, is a contract violation.
- `ToolOutputResult` returned in `Done` may have `is_error: true` and an optional `exit_code`. This is a completed failure, not an invocation failure.
- All relative paths in `call.arguments` must be resolved against `session_ctx.workspace_root`.

## Runner Contract

### `SessionRunner::send_message`

- If the Session already has an active Run, return `RunnerError::Busy` without appending the new message to history.
- Otherwise, append the developer message, store `active_run`, and spawn a `RunExecutor`.
- Return the generated `RunId` immediately.

### `SessionRunner::cancel_run`

- If there is an active Run, trigger its `CancellationToken` and wait until the run task clears `active_run`.
- Return `Ok` if no Run is active or after the active Run terminates.
- Cancellation is idempotent.

### `RunExecutor`

- Emit `RunStarted` before the first Model Turn.
- For each Model Turn:
  - Build the provider context from Session history, system instructions, and accumulated `turn_messages`.
  - Call the provider.
  - Stream events to the runtime event bus and persist the completed assistant message.
  - If the assistant message requests tool calls, execute each tool call sequentially, emit tool lifecycle events, and persist each tool result.
- Stop when:
  - A Model Turn completes with no tool calls (Succeeded).
  - Cancellation is requested (Cancelled).
  - A fatal error occurs (Failed).
- Emit exactly one terminal `RunFinished` event, optionally preceded by `RunCancelled`.
- Clear `active_run` on every exit path.
- Do not persist partial assistant messages on cancellation or failure.
- No hard limit on Model Turns or tool-call cycles is enforced in this iteration; the loop terminates based on provider behavior, errors, or cancellation.

## Session Store Contract

- `append_message` must return a stable `id` for the new entry and set its `parent_id` to the previously last entry.
- `load_session` must reconstruct the active path from the JSONL tree so that the runner can resume with the correct history.
- Already persisted messages must not be removed or modified by the runner, even on cancellation or failure.

## Runtime Event Contract

- Events are ordered within a Run and forwarded to the desktop client as they are emitted.
- The client must not use events for recovery; it must use `load_session` to recover durable state.
- Events are not guaranteed to be delivered to a client that connects after the event is emitted.
