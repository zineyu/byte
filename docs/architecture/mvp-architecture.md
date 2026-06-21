# Byte Agent MVP Architecture

## 1. Goal

Byte Agent MVP is a desktop coding agent for a local code workspace. It uses a Tauri v2 + React desktop shell and a Rust agent runtime daemon. The MVP optimizes for proving the core coding loop: open a workspace, chat with the agent, let the agent inspect/edit files and run commands, stream events to the UI, and persist the session as a branchable history.

## 2. Non-goals

- No full Pi clone.
- No plugin/package ecosystem beyond local Agent Skills.
- No sandboxing or permission filtering in the MVP.
- No PTY or interactive terminal support.
- No background process manager for dev servers/watchers.
- No SQLite session database.
- No multi-provider matrix beyond OpenAI-compatible providers.

## 3. Key decisions

- Agent runtime runs as an independent local Rust daemon, launched by the Tauri shell. See `docs/adr/0001-use-local-daemon-for-agent-runtime.md`.
- Tauri and daemon communicate using LF-delimited JSON-RPC over a Unix Domain Socket. See `docs/adr/0008-use-unix-socket-json-rpc-between-shell-and-daemon.md`.
- Runtime progress is event-driven, but persisted state is not full event sourcing. See `docs/adr/0003-use-runtime-event-stream-without-event-sourcing.md`.
- MVP runs in unrestricted local agent mode. See `docs/adr/0004-use-unrestricted-local-agent-mode-for-mvp.md`.
- Sessions are JSONL trees with `id` / `parent_id`. See `docs/adr/0005-store-sessions-as-jsonl-trees.md`.
- Auto-compaction creates visible session entries. See `docs/adr/0006-store-compaction-as-visible-session-entries.md`.
- MVP secrets are stored in plaintext config behind a replaceable `SecretStore`. See `docs/adr/0007-store-mvp-secrets-in-plaintext-config.md`.

## 4. High-level shape

```text
React UI
  │
  │ Tauri command bridge: start/stop daemon, Unix socket JSONL transport
  │ Tauri event bridge: daemon-event notifications
  ▼
Tauri Desktop Shell
  │
  │ LF-delimited JSON-RPC over Unix Domain Socket
Rust Agent Daemon
  ├─ RpcServer
  ├─ SessionRunner
  ├─ RuntimeEventBus
  ├─ PromptBuilder
  ├─ OpenAiCompatibleProvider
  ├─ ToolRegistry
  ├─ SkillRegistry
  ├─ SessionStore(JSONL tree)
  └─ ConfigStore / SecretStore
```

The React app never directly drives the model/tool loop. It sends user/session commands to the daemon, receives snapshots and runtime events, and renders them.

## 4.1 Pi reference alignment

Pi's coding-agent keeps its core `AgentSession` independent from interactive, RPC, and SDK modes. Byte Agent should keep the same separation: `SessionRunner` and runtime services own the agent loop, while Tauri/React is only one client over the daemon protocol.

Pi also separates tool definition, tool execution, and active-tool selection. Byte Agent should mirror this as `ToolRegistry` + concrete tool implementations + future policy/visibility control, even though the MVP starts with `AllowAllPolicy`.

## 5. Suggested repository layout

```text
/
├── Cargo.toml              # Rust workspace root
├── crates/
│   ├── byte-core/          # SessionRunner, prompt/context, event model
│   ├── byte-protocol/      # JSON-RPC commands, responses, RuntimeEvent, SessionView
│   ├── byte-daemon/        # Unix socket server, process entrypoint
│   ├── byte-models/        # OpenAI-compatible provider, ModelProvider trait
│   ├── byte-tools/         # read/write/edit/ls/grep/find/bash tools
│   ├── byte-skills/        # Agent Skills discovery and activation
│   └── byte-session/       # JSONL tree session store
├── apps/
│   └── desktop/            # Tauri v2 + React app
├── docs/
│   ├── architecture/
│   │   └── mvp-architecture.md
│   └── adr/
└── CONTEXT.md
```

## 6. Core runtime modules

### SessionRunner

Owns the conversation loop for one session:

1. Accept a user message.
2. Build model context.
3. Call `ModelProvider`.
4. Stream assistant deltas.
5. Execute model-requested tools through `ToolRegistry`.
6. Append session entries.
7. Emit runtime events.
8. Repeat until the assistant stops or the run is cancelled.

Constraint: one active run per session. A run can be cancelled. The daemon may later support multiple sessions, but session-internal execution remains serial.

### PromptBuilder

Builds context from:

- fixed system prompt;
- tool definitions;
- skill catalog;
- root workspace instruction files: `AGENTS.md`, `CONTEXT.md`;
- active session path;
- visible `CompactionEntry` summaries when old history has been compacted.

### ModelProvider

MVP implements only `OpenAiCompatibleProvider`, but keeps a trait boundary:

```rust
trait ModelProvider {
    async fn stream_chat(&self, request: ModelRequest) -> ModelStream;
}
```

### ToolRegistry

MVP tool set follows Pi-style basics:

- `read_file`
- `write_file`
- `apply_patch` / edit
- `run_command`
- `list_directory`
- `grep`
- `find_files`
- `activate_skill`

Tools are invoked by `SessionRunner`, not directly by the UI command surface.

### Policy boundary

MVP policy is `AllowAllPolicy` because the product intentionally runs in unrestricted local agent mode. Keep the interface anyway:

```rust
trait ToolPolicy {
    fn check(&self, call: &ToolCall, ctx: &SessionContext) -> PolicyDecision;
}
```

## 7. Runtime events

Use lifecycle + delta event granularity:

```text
run_started
run_finished
run_cancelled
message_started
message_delta
message_completed
tool_started
tool_delta
tool_finished
command_output
session_changed
compaction_started
compaction_finished
error
```

Events drive the React store reducer. Persisted session entries remain the source of recovery; runtime events are not the only state store.

## 8. RPC command surface

MVP daemon commands:

```text
open_workspace
new_session
list_sessions
load_session
send_message
cancel_run
get_state
set_model
```

Responses and runtime events share the local Unix socket as LF-delimited JSON-RPC frames. Commands use request/response objects correlated by `RpcId`; runtime events are JSON-RPC notifications with method `runtime_event`, which Tauri forwards to React as `daemon-event`.

## 9. Session storage

Each session file starts with a header and then append-only entries:

```json
{"type":"session","version":1,"id":"...","workspace":"...","created_at":"..."}
{"type":"message","id":"...","parent_id":null,"message":{...}}
{"type":"tool_result","id":"...","parent_id":"...","tool_call_id":"..."}
{"type":"compaction","id":"...","parent_id":"...","summary":"..."}
```

The active path is resolved by following `parent_id` links. Branching can be implemented by appending a new child to any prior entry without creating a new file.

## 10. Skills

MVP supports local Agent Skills with progressive disclosure:

- scan user and workspace skill directories;
- workspace skill overrides user skill with the same name;
- inject only the skill catalog at session start;
- model may call `activate_skill(name)`;
- user may explicitly invoke `/skill:name`;
- activated skill content is structured, deduplicated, and protected from compaction.

Suggested scan paths:

```text
<workspace>/.byte/skills/
<workspace>/.agents/skills/
~/.byte/skills/
~/.agents/skills/
```

## 11. Frontend architecture

Use a three-column workbench:

```text
┌───────────────┬────────────────────────────┬──────────────────────┐
│ Workspace     │ Chat + Runtime Timeline    │ Tool/Diff Inspector  │
│ Sessions      │ User input                 │ Command output       │
│ Session tree  │ Assistant messages         │ Skill details        │
└───────────────┴────────────────────────────┴──────────────────────┘
```

React state:

- load initial `AppState` / `SessionView` from daemon;
- subscribe to runtime events;
- apply all events through a central store reducer;
- do not parse JSONL directly in React.

## 12. Command execution

`run_command` supports:

- non-interactive commands only;
- configured `cwd`;
- stdout/stderr streaming events;
- exit code;
- timeout;
- cancellation.

No PTY, stdin interaction, or background process registry in MVP.

## 13. Config

MVP config may be plaintext TOML/JSON:

```toml
[provider]
base_url = "https://api.openai.com/v1"
api_key = "..."
model = "gpt-4.1"
```

Keep `ConfigStore` and `SecretStore` separate so plaintext secrets can later move to OS keychain.

## 14. Crash recovery

Tauri owns the daemon child process. If it exits:

1. UI shows daemon disconnected state.
2. Developer can restart daemon.
3. Tauri reloads the last workspace/session.
4. Daemon reconstructs `SessionView` from JSONL.
5. Any active run is marked `interrupted`.

## 15. Testing priorities

Highest-priority tests:

- JSONL RPC framing and request/response correlation;
- event order for a mocked model/tool loop;
- session JSONL append and active-path reconstruction;
- `apply_patch` behavior in temp workspaces;
- `run_command` streaming, timeout, exit code, cancellation;
- skill discovery, collision precedence, and `activate_skill` output.

Use `MockModelProvider` to replay deterministic tool-call transcripts.

## 16. Main risks

- Unrestricted local agent mode can damage user files or leak secrets.
- Plaintext API keys are not secure.
- Auto-compaction can cause model misunderstanding if summaries are poor.
- JSONL tree sessions need careful append and recovery logic.
- Skills from workspace can inject strong behavioral instructions.

These risks are accepted for MVP speed, but the architecture preserves seams for later policy, keychain, sandboxing, and richer session validation.

## 17. MVP acceptance criteria

- User opens one Code Workspace in the desktop app.
- User configures one OpenAI-compatible provider.
- User starts a session and sends a message.
- Agent streams assistant output and tool lifecycle events.
- Agent can read, search, edit files, and run non-interactive commands.
- UI shows messages, tool calls, command output, and diffs.
- Session is saved as JSONL and reloads after app restart.
- Auto-compaction creates a visible Compaction Entry.
- User and workspace Skills are discovered; `activate_skill` works.
