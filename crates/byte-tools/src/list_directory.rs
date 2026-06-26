use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, resolve_tool_path};

/// A tool that lists the entries in a directory.
pub struct ListDirectoryTool;

#[async_trait]
impl Tool for ListDirectoryTool {
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "list_directory".into(),
            description: "List the files and subdirectories in a directory.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the directory to list."
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
        if cancel.is_cancelled() {
            return Err(ToolError::new("list_directory cancelled"));
        }

        let path = resolve_tool_path(call, ctx)?;

        match tokio::fs::try_exists(&path).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(ToolError::new(format!(
                    "directory does not exist: {}",
                    path.display()
                )));
            }
            Err(error) => {
                return Err(ToolError::new(format!(
                    "failed to check directory {}: {}",
                    path.display(),
                    error
                )));
            }
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&path).await.map_err(|error| {
            ToolError::new(format!(
                "failed to read directory {}: {}",
                path.display(),
                error
            ))
        })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(|error| {
            ToolError::new(format!(
                "failed to read directory entry in {}: {}",
                path.display(),
                error
            ))
        })? {
            if cancel.is_cancelled() {
                return Err(ToolError::new("list_directory cancelled"));
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = match entry.file_type().await {
                Ok(file_type) if file_type.is_dir() => "directory",
                Ok(file_type) if file_type.is_symlink() => "symlink",
                _ => "file",
            };
            entries.push(serde_json::json!({"name": name, "type": kind}));
        }

        entries.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        serde_json::to_string_pretty(&entries).map_err(|error| {
            ToolError::new(format!("failed to serialize directory entries: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn call(path: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "list_directory".into(),
            arguments: serde_json::json!({"path": path}),
        }
    }

    #[tokio::test]
    async fn lists_files_and_directories() {
        let dir = tempdir().expect("temp dir");
        tokio::fs::create_dir(dir.path().join("src")).await.unwrap();
        tokio::fs::write(dir.path().join("README.md"), "# hi")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(dir.path().to_path_buf()),
        };

        let result = ListDirectoryTool
            .invoke(&call("."), &ctx, &CancellationToken::new())
            .await
            .expect("list succeeds");

        let entries: Vec<serde_json::Value> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["name"], "README.md");
        assert_eq!(entries[0]["type"], "file");
        assert_eq!(entries[1]["name"], "src");
        assert_eq!(entries[1]["type"], "directory");
    }

    #[tokio::test]
    async fn lists_absolute_path() {
        let dir = tempdir().expect("temp dir");
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let result = ListDirectoryTool
            .invoke(
                &ToolCall {
                    id: "call-1".into(),
                    name: "list_directory".into(),
                    arguments: serde_json::json!({"path": dir.path().to_str().unwrap()}),
                },
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .expect("list succeeds");

        let entries: Vec<serde_json::Value> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "a.txt");
        assert_eq!(entries[0]["type"], "file");
    }

    #[tokio::test]
    async fn returns_error_when_path_does_not_exist() {
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(PathBuf::from("/nonexistent/workspace")),
        };
        let err = ListDirectoryTool
            .invoke(&call("missing"), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn returns_error_when_cancelled() {
        let dir = tempdir().expect("temp dir");
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(dir.path().to_path_buf()),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = ListDirectoryTool
            .invoke(&call("."), &ctx, &cancel)
            .await
            .expect_err("should be cancelled");
        assert!(err.to_string().contains("list_directory cancelled"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_path_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "list_directory".into(),
            arguments: serde_json::json!({}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let err = ListDirectoryTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("missing `path` argument"));
    }
}
