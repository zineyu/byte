# Use unrestricted local agent mode for the MVP

Byte Agent's MVP will not enforce runtime permission filtering for tool calls: file access, file mutation, command execution, deletion, and network-capable commands are treated as allowed in the local development environment. This is intentionally unsafe as a product default, but reduces MVP scope while preserving an architectural seam for a future ApprovalPolicy or ToolPolicy implementation to replace the initial AllowAllPolicy.
