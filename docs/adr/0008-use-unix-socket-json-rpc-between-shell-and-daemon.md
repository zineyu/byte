# Use Unix Socket JSON-RPC between the desktop shell and daemon

Status: Superseded by `docs/adr/0014-manual-start-daemon-websocket-json-rpc.md`.

Byte Agent will connect the Tauri desktop shell to the local Agent Runtime daemon using **JSON-RPC over a WebSocket**. The daemon is started manually by the user and listens on a loopback address supplied by `--rpc-websocket`. Each WebSocket text frame carries exactly one JSON-RPC message. The desktop shell persists the address in `~/.config/byte/daemon.toml` and reconnects automatically. The shell validates that the address is local (`127.0.0.1` or `localhost`). See `docs/adr/0014-manual-start-daemon-websocket-json-rpc.md` for the current design.
