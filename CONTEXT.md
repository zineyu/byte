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


**Unrestricted Local Agent Mode**:
A development-mode operating assumption where the Desktop Coding Agent may read, write, and execute commands without runtime permission filtering. It relies on the Developer intentionally running the product in a trusted local environment.
_Avoid_: Safe default, sandboxed mode, permissioned mode

**Workspace Instruction Files**:
The root-level AGENTS.md and CONTEXT.md files in a Code Workspace that are shown to the Developer and included in prompt context for a Session.
_Avoid_: Hidden prompt injection, global instructions

**Compaction Entry**:
A visible Session entry containing a summary of older conversation history for continued context construction.
_Avoid_: Hidden summary cache, overwritten history
## Example dialogue

Developer: "Open this Code Workspace and explain why the tests fail."

Desktop Coding Agent: "I can inspect files and run commands in the selected Code Workspace, then propose and apply a fix with your approval."
