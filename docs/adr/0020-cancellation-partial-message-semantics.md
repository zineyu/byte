# ADR-0020: Do Not Persist Partial Assistant Messages on Cancellation

**Status**: Accepted  
**Date**: 2026-07-16  
**Supersedes**: Prior implicit behavior in `RunExecutor::consume_provider_stream`

## Context

The Core Coding Loop feature (see `specs/001-core-coding-loop/`) requires the daemon to run multiple Model Turns within a single Run. During a Model Turn, the assistant message is streamed incrementally as `MessageDelta` events. A Developer can request cancellation of the active Run at any time while the model is streaming or while a tool action is executing.

Previously, `RunExecutor::consume_provider_stream` persisted any buffered assistant content as a partial `assistant` message in the Session history when cancellation was observed. This left a recoverable but incomplete history entry that did not represent a coherent assistant message (FR-012 of the Core Coding Loop spec forbids this).

## Decision

On cancellation, the runner will flush any remaining buffered deltas as `MessageDelta` events, emit exactly one `RunCancelled` event, and return `RunOutcome::Cancelled`. It will **not** call `SessionStore::append_message` for the partial assistant message. The durable Session history will contain only the previously completed developer messages, assistant messages, and tool results; the partial assistant content is dropped.

The implementation keeps the cancellation branch inside `consume_provider_stream` and removes the previous `append_message` call. `RunExecutor::run` already clears `active_run` and emits the terminal `RunFinished(Cancelled)` event on every exit path, so no additional cleanup is required.

## Alternatives Considered

### 1. Persist partial assistant message with a marker

- **Pros**: The UI could show the partial assistant content after a reconnect or daemon restart.
- **Cons**: Adds a new message role or marker to the persisted format; contradicts the specification that cancelled Runs must not leave partial assistant messages in durable history; requires migration logic.
- **Rejected**: The protocol already supports ephemeral `MessageDelta` events for live progress; durability is reserved for completed messages and tool results.

### 2. Keep the prior behavior and document it as a known exception

- **Pros**: No code change.
- **Cons**: Violates a mandatory acceptance criterion (FR-012) and leaves the Session history in a semantically incoherent state after cancellation.
- **Rejected**: The specification explicitly requires dropping the partial message.

## Consequences

- **Positive**: Session history remains coherent after cancellation; a subsequent Run starts from the last completed message.
- **Positive**: No new persisted format or message role is needed; existing `Message`, `MessageBlock`, and `LlmMessage` shapes are sufficient.
- **Negative**: A client that reconnects after cancellation will not see the partial assistant text from the cancelled Run; it must rely on `MessageDelta` events while the Run is active.
- **Testing**: Regression tests in `crates/byte-core/src/runner.rs` verify that cancellation leaves only completed messages and that a subsequent Run can start in the same Session.
