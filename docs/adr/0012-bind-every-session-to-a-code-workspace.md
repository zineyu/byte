# Bind every Session to a Code Workspace

Byte Agent will require every Session to be bound to exactly one Code Workspace at creation time. The workspace path will be persisted in the Session header, made required in the protocol types, and validated to exist as a directory before the Session is created. This decision removes the ambiguity of no-workspace Sessions and lets every Run resolve Workspace Instructions and tool paths against a well-known root.

We considered keeping `workspace` optional in `NewSessionParams`, `SessionEntry`, and `SessionView` while simply never passing `null`. We rejected that because the type system would still allow accidental no-workspace Sessions and would force every consumer to handle the `None` case. Making the field required enforces the domain invariant that a Session is "a saved conversation and tool-action history for one Developer working in one Code Workspace."

Consequences:
- `NewSessionParams.workspace`, `SessionEntry::Session.workspace`, `SessionSummary.workspace`, `SessionView.workspace`, and `SessionContext.workspace_root` become non-optional.
- Existing Session JSONL files with `"workspace": null` will fail to load; the Developer must delete them. This is acceptable for the MVP because session persistence is recent and the product is pre-release.
- Session creation UI flows ("New Chat" and "Open Workspace") must prompt for a directory via the native file picker. Cancelling the picker aborts Session creation.
- A single Code Workspace can still have multiple Sessions, but a Session cannot be rebound to a different workspace after creation.
