# Protocol Glossary

This glossary captures the protocol-level and runtime terms used across `byte-protocol`, `byte-daemon`, and the desktop shell.

When working on JSON-RPC methods, runtime events, or cross-boundary types, use these terms.

## Terms

### Conversation Turn

A single Developer message and the Desktop Coding Agent's assistant response within a Session-shaped conversation. It is smaller than a saved Session and does not by itself imply durable history.

_Avoid_: Session when persistence and history matter, chat request

### Run

An accepted execution attempt for one Conversation Turn, starting when the daemon accepts `send_message` and ending in success or failure. A Run is not a Session and does not by itself imply durable history.

_Avoid_: Session, background job, queued task

### Model Provider

An external model service that the Desktop Coding Agent can ask for assistant responses during a Conversation Turn. The MVP treats it as Developer-configured local product state, not as Code Workspace content.

_Avoid_: Bot, backend, workspace setting

### Runtime Event

A one-way notification emitted by the daemon and forwarded by the desktop shell to the React frontend. Runtime events report state changes, run lifecycle, and streaming message deltas.

_Avoid_: Log line, notification, bus message

### Message Role

The role of a message inside a Run:

- `developer`: the human Developer message
- `assistant`: the model-generated response

At the provider adapter boundary, `developer` maps to the OpenAI `user` role.

_Avoid_: User, bot
