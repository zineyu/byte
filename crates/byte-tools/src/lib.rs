//! Tool implementations and registries for the byte runtime.
//!
//! This crate provides concrete tools for filesystem access, search, and
//! command execution, plus a registry trait and a simple in-memory
//! implementation used in the MVP.
#![deny(rustdoc::broken_intra_doc_links)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Apply search/replace patches to files.
pub mod apply_patch;

/// Find files matching glob patterns.
pub mod find_files;

/// Recursively search file contents with regular expressions.
pub mod grep;

/// List directory entries.
pub mod list_directory;

/// Diff generation for file-editing tools.
pub mod diff;

/// Read file contents.
pub mod read_file;

/// Tool registries and the MVP implementation.
pub mod registry;

/// Run non-interactive shell commands.
pub mod run_command;

/// Create or overwrite files.
pub mod write_file;

/// Re-export of [`ApplyPatchTool`].
pub use apply_patch::ApplyPatchTool;

/// Re-export of [`unified_diff`].
pub use diff::unified_diff;

/// Re-export of [`FindFilesTool`].
pub use find_files::FindFilesTool;

/// Re-export of [`GrepTool`].
pub use grep::GrepTool;

/// Re-export of [`ListDirectoryTool`].
pub use list_directory::ListDirectoryTool;

/// Re-export of [`ReadFileTool`].
pub use read_file::ReadFileTool;

/// Re-export of [`MvpToolRegistry`].
pub use registry::MvpToolRegistry;

/// Re-export of [`RunCommandTool`].
pub use run_command::RunCommandTool;

/// Re-export of [`WriteFileTool`].
pub use write_file::WriteFileTool;

/// An error produced while invoking a tool.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct ToolError {
    /// User-readable error message.
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

/// The final result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputResult {
    /// Serialized output from the tool.
    pub output: String,
    /// Exit code returned by the tool, if any.
    pub exit_code: Option<i32>,
    /// Whether the tool call should be treated as an error.
    pub is_error: bool,
}

impl ToolOutputResult {
    /// Create a successful result with the given output.
    #[must_use]
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            exit_code: None,
            is_error: false,
        }
    }

    /// Create a successful result with the given output and exit code.
    #[must_use]
    pub fn success_with_exit_code(output: impl Into<String>, exit_code: i32) -> Self {
        Self {
            output: output.into(),
            exit_code: Some(exit_code),
            is_error: false,
        }
    }

    /// Create an error result with the given message.
    #[must_use]
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            exit_code: None,
            is_error: true,
        }
    }

    /// Create an error result with the given message and exit code.
    #[must_use]
    pub fn error_with_exit_code(output: impl Into<String>, exit_code: i32) -> Self {
        Self {
            output: output.into(),
            exit_code: Some(exit_code),
            is_error: true,
        }
    }
}

/// A streaming output event produced by a tool during execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutputEvent {
    /// A chunk of incremental output.
    Chunk {
        /// Incremental output chunk.
        chunk: String,
    },
}

/// A sink for streaming tool output events.
#[async_trait]
pub trait ToolEventSink: Send + Sync {
    /// Emit a streaming tool output event.
    async fn emit(&self, event: ToolOutputEvent);
}

/// A no-op sink for tests and tools that do not stream output.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

#[async_trait]
impl ToolEventSink for NoopEventSink {
    async fn emit(&self, _event: ToolOutputEvent) {}
}

/// A tool that can be invoked by the runtime.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition;

    /// Invoke the tool with the given call and context.
    ///
    /// This is the non-streaming entry point used by tests and simple tools.
    /// The runtime calls [`Self::invoke_with_sink`] so that tools can emit
    /// incremental output while they run.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError>;

    /// Invoke the tool with a streaming event sink.
    ///
    /// The default implementation calls [`Self::invoke`] and emits no
    /// streaming events. Tools that produce incremental output (such as
    /// `run_command`) should override this method.
    async fn invoke_with_sink(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
        sink: Arc<dyn ToolEventSink>,
    ) -> Result<ToolOutputResult, ToolError> {
        let _ = sink;
        let output = self.invoke(call, ctx, cancel).await?;
        Ok(ToolOutputResult::success(output))
    }
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
    /// Allow every tool call.
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

    /// Get a tool and its policy by name, if registered.
    fn get(&self, name: &str) -> Option<(Arc<dyn Tool>, Arc<dyn ToolPolicy>)>;

    /// Invoke a tool call after checking its policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the tool is unknown, the policy rejects the call,
    /// or the tool invocation fails.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
        sink: Arc<dyn ToolEventSink>,
    ) -> Result<ToolOutputResult, ToolError>;
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

    Ok(ctx.workspace_root.join(path))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;

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
                        workspace_root: tempfile::tempdir().unwrap().path().to_path_buf()
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
            workspace_root: temp.path().to_path_buf(),
        };
        let result = registry
            .invoke(
                &call,
                &ctx,
                &CancellationToken::new(),
                Arc::new(NoopEventSink),
            )
            .await
            .unwrap();
        assert_eq!(result.output, "fn main() {}");
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
                    workspace_root: tempfile::tempdir().unwrap().path().to_path_buf(),
                },
                &CancellationToken::new(),
                Arc::new(NoopEventSink),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown tool"));
    }
}
