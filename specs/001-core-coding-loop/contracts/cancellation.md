# Contract: Cancellation

**Feature**: Core Coding Loop
**Date**: 2026-07-16

## Developer-Facing Cancellation

### Request

- The Developer can request cancellation of the active Run for a Session at any time after the Run has started and before it has reached a terminal state.
- The request is delivered through the JSON-RPC `cancel_run` method, which delegates to `SessionRunner::cancel_run`.

### Guarantees

- The Run will reach exactly one terminal outcome: `Cancelled`.
- A `RunCancelled` event may be emitted before the final `RunFinished(Cancelled)` event.
- The Run will not emit a `RunFinished(Succeeded)` or `RunFinished(Failed)` after cancellation is requested, even if a tool or provider action completes afterward.
- The `active_run` state in `SessionRunner` will be cleared so that a subsequent `send_message` can start a new Run.
- A second `cancel_run` request while the first is in progress is idempotent and waits for the Run to terminate.

### Limitations

- Cancellation is best-effort. File writes, commands, or other tool actions that completed before the cancellation signal took effect may leave lasting effects in the Code Workspace or on the system.
- Cancellation does not roll back completed work.
- The Run may not stop instantaneously if the Model Provider or a tool action is blocked in a non-cancellable operation.
- Any buffered assistant text deltas are emitted as `MessageDelta` events before `RunCancelled`, but the partial assistant message is **not** persisted.

## Internal Cancellation Signal

- Each `RunExecutor` receives a child `CancellationToken` derived from the parent token held by `SessionRunner`.
- The `RunExecutor` checks the token at the start of each Model Turn, between provider stream events, and passes it to tool invocations.
- Tools are expected to cooperate with the token by aborting long-running work and returning a `ToolOutputResult` or `ToolError` quickly.
- If cancellation is observed while the provider stream is active, the runner emits the remaining buffered deltas, drops any partial assistant message, and returns `RunOutcome::Cancelled`.
- If cancellation is observed after a tool call has already completed and before the next provider call, the runner emits `RunCancelled` and terminates with `RunFinished(Cancelled)`.

## Ordering and Idempotency

- Exactly one `RunCancelled` event is emitted per Run, even if the Developer requests cancellation multiple times.
- `RunFinished(Cancelled)` is emitted exactly once after the Run terminates.
- No `RunStarted` event is emitted for the same Run after cancellation.
- `SessionRunner::cancel_run` must not return until the run task has cleared `active_run`, ensuring that a subsequent `send_message` observes the Session as idle.
