use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError};

/// A tool that reads the contents of a file relative to the workspace root.
#[derive(Debug, Clone, Copy)]
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
        const MAX_SIZE: u64 = 1024 * 1024; // 1 MiB
        const CHUNK_SIZE: usize = 8192;

        let path = crate::resolve_tool_path(call, ctx)?;
        if cancel.is_cancelled() {
            return Err(ToolError::new("read_file cancelled"));
        }

        let mut file = tokio::fs::File::open(&path).await.map_err(|error| {
            ToolError::new(format!("failed to open {}: {}", path.display(), error))
        })?;

        let metadata = file.metadata().await.map_err(|error| {
            ToolError::new(format!(
                "failed to read metadata for {}: {}",
                path.display(),
                error
            ))
        })?;
        if metadata.len() > MAX_SIZE {
            return Err(ToolError::new(format!(
                "file {} exceeds size limit of {} bytes",
                path.display(),
                MAX_SIZE
            )));
        }

        let mut buffer = Vec::with_capacity(4096);
        let mut chunk = [0u8; CHUNK_SIZE];

        loop {
            if cancel.is_cancelled() {
                return Err(ToolError::new("read_file cancelled"));
            }

            let n = file.read(&mut chunk[..]).await.map_err(|error| {
                ToolError::new(format!("failed to read {}: {}", path.display(), error))
            })?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
            if buffer.len() as u64 > MAX_SIZE {
                return Err(ToolError::new(format!(
                    "file {} exceeds size limit of {} bytes",
                    path.display(),
                    MAX_SIZE
                )));
            }
        }

        String::from_utf8(buffer).map_err(|error| {
            ToolError::new(format!(
                "file {} is not valid UTF-8: {}",
                path.display(),
                error
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;

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
            session_id: None,
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
            session_id: None,
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
            session_id: None,
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
            session_id: None,
            workspace_root: None,
        };

        let result = ReadFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("workspace root"));
    }

    #[tokio::test]
    async fn returns_error_when_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.rs");
        tokio::fs::write(&path, "fn main() {}").await.unwrap();

        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": path.to_str().unwrap()}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = ReadFileTool
            .invoke(&call, &ctx, &cancel)
            .await
            .expect_err("should be cancelled");
        assert!(err.to_string().contains("read_file cancelled"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_path_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        };
        let ctx = SessionContext {
            session_id: None,
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
            session_id: None,
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
