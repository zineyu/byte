# Byte Agent

Byte Agent is a developer-facing product context for a local coding assistant. Its language distinguishes an interactive coding collaborator from a generic chat client or unattended automation runner.

## Language

**Desktop Coding Agent**:
A local application that helps a developer change a code workspace through conversation and explicit tool actions. It is not a general-purpose chat client, an embeddable runtime alone, or an unattended automation scheduler.
_Avoid_: Generic desktop chat, agent SDK, task runner

**Developer**:
The human using the Desktop Coding Agent to understand, change, and verify a code workspace.
_Avoid_: End user, operator, customer

**Code Workspace**:
The local project directory the Developer has intentionally opened for the Desktop Coding Agent to inspect or modify.
_Avoid_: Folder, repo, project when the access boundary matters

**Core Coding Loop**:
The smallest useful workflow where the Desktop Coding Agent can inspect a Code Workspace, discuss a change, apply file edits or run commands with appropriate approval, and preserve the conversation as a Session.
_Avoid_: Full Pi clone, UI demo, autonomous task pipeline

**Session**:
A saved conversation and tool-action history for one Developer working in one Code Workspace.
_Avoid_: Chat log when tool history matters, task record

### Conversation Turn

A single Developer message and the Desktop Coding Agent's assistant response within a Session-shaped conversation. It is smaller than a saved Session and does not by itself imply durable history.

_Avoid_: Session when persistence and history matter, chat request

See `docs/protocol/glossary.md` for protocol-level terms.

### Run

An accepted execution attempt for one Conversation Turn, starting when the daemon accepts `send_message` and ending in success or failure. A Run is not a Session and does not by itself imply durable history.

_Avoid_: Session, background job, queued task

See `docs/protocol/glossary.md` for protocol-level terms.

### Model Turn

A single request to the Model Provider and the response it yields within a Run. One Conversation Turn may contain multiple Model Turns when the model issues tool calls that must be executed and fed back into a follow-up request. A Model Turn ends when the model returns either assistant content or a set of tool calls.

_Avoid_: Conversation Turn when the whole user-facing exchange matters, Run when persistence and lifecycle matter

### Model Provider

An external model service that the Desktop Coding Agent can ask for assistant responses during a Conversation Turn. The MVP treats it as Developer-configured local product state, not as Code Workspace content.

_Avoid_: Bot, backend, workspace setting

Configuration details: `docs/models/configuration.md`.


**Unrestricted Local Agent Mode**:
A development-mode operating assumption where the Desktop Coding Agent may read, write, and execute commands without runtime permission filtering. It relies on the Developer intentionally running the product in a trusted local environment.
_Avoid_: Safe default, sandboxed mode, permissioned mode

**Workspace Instruction Files**:
The root-level AGENTS.md file in a Code Workspace that contributes Workspace Instructions to a Session.
_Avoid_: Hidden prompt injection, global instructions

**Workspace Instructions**:
The content contributed by Workspace Instruction Files to the system prompt for a Session.
_Avoid_: Hidden prompt injection, global instructions

**Compaction Entry**:
A visible Session entry containing a natural-language summary of older conversation history and the range of messages it replaces, used for continued context construction. It is a durable, persisted entity and is rendered as a distinct timeline item.
_Avoid_: Hidden summary cache, overwritten history

**Message History**:
The complete record of messages and tool actions within a Session, spanning its persisted form, the runtime view, and the LLM context used for a Run.
_Avoid_: Chat log when tool history matters, raw JSONL.

**Message**:
A single persisted Session Entry representing a conversation turn or tool result. It has a Message Role and a Message Body.
_Avoid_: Session entry when the runtime or provider view matters.

**Message Body**:
The payload of a Message, represented as a list of Message Blocks. It is extensible: MVP supports text and tool-call blocks; future blocks may include images or thinking traces.
_Avoid_: String when multi-modal or structured content may be needed; Message Content when the new term is used.

**Message Block**:
A single typed unit inside a Message Body, such as a text segment or a tool call.
_Avoid_: Message when the unit itself is meant.

**Block Delta**:
An incremental update to one Message Block during streaming, used to update the runtime view without replacing the whole Message.
_Avoid_: Message delta when the update targets a block.

## Example dialogue

Developer: "Open this Code Workspace and explain why the tests fail."

Desktop Coding Agent: "I can inspect files and run commands in the selected Code Workspace, then propose and apply a fix with your approval."
