use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, resolve_tool_path};

/// Maximum number of file paths returned by `find_files`.
const MAX_RESULTS: usize = 10_000;

/// A tool that finds files matching a glob pattern.
#[derive(Debug, Clone, Copy)]
pub struct FindFilesTool;

#[async_trait]
impl Tool for FindFilesTool {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "find_files".into(),
            description: "Find files matching a glob pattern under a directory.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern such as '*.rs' or 'src/**/*.txt'."
                    },
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the directory to search. Defaults to the workspace root."
                    }
                },
                "required": ["pattern"]
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
        if cancel.is_cancelled() {
            return Err(ToolError::new("find_files cancelled"));
        }

        let pattern = call
            .arguments
            .get("pattern")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `pattern` argument"))?;

        let base_path = match call.arguments.get("path").and_then(|value| value.as_str()) {
            Some(_) => resolve_tool_path(call, ctx)?,
            None => ctx.workspace_root.clone().ok_or_else(|| {
                ToolError::new("missing workspace root; provide a `path` argument")
            })?,
        };

        match tokio::fs::try_exists(&base_path).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(ToolError::new(format!(
                    "directory does not exist: {}",
                    base_path.display()
                )));
            }
            Err(error) => {
                return Err(ToolError::new(format!(
                    "failed to check path {}: {}",
                    base_path.display(),
                    error
                )));
            }
        }

        let pattern_string = build_glob_pattern(&base_path, pattern)?;

        let cancel = cancel.clone();
        let base = base_path.clone();
        let matches = tokio::task::spawn_blocking(move || -> Result<Vec<String>, ToolError> {
            if cancel.is_cancelled() {
                return Err(ToolError::new("find_files cancelled"));
            }

            let glob = glob::glob(&pattern_string)
                .map_err(|error| ToolError::new(format!("invalid glob pattern: {error}")))?;

            let mut matches = Vec::new();
            #[cfg(unix)]
            let mut visited = HashSet::new();

            for entry in glob {
                if cancel.is_cancelled() {
                    return Err(ToolError::new("find_files cancelled"));
                }

                match entry {
                    Ok(path) if path.is_file() => {
                        #[cfg(unix)]
                        if let Ok(meta) = std::fs::metadata(&path) {
                            let key = (meta.dev(), meta.ino());
                            if !visited.insert(key) {
                                continue;
                            }
                        }

                        if matches.len() >= MAX_RESULTS {
                            return Err(ToolError::new(
                                "find_files result limit exceeded".to_string(),
                            ));
                        }

                        let relative = strip_base(&base, &path);
                        matches.push(relative.display().to_string());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return Err(ToolError::new(format!(
                            "failed to read glob entry: {error}"
                        )));
                    }
                }
            }

            matches.sort();
            Ok(matches)
        })
        .await
        .map_err(|error| ToolError::new(format!("find_files task failed: {error}")))??;

        serde_json::to_string_pretty(&matches).map_err(|error| {
            ToolError::new(format!("failed to serialize find_files results: {error}"))
        })
    }
}

/// Build a glob pattern string from a base directory and a user pattern.
fn build_glob_pattern(base: &Path, pattern: &str) -> Result<String, ToolError> {
    let base_str = base
        .to_str()
        .ok_or_else(|| ToolError::new("search path contains invalid UTF-8"))?;

    let normalized = if let Some(stripped) = pattern.strip_prefix('/') {
        stripped
    } else {
        pattern
    };

    let escaped_base = glob::Pattern::escape(base_str);
    Ok(format!("{escaped_base}/{normalized}"))
}

/// Return `path` with `base` stripped, or `path` itself if it is not under `base`.
fn strip_base(base: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(base).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn call(pattern: &str, path: Option<&str>) -> ToolCall {
        let mut arguments = serde_json::json!({"pattern": pattern});
        if let Some(path) = path {
            arguments["path"] = serde_json::Value::String(path.into());
        }
        ToolCall {
            id: "call-1".into(),
            name: "find_files".into(),
            arguments,
        }
    }

    #[tokio::test]
    async fn finds_files_with_glob_pattern() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::create_dir(root.join("src")).await.unwrap();
        tokio::fs::write(root.join("a.rs"), "").await.unwrap();
        tokio::fs::write(root.join("src/b.rs"), "").await.unwrap();
        tokio::fs::write(root.join("a.txt"), "").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };

        let result = FindFilesTool
            .invoke(&call("**/*.rs", Some(".")), &ctx, &CancellationToken::new())
            .await
            .expect("find_files succeeds");

        let matches: Vec<String> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&"a.rs".to_string()));
        assert!(matches.contains(&"src/b.rs".to_string()));
    }

    #[tokio::test]
    async fn defaults_to_workspace_root_when_path_is_omitted() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::write(root.join("config.toml"), "")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };

        let result = FindFilesTool
            .invoke(&call("*.toml", None), &ctx, &CancellationToken::new())
            .await
            .expect("find_files succeeds");

        let matches: Vec<String> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(matches, vec!["config.toml"]);
    }

    #[tokio::test]
    async fn returns_error_when_workspace_root_missing_and_no_path_given() {
        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let err = FindFilesTool
            .invoke(&call("*.rs", None), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("missing workspace root"));
    }

    #[tokio::test]
    async fn returns_error_when_directory_does_not_exist() {
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(PathBuf::from("/nonexistent/workspace")),
        };
        let err = FindFilesTool
            .invoke(&call("*.rs", Some(".")), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn returns_error_for_invalid_glob_pattern() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };
        let err = FindFilesTool
            .invoke(&call("[", Some(".")), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("invalid glob pattern"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_pattern_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "find_files".into(),
            arguments: serde_json::json!({}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let err = FindFilesTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("missing `pattern` argument"));
    }

    #[tokio::test]
    async fn uses_absolute_path_without_workspace() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::write(root.join("x.rs"), "").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let result = FindFilesTool
            .invoke(
                &call("*.rs", Some(root.to_str().unwrap())),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .expect("find_files succeeds");

        let matches: Vec<String> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(matches, vec!["x.rs"]);
    }

    #[tokio::test]
    async fn escapes_glob_metacharacters_in_base_path() {
        let dir = tempdir().expect("temp dir");
        // Create a directory whose name contains glob metacharacters.
        let root = dir.path().join("[root]");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("file.rs"), "").unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };
        let result = FindFilesTool
            .invoke(
                &call("*.rs", Some(root.to_str().unwrap())),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .expect("find_files succeeds");

        let matches: Vec<String> = serde_json::from_str(&result).expect("valid json");
        assert_eq!(matches, vec!["file.rs"]);
    }

    #[tokio::test]
    async fn returns_error_when_result_limit_exceeded() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        for i in 0..10_001 {
            std::fs::write(root.join(format!("file{i}.txt")), "").unwrap();
        }

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };
        let err = FindFilesTool
            .invoke(&call("*.txt", Some(".")), &ctx, &CancellationToken::new())
            .await
            .expect_err("should exceed result limit");
        assert!(err.to_string().contains("result limit exceeded"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn does_not_loop_on_cyclic_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "").unwrap();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        symlink(root, sub.join("loop")).unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };
        let result = FindFilesTool
            .invoke(
                &call("**/*.txt", Some(".")),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .expect("find_files should complete without looping");

        let matches: Vec<String> = serde_json::from_str(&result).expect("valid json");
        assert!(matches.contains(&"a.txt".to_string()));
        // Without loop detection the symlink could produce the same file many
        // times, eventually hitting the result limit.
        assert!(matches.len() < 10_000);
    }

    #[tokio::test]
    async fn returns_cancel_error_from_spawn_blocking() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::write(root.join("x.rs"), "").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(root.to_path_buf()),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = FindFilesTool
            .invoke(&call("*.rs", Some(".")), &ctx, &cancel)
            .await
            .expect_err("should be cancelled");
        assert!(err.to_string().contains("find_files cancelled"));
    }
}
