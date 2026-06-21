# Use Unix Socket JSON-RPC between the desktop shell and daemon

Status: Accepted. Supersedes `docs/adr/0002-use-stdio-json-rpc-between-shell-and-daemon.md`.

Byte Agent will connect the Tauri desktop shell to the local Agent Runtime daemon using LF-delimited JSON-RPC over a Unix Domain Socket. The desktop shell still owns the daemon process lifecycle and passes the socket path at launch, but request/response frames and runtime event notifications now share one local IPC stream instead of stdio. This keeps the transport local-only, avoids exposing a TCP port, allows stderr/stdout to remain operational diagnostics, and gives the shell a single reader task that can route responses by `RpcId` while forwarding `runtime_event` JSON-RPC notifications into React as Tauri `daemon-event` events. Windows named-pipe support is intentionally deferred until Windows packaging becomes a concrete target.
