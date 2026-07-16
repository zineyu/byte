# Data Model: Core Coding Loop

**Feature**: Core Coding Loop
**Date**: 2026-07-16

## Entities

### Session

- **Represents**: A durable conversation and tool-action history for one Developer in one Code Workspace.
- **Fields**:
  - `session_id`: unique identifier.
  - `workspace`: absolute path to the Code Workspace.
  - `created_at`: timestamp.
  - `messages`: JSONL entries forming the active conversation path.
- **Relationships**: A Session may have zero or one active Run in memory; no persisted Run entity is required.
- **Validation**: `session_id` must be safe for filesystem use; paths must be valid and absolute.

### Run

- **Represents**: One accepted execution attempt for a Conversation Turn. It is transient and not persisted.
- **Fields**:
  - `run_id`: UUID string generated when the Run starts.
  - `session_id`: parent Session identifier.
  - `cancel_token`: `CancellationToken` used to stop the Run.
- **State transitions**:
  - `Active → Succeeded` when a Model Turn completes without tool calls.
  - `Active → Cancelled` when the Developer cancels or a terminal cancellation is processed.
  - `Active → Failed` when a provider, tool, or persistence error is fatal.
- **Validation**: Only one Run may be active per Session at a time.

### Message (Persisted Session Entry)

- **Represents**: A single durable unit of conversation or tool output.
- **Fields**:
  - `id`: stable identifier.
  - `parent_id`: identifier of the previous entry in the active path.
  - `role`: `developer`, `assistant`, `tool`, or `summary`.
  - `body`: list of `MessageBlock` values (text, tool call).
  - `tool_call_id`: optional correlation ID for `tool` role messages.
- **Validation**: `assistant` messages with tool calls must include both text and `toolCall` blocks. `tool` messages must include a `tool_call_id` matching a preceding assistant tool call.

### LlmMessage (Ephemeral Provider Context)

- **Represents**: One message sent to the Model Provider.
- **Fields**:
  - `role`: `system`, `developer`, `assistant`, or `tool`.
  - `body`: list of `MessageBlock` values.
- **Validation**: Provider mapping converts `developer` to `user` and `summary` to `system` at the adapter boundary. Assistant messages containing tool calls must precede their matching `tool` messages.

### ToolCall

- **Represents**: A model-requested action.
- **Fields**:
  - `id`: unique identifier for correlation.
  - `name`: tool name registered in the Tool Registry.
  - `arguments`: JSON object of tool parameters.
- **Validation**: Tool name must exist in the registry; arguments must match the tool's JSON schema.

### ToolResult / ToolOutputResult

- **Represents**: The completed outcome of a Tool Call.
- **Fields**:
  - `output`: textual result or error description.
  - `is_error`: boolean indicating whether the result represents a failure.
  - `exit_code`: optional integer for command-like tools.
- **Validation**: A completed result must be returned even for invocation errors, so the model can continue reasoning.

### RuntimeEvent

- **Represents**: An ephemeral notification forwarded to the desktop client.
- **Fields**: variant-specific fields such as `run_id`, `message_id`, `tool_call_id`, `delta`, `status`, and `error`.
- **Validation**: Events for a given Run must be ordered so that clients can reconstruct a consistent view. No durability guarantee is required.

## State Transition Diagram

```text
Idle
 |
 v
RunStarted ──> ModelTurn ──> MessageCompleted
   |                                |
   | has tool_calls                 | no tool_calls
   v                                v
ToolStarted ... ToolFinished    RunFinished(Succeeded)
   |                                ^
   v                                |
ToolResultPersisted                |
   |                                |
   +──> next ModelTurn ─────────────┘

RunStarted ──> ... ──> Cancel ──> RunCancelled ──> RunFinished(Cancelled)
RunStarted ──> ... ──> Error ──> RunFinished(Failed)
```

## Constraints

- A Session may have at most one active Run.
- A Run must have exactly one terminal outcome.
- A completed `assistant` message with tool calls must be persisted before any of its `tool` results.
- A `tool` result must be persisted before it is included in the next Model Turn's context.
- Partial assistant messages must not be persisted on cancellation or failure.
- No hard limit is placed on the number of Model Turns within a Run in this iteration.
