# Use a local daemon for the agent runtime

Byte Agent will run the Agent Runtime as an independent local Rust daemon, while the Tauri desktop app acts as a client over a local IPC or HTTP boundary. This adds MVP lifecycle and packaging complexity compared with embedding the runtime directly in Tauri, but preserves a stable boundary for future clients such as a CLI, IDE integration, or multiple desktop windows sharing the same runtime.
