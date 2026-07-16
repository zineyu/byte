# Quickstart: Core Coding Loop Validation

**Feature**: Core Coding Loop
**Date**: 2026-07-16

## Prerequisites

- A working Rust toolchain and `cargo`.
- A working Node.js/pnpm environment for the desktop shell (no changes are expected there, but `just verify` includes desktop checks).
- `just` installed for task running.
- The daemon and desktop are not required to be running for the automated tests, but a manual end-to-end demo uses both.

## Automated Validation

### 1. Run the Rust test suite

```bash
just verify rust
```

Expected outcome: all `cargo test` tests pass, including existing loop tests and new tests for:
- cancellation without partial assistant persistence,
- read → apply_patch → run_command → final response demo.

### 2. Run the full verification gate

```bash
just verify
```

Expected outcome: all gates pass, including `cargo fmt`, `cargo clippy`, `cargo test`, desktop typecheck/build/audit, and `npx @google/design.md lint DESIGN.md` with no errors.

### 3. Run only the runner tests during development

```bash
cargo test -p byte-core runner::
```

Expected outcome: focused runner tests pass quickly.

## Manual End-to-End Demo

### 1. Start the daemon

```bash
just start-daemon
```

The daemon listens on the default loopback address (e.g., `127.0.0.1:8787`).

### 2. Start the desktop client

In a second terminal:

```bash
just start-desktop
```

### 3. Configure the daemon address

In the desktop settings, enter the daemon address if it is not already configured, or ensure `~/.config/byte/daemon.toml` contains the correct address.

### 4. Create a test Code Workspace

Create a temporary directory with a file that can be read and edited, plus a script that can be run non-interactively. For example:

```bash
mkdir -p /tmp/byte-loop-demo
cd /tmp/byte-loop-demo
echo 'fn main() { println!("old"); }' > src/main.rs
```

Create a `verify.sh` script that returns exit code 0 on success:

```bash
#!/bin/bash
cargo check
```

### 5. Send a coding request

In the desktop UI, open the test workspace and send a message such as:

> "Read src/main.rs, change the printed message to 'new', run cargo check, and tell me the result."

### 6. Observe the expected flow

- `RunStarted` appears.
- The model requests `read_file` on `src/main.rs`.
- `ToolStarted`, `ToolFinished`, and the file content are visible.
- The model requests `apply_patch` to edit `src/main.rs`.
- `ToolFinished` shows the applied diff.
- The model requests `run_command` with `cargo check`.
- Command output streams and the final exit code appears.
- The model returns a final assistant response.
- `RunFinished(Succeeded)` appears.
- The Session history shows the developer message, assistant tool calls, and tool results in order.

### 7. Test cancellation

Start a long-running request such as:

> "Run sleep 30 and then echo done."

While the command is running, click Cancel. Observe:

- `RunCancelled` appears.
- `RunFinished(Cancelled)` appears.
- The command process is killed (if it cooperates with the cancellation token).
- The Session history does not contain a partial assistant message; it contains the developer message and any completed tool results before cancellation.

### 8. Test concurrency rejection

Start a request that takes time to complete. While it is running, send a second message in the same Session. Observe:

- The second request is rejected as busy.
- The first Run continues unchanged.
- The Session history does not contain the rejected second message.

## Expected Final Outcomes

- 100% of automated loop tests pass.
- The manual demo completes the read → edit → command → final response flow without client-side orchestration.
- Cancellation leaves a recoverable Session and allows a subsequent Run.
- Same-Session concurrent requests are rejected cleanly.
- `just verify` passes with no errors.
