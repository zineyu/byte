use std::path::PathBuf;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError};

/// A tool that reads the contents of a file relative to the workspace root.
pub struct ReadFileTool;
// TODO(Slice 2+): The MVP runs in "unrestricted local agent mode" (see AGENTS.md).
// read_file currently resolves any absolute path without a sandbox policy or
// path-allowlist. A 1 MiB size cap is enforced to avoid accidental DoS from
// large files; add a proper sandbox policy before exposing the daemon to
// untrusted workspaces or models.

#[async_trait]
impl Tool for ReadFileTool {
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "read_file".into(),
            description: "Read the contents of a file at the given path.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the file to read."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        // Cancellation is only checked before I/O begins. tokio::fs::read_to_string
        // is not cancellable mid-read; for very large files this can delay
        // shutdown. Slice 2+ can switch to a bounded, cancellation-aware reader.
        const MAX_SIZE: u64 = 1024 * 1024; // 1 MiB
        let path = resolve_path(call, ctx)?;
        if cancel.is_cancelled() {
            return Err(ToolError::new("read_file cancelled"));
        }
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.len() > MAX_SIZE => Err(ToolError::new(format!(
                "file {} exceeds size limit of {} bytes",
                path.display(),
                MAX_SIZE
            ))),
            _ => tokio::fs::read_to_string(&path).await.map_err(|error| {
                ToolError::new(format!("failed to read {}: {}", path.display(), error))
            }),
        }
    }
}

fn resolve_path(call: &ToolCall, ctx: &SessionContext) -> Result<PathBuf, ToolError> {
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
    use super::*;
    use byte_protocol::{SessionContext, ToolCall};

    #[tokio::test]
    async fn reads_file_relative_to_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("src/main.rs");
        tokio::fs::create_dir_all(temp.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(&path, "fn main() {}").await.unwrap();

        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };
        let ctx = SessionContext {
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, "fn main() {}");
    }

    #[tokio::test]
    async fn reads_file_with_absolute_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absolute.rs");
        tokio::fs::write(&path, "const X: u32 = 1;").await.unwrap();

        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": path.to_str().unwrap()}),
        };
        let ctx = SessionContext {
            workspace_root: Some(PathBuf::from("/other")),
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, "const X: u32 = 1;");
    }

    #[tokio::test]
    async fn returns_error_for_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "missing.rs"}),
        };
        let ctx = SessionContext {
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing.rs"));
    }

    #[tokio::test]
    async fn returns_error_for_relative_path_without_workspace() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };
        let ctx = SessionContext {
            workspace_root: None,
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("workspace root"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_path_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        };
        let ctx = SessionContext {
            workspace_root: Some(PathBuf::from("/tmp")),
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path"));
    }

    #[tokio::test]
    async fn returns_error_when_file_exceeds_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("huge.rs");
        // 1 MiB + 1 byte, which exceeds the tool's hard-coded size limit.
        let oversized = vec![b'x'; 1024 * 1024 + 1];
        tokio::fs::write(&path, oversized).await.unwrap();

        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": path.to_str().unwrap()}),
        };
        let ctx = SessionContext {
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;
        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("size limit"),
            "expected size limit error, got {message}"
        );
    }
}
