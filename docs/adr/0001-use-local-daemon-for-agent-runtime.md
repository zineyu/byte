# Use a local daemon for the agent runtime

Status: Superseded by `docs/adr/0014-manual-start-daemon-websocket-json-rpc.md`.

Byte Agent's MVP will run the Agent Runtime as an **independent local Rust daemon that is started manually by the user**. The Tauri desktop shell acts only as a client, connecting over a WebSocket carrying JSON-RPC. The Tauri shell no longer owns the daemon process. See `docs/adr/0014-manual-start-daemon-websocket-json-rpc.md` for the current design.
