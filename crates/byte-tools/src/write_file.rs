use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use std::path::Path;
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, resolve_tool_path, unified_diff};

/// A tool that creates or overwrites a file relative to the workspace root.
#[derive(Debug, Clone, Copy)]
pub struct WriteFileTool;
// TODO(Slice 2+): The MVP runs in "unrestricted local agent mode" (see AGENTS.md).
// write_file currently resolves any absolute path without a sandbox policy or
// path-allowlist, and does not normalize `..` components (so paths may escape
// the workspace). Add a proper sandbox policy before exposing the daemon to
// untrusted workspaces or models.

#[async_trait]
impl Tool for WriteFileTool {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "write_file".into(),
            description: "Create or overwrite a file at the given path with the given content."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the file to write."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file."
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    /// Invoke the tool with the given call and context.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        // Mirror the bounded-write policy used by `read_file`/`apply_patch` so
        // that a model cannot accidentally write unbounded content.
        const MAX_SIZE: usize = 1024 * 1024; // 1 MiB

        if cancel.is_cancelled() {
            return Err(ToolError::new("write_file cancelled"));
        }

        let path = resolve_tool_path(call, ctx)?;
        let content = call
            .arguments
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `content` argument"))?;

        if content.len() > MAX_SIZE {
            return Err(ToolError::new(format!(
                "content exceeds size limit of {MAX_SIZE} bytes"
            )));
        }

        if let Some(parent) = path.parent() {
            match tokio::fs::try_exists(parent).await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(ToolError::new(format!(
                        "parent directory does not exist: {}",
                        parent.display()
                    )));
                }
                Err(error) => {
                    return Err(ToolError::new(format!(
                        "failed to check parent directory {}: {}",
                        parent.display(),
                        error
                    )));
                }
            }
        }

        // Capture original content for the diff before the file is overwritten.
        // If the existing file is too large to diff, skip reading it so the
        // memory footprint stays bounded; the write itself still proceeds.
        let original_content = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.len() > MAX_SIZE as u64 => None,
            _ => tokio::fs::read_to_string(&path).await.ok(),
        };

        // Write to a temporary file in the same directory and atomically rename
        // it over the target. If the rename fails the original file remains intact.
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temp_name = format!(".{file_name}.tmp.{}", uuid::Uuid::new_v4());
        let temp_path = parent.join(temp_name);

        // Preserve the original file's permissions so the temporary file does
        // not replace them with default filesystem permissions after rename.
        let original_permissions = tokio::fs::metadata(&path)
            .await
            .map(|meta| meta.permissions())
            .ok();

        if let Err(error) = tokio::fs::write(&temp_path, content).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to write temporary file {}: {}",
                temp_path.display(),
                error
            )));
        }

        if let Some(perms) = original_permissions
            && let Err(error) = tokio::fs::set_permissions(&temp_path, perms).await
        {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to set permissions on {}: {}",
                temp_path.display(),
                error
            )));
        }

        if let Err(error) = tokio::fs::rename(&temp_path, &path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to write {}: {}",
                path.display(),
                error
            )));
        }

        let diff = unified_diff(&path, original_content.as_deref(), content);

        Ok(format!(
            "wrote {} bytes to {}\n\n{}",
            content.len(),
            path.display(),
            diff
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn call(path: &str, content: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({"path": path, "content": content}),
        }
    }

    #[tokio::test]
    async fn writes_file_relative_to_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        let result = WriteFileTool
            .invoke(
                &call("hello.txt", "Hello, world!"),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let path = temp.path().join("hello.txt");
        assert!(result.starts_with("wrote 13 bytes to"));
        assert!(result.contains(&path.display().to_string()));
        assert!(result.contains("--- /dev/null"));
        assert!(result.contains(&format!("+++ {}", path.display())));
        assert!(result.contains("+Hello, world!"));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hello.txt");
        tokio::fs::write(&path, "old content").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        let result = WriteFileTool
            .invoke(
                &call("hello.txt", "new content"),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.starts_with("wrote 11 bytes to"));
        assert!(result.contains("---"));
        assert!(result.contains("-old content"));
        assert!(result.contains("+new content"));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn skips_diff_for_oversized_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("huge.txt");
        // Existing file is larger than the 1 MiB diff cap.
        let oversized = "x".repeat(1024 * 1024 + 1);
        tokio::fs::write(&path, &oversized).await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        let result = WriteFileTool
            .invoke(
                &call("huge.txt", "new content"),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.starts_with("wrote 11 bytes to"));
        // Without the original content, the diff is treated as a new file.
        assert!(result.contains("--- /dev/null"));
        assert!(!result.contains("-xxxx"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn returns_error_when_parent_directory_missing() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        let result = WriteFileTool
            .invoke(
                &call("missing/dir/hello.txt", "content"),
                &ctx,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("parent directory"));
        assert!(!temp.path().join("missing").exists());
    }

    #[tokio::test]
    async fn returns_error_for_missing_path_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({"content": "content"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: PathBuf::from("/tmp"),
        };

        let result = WriteFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_content_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({"path": "hello.txt"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: PathBuf::from("/tmp"),
        };

        let result = WriteFileTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("content"));
    }

    #[tokio::test]
    async fn returns_error_when_content_exceeds_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };
        // 1 MiB + 1 byte, which exceeds the tool's hard-coded size limit.
        let oversized = "x".repeat(1024 * 1024 + 1);

        let result = WriteFileTool
            .invoke(
                &call("huge.txt", &oversized),
                &ctx,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("size limit"),
            "expected size limit error, got {message}"
        );
        assert!(!temp.path().join("huge.txt").exists());
    }

    #[tokio::test]
    async fn writes_file_atomically_without_leaving_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        WriteFileTool
            .invoke(
                &call("hello.txt", "Hello, world!"),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(temp.path().join("hello.txt"))
            .await
            .unwrap();
        assert_eq!(content, "Hello, world!");

        let mut entries = tokio::fs::read_dir(temp.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert_eq!(
                entry.path(),
                temp.path().join("hello.txt"),
                "unexpected file left in temp dir: {}",
                entry.path().display()
            );
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn preserves_unix_file_permissions_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("script.sh");
        tokio::fs::write(&path, "#!/bin/sh\necho old\n")
            .await
            .unwrap();

        let mut perms = tokio::fs::metadata(&path).await.unwrap().permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&path, perms).await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        };

        WriteFileTool
            .invoke(
                &call("script.sh", "#!/bin/sh\necho new\n"),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let final_mode = tokio::fs::metadata(&path)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(final_mode & 0o777, 0o755);
    }
}
