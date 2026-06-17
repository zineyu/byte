# Use a runtime event stream without event sourcing

Byte Agent will model runtime progress as an event stream: model deltas, tool-call lifecycle changes, approval requests, command output, errors, and session updates are emitted as Runtime Events for the desktop client to render. The MVP will not use full event sourcing; persisted sessions may store messages, tool calls, and snapshots directly, avoiding replay-only state reconstruction until audit-grade history or multi-client projection becomes necessary.
