use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use regex::Regex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::{
    Tool, ToolError, ToolOutputStream, ToolStreamEvent, resolve_tool_path, single_event_stream,
};

/// A tool that recursively searches file contents for a regular expression.
#[derive(Debug, Clone, Copy)]
pub struct GrepTool;

/// Maximum file size in bytes that `grep` will read.
const MAX_SIZE: u64 = 1024 * 1024; // 1 MiB

#[async_trait]
impl Tool for GrepTool {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "grep".into(),
            description: "Recursively search file contents for a regular expression pattern."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression pattern to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the directory to search."
                    }
                },
                "required": ["pattern", "path"]
            }),
        }
    }

    /// Invoke the tool and return its output as a single-event stream.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<ToolOutputStream, ToolError> {
        match self.grep(call, ctx, cancel).await {
            Ok(output) => Ok(single_event_stream(Ok(ToolStreamEvent::done(output)))),
            Err(error) => Ok(single_event_stream(Ok(ToolStreamEvent::done_error(&error)))),
        }
    }
}

impl GrepTool {
    /// Execute the tool's core logic.
    async fn grep(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::new("grep cancelled"));
        }

        let pattern = call
            .arguments
            .get("pattern")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `pattern` argument"))?;

        let regex = Regex::new(pattern)
            .map_err(|error| ToolError::new(format!("invalid regex pattern: {error}")))?;

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
                    "failed to check path {}: {}",
                    path.display(),
                    error
                )));
            }
        }
        let cancel = cancel.clone();
        let search_path = path.clone();
        let (mut matches, warnings) =
            tokio::task::spawn_blocking(move || grep_blocking(&regex, &search_path, &cancel))
                .await
                .map_err(|error| ToolError::new(format!("grep task failed: {error}")))??;

        matches.sort_by(|a: &serde_json::Value, b: &serde_json::Value| {
            let path_a = a["path"].as_str().unwrap_or("");
            let path_b = b["path"].as_str().unwrap_or("");
            let line_a = a["line"].as_u64().unwrap_or(0);
            let line_b = b["line"].as_u64().unwrap_or(0);
            path_a.cmp(path_b).then(line_a.cmp(&line_b))
        });

        let output = serde_json::json!({ "matches": matches, "warnings": warnings });
        serde_json::to_string_pretty(&output)
            .map_err(|error| ToolError::new(format!("failed to serialize grep results: {error}")))
    }
}

/// Search files under `path` for `regex` synchronously, returning matches and warnings.
fn grep_blocking(
    regex: &Regex,
    path: &std::path::Path,
    cancel: &CancellationToken,
) -> Result<(Vec<serde_json::Value>, Vec<String>), ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::new("grep cancelled"));
    }

    let mut matches = Vec::new();
    let mut warnings = Vec::new();

    let walker = walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(50)
        .into_iter()
        .filter_map(|result| match result {
            Ok(entry) => Some(entry),
            Err(error) => {
                warn!(%error, "failed to traverse directory entry; skipping");
                None
            }
        });

    for entry in walker {
        if cancel.is_cancelled() {
            return Err(ToolError::new("grep cancelled"));
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path().to_path_buf();

        let metadata = match std::fs::metadata(&file_path) {
            Ok(meta) => meta,
            Err(error) => {
                warn!(
                    path = %file_path.display(),
                    %error,
                    "failed to read file metadata; skipping"
                );
                continue;
            }
        };
        if metadata.len() > MAX_SIZE {
            warnings.push(format!(
                "skipped {}: file exceeds size limit of {} bytes",
                strip_base(path, &file_path).display(),
                MAX_SIZE
            ));
            continue;
        }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(error) => {
                warn!(
                    path = %file_path.display(),
                    %error,
                    "failed to read file contents; skipping"
                );
                continue;
            }
        };

        let relative_path = strip_base(path, &file_path);
        for (line_number, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                matches.push(serde_json::json!({
                    "path": relative_path.display().to_string(),
                    "line": line_number + 1,
                    "content": line,
                }));
            }
        }
    }

    Ok((matches, warnings))
}

/// Return `path` with `base` stripped, or `path` itself if it is not under `base`.
fn strip_base(base: &std::path::Path, path: &std::path::Path) -> std::path::PathBuf {
    path.strip_prefix(base).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn call(pattern: &str, path: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "grep".into(),
            arguments: serde_json::json!({"pattern": pattern, "path": path}),
        }
    }

    #[tokio::test]
    async fn finds_matches_recursively() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::create_dir(root.join("src")).await.unwrap();
        tokio::fs::write(root.join("a.txt"), "apple\nbanana\n")
            .await
            .unwrap();
        tokio::fs::write(root.join("src/b.rs"), "fn banana() {}\n")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: root.to_path_buf(),
        };

        let result = GrepTool
            .grep(&call("banana", "."), &ctx, &CancellationToken::new())
            .await
            .expect("grep succeeds");

        let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        let matches = output["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0]["content"], "banana");
        assert_eq!(matches[0]["path"], "a.txt");
        assert_eq!(matches[1]["content"], "fn banana() {}");
        assert_eq!(matches[1]["path"], "src/b.rs");
        assert!(output["warnings"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn skips_oversized_files() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        let oversized = vec![b'x'; 1024 * 1024 + 1];
        tokio::fs::write(root.join("big.log"), oversized)
            .await
            .unwrap();
        tokio::fs::write(root.join("small.txt"), "needle\n")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: root.to_path_buf(),
        };

        let result = GrepTool
            .grep(&call("needle", "."), &ctx, &CancellationToken::new())
            .await
            .expect("grep succeeds");

        let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        let matches = output["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["path"], "small.txt");
        let warnings = output["warnings"].as_array().expect("warnings array");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].as_str().unwrap().contains("big.log"));
        assert!(warnings[0].as_str().unwrap().contains("size limit"));
    }

    #[tokio::test]
    async fn returns_error_for_invalid_regex() {
        let ctx = SessionContext {
            session_id: None,
            workspace_root: tempdir().unwrap().path().to_path_buf(),
        };
        let err = GrepTool
            .grep(&call("[", "."), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("invalid regex pattern"));
    }

    #[tokio::test]
    async fn returns_error_when_directory_does_not_exist() {
        let ctx = SessionContext {
            session_id: None,
            workspace_root: PathBuf::from("/nonexistent/workspace"),
        };
        let err = GrepTool
            .grep(&call("foo", "missing"), &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_pattern_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "grep".into(),
            arguments: serde_json::json!({"path": "."}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: tempdir().unwrap().path().to_path_buf(),
        };
        let err = GrepTool
            .grep(&call, &ctx, &CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("missing `pattern` argument"));
    }

    #[tokio::test]
    async fn returns_cancel_error_from_spawn_blocking() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        tokio::fs::write(root.join("x.txt"), "needle\n")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: root.to_path_buf(),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = GrepTool
            .grep(&call("needle", "."), &ctx, &cancel)
            .await
            .expect_err("should be cancelled");
        assert!(err.to_string().contains("grep cancelled"));
    }
}
