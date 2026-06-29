use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub mod apply_patch;
pub mod find_files;
pub mod grep;
pub mod list_directory;
pub mod read_file;
pub mod registry;
pub mod run_command;
pub mod write_file;

pub use apply_patch::ApplyPatchTool;
pub use find_files::FindFilesTool;
pub use grep::GrepTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use registry::MvpToolRegistry;
pub use run_command::RunCommandTool;
pub use write_file::WriteFileTool;

/// An error produced while invoking a tool.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct ToolError {
    message: String,
}

impl ToolError {
    /// Create a new tool error with a user-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A tool that can be invoked by the runtime.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition;

    /// Invoke the tool with the given call and context.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError>;
}

/// A policy that decides whether a tool call is allowed.
pub trait ToolPolicy: Send + Sync {
    /// Check whether the tool call is allowed in the given context.
    ///
    /// # Errors
    ///
    /// Returns an error if the tool call is not allowed.
    fn check(&self, call: &ToolCall, ctx: &SessionContext) -> Result<(), ToolError>;
}

/// A policy that allows all tool calls.
#[derive(Debug, Clone, Copy)]
pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn check(&self, _call: &ToolCall, _ctx: &SessionContext) -> Result<(), ToolError> {
        Ok(())
    }
}

/// A registry of tools indexed by name.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Register a tool with the given name and policy.
    fn register(&mut self, name: String, tool: Arc<dyn Tool>, policy: Arc<dyn ToolPolicy>);

    /// Return the protocol definitions for all registered tools.
    fn definitions(&self) -> Vec<byte_protocol::ToolDefinition>;

    /// Return the names of all registered tools.
    fn names(&self) -> Vec<String>;

    /// Get a tool definition by name, if it exists.
    fn get(&self, name: &str) -> Option<(Arc<dyn Tool>, Arc<dyn ToolPolicy>)>;

    /// Invoke a tool call after checking its policy.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError>;
}

/// Resolve a `path` argument from a tool call against the session workspace root.
///
/// Absolute paths are returned as-is; relative paths are joined with the
/// workspace root when one is present.
// TODO(Slice 2+): The MVP runs in "unrestricted local agent mode" (see AGENTS.md).
// This helper does not normalize `..` components, so a relative path can escape
// the workspace root. Add a sandbox policy / allowlist before exposing the
// daemon to untrusted workspaces or models.
pub(crate) fn resolve_tool_path(
    call: &ToolCall,
    ctx: &SessionContext,
) -> Result<PathBuf, ToolError> {
    let raw = call
        .arguments
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ToolError::new("missing `path` argument"))?;

    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return Ok(path);
    }

    match &ctx.workspace_root {
        Some(root) => Ok(root.join(path)),
        None => Err(ToolError::new(
            "relative path requires a workspace root in the session context",
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    #[test]
    fn tool_error_preserves_message() {
        let error = ToolError::new("file not found");
        assert_eq!(error.to_string(), "file not found");
    }

    #[test]
    fn allow_all_policy_accepts_any_call() {
        let policy = AllowAllPolicy;
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };
        assert!(
            policy
                .check(
                    &call,
                    &SessionContext {
                        session_id: None,
                        workspace_root: None
                    }
                )
                .is_ok()
        );
    }

    #[tokio::test]
    async fn mvp_registry_invokes_read_file() {
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );

        assert!(registry.names().contains(&"read_file".to_string()));

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        tokio::fs::write(&path, "fn main() {}").await.unwrap();

        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "main.rs"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };
        let result = registry
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, "fn main() {}");
    }

    #[tokio::test]
    async fn mvp_registry_returns_error_for_missing_tool() {
        let registry = MvpToolRegistry::new();
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };
        let result = registry
            .invoke(
                &call,
                &SessionContext {
                    session_id: None,
                    workspace_root: None,
                },
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown tool"));
    }
}
