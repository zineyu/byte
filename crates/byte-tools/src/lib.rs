use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub mod read_file;
pub mod registry;

pub use read_file::ReadFileTool;
pub use registry::MvpToolRegistry;

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
    fn check(&self, call: &ToolCall, ctx: &SessionContext) -> Result<(), ToolError>;
}

/// A policy that allows all tool calls.
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

#[cfg(test)]
mod tests {
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
                    workspace_root: None,
                },
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown tool"));
    }
}
