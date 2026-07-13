# Manual-start daemon with WebSocket JSON-RPC

Status: Accepted. Supersedes `docs/adr/0001-use-local-daemon-for-agent-runtime.md` and `docs/adr/0008-use-unix-socket-json-rpc-between-shell-and-daemon.md`.

Date: 2026-07-13

## Context

Byte Agent originally launched the Rust daemon from the Tauri desktop shell and spoke to it over a Unix domain socket. This worked for a single desktop client, but it tightly coupled the daemon lifecycle to the desktop app, made multi-client sharing impossible, and required a fresh Unix socket per process.

As the product matures, we want:

- Multiple clients (desktop windows, future CLI/IDE integrations) to share the same local runtime.
- The daemon to survive desktop app restarts and reconnections.
- A transport that works on all developer platforms without platform-specific IPC code paths.
- A clear manual boundary: the user chooses when to start and stop the daemon.

## Decision

The daemon is started manually by the user. Tauri no longer spawns, owns, or kills the daemon process. The desktop shell connects to the daemon over JSON-RPC on a WebSocket, with one WebSocket text frame carrying exactly one JSON-RPC message.

Key consequences:

- The daemon listens on a WebSocket address supplied by `--rpc-websocket <addr>`.
- The desktop app stores the daemon WebSocket address in `~/.config/byte/daemon.toml` and reconnects automatically on launch.
- On first launch, or if the saved address cannot be reached, the desktop app shows a connection dialog asking for the daemon address.
- Only `127.0.0.1` or `localhost` addresses are accepted; the shell rejects non-local addresses.
- The daemon keeps a set of connected WebSocket clients and broadcasts `runtime_event` notifications to all of them.
- Multiple clients can view the same session, but only one active run per session is allowed; concurrent `send_message` calls return a `SessionBusy` error.
- MVP remains unauthenticated and relies on the local-trusted environment, consistent with `docs/adr/0004-use-unrestricted-local-agent-mode-for-mvp.md`.
- `PROTOCOL_VERSION` in `byte-protocol` is bumped from `7` to `8` to signal the transport and lifecycle change.

## Alternatives considered

### Keep the Unix socket and spawn the daemon from Tauri

- Rejected: it prevents multi-client sharing, ties daemon lifetime to the desktop app, and requires Unix-specific code. WebSocket is local-only in practice because of the address restriction, while being portable and easy to test.

### Use HTTP/REST instead of WebSocket

- Rejected: JSON-RPC request/response correlation plus streamed runtime notifications naturally fit a single bidirectional WebSocket. Two separate transports (HTTP + SSE) would increase complexity without benefit for a local agent.

### Listen on a TCP port without address restriction

- Rejected: listening on `0.0.0.0` or allowing non-local addresses would expose the unrestricted local agent to the network. Restricting to loopback preserves the local-only security model.

## Consequences

- The daemon becomes a long-lived local service rather than a child process.
- Closing the desktop app does not stop the daemon.
- The desktop shell must handle reconnection, address validation, and persistence.
- The protocol version change makes old clients detect a mismatch when connecting to a new daemon.
- Existing Unix socket integration tests must be rewritten to speak WebSocket.
- Future clients can connect to the same address and share the same runtime state.
