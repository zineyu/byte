# Use stdio JSON-RPC between the desktop shell and daemon

Status: Superseded by `docs/adr/0008-use-unix-socket-json-rpc-between-shell-and-daemon.md`.


Byte Agent will connect the Tauri desktop shell to the local Agent Runtime daemon using LF-delimited JSON-RPC over stdio. This avoids exposing a localhost port during the MVP, keeps process ownership with the desktop shell, and still supports streamed events, cancellation, and request-response commands; HTTP or platform-native IPC can be reconsidered once multi-client access becomes a real requirement.
