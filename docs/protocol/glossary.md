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

The role of a message inside a Run or a persisted Session history node:

- `system`: a dynamic, per-run instruction and tool context built by `LlmContextBuilder`. It is **not** persisted in the Session.
- `developer`: the human Developer message.
- `assistant`: the model-generated response.
- `tool`: a tool result returned to the model, persisted as a `Message` entry with a `text` body block.
- `summary`: a compacted summary of earlier conversation history, persisted as a visible Session entry.

At the provider adapter boundary, `system` maps to the OpenAI `system` role and `developer` maps to the OpenAI `user` role. `summary` is converted to a `system` message before being sent to the Model Provider.

### Message Body

The content payload of a Message, represented as a list of Message Blocks.

_Avoid_: Message Content when the new term is used.

### Message Block

A single typed unit inside a Message Body, such as a text segment or a tool call. MVP supports `text` and `toolCall` blocks; future blocks may include images or thinking traces.

### Block Delta

An incremental update to one Message Block during streaming, used to update the runtime view without replacing the whole Message. MVP only streams `text` deltas; tool-call blocks are emitted atomically at the end of a message.

### LlmMessage

A message in the LLM context produced by `LlmContextBuilder` and sent to the Model Provider. It shares the same Message Body shape as a persisted Message but may include non-persisted system and summary messages.

_Avoid_: Run Message when the message is for the LLM context.
