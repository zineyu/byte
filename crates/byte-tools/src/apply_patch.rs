use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, resolve_tool_path};
use std::path::Path;

/// A tool that applies multiple search/replace patches to a file.
#[derive(Debug, Clone, Copy)]
pub struct ApplyPatchTool;
// TODO(Slice 2+): The MVP runs in "unrestricted local agent mode" (see AGENTS.md).
// apply_patch currently resolves any absolute path without a sandbox policy or
// path-allowlist, and does not normalize `..` components (so paths may escape
// the workspace). A 1 MiB size cap and atomic rename are enforced to limit
// accidental damage; add a proper sandbox policy before exposing the daemon to
// untrusted workspaces or models.

#[async_trait]
impl Tool for ApplyPatchTool {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "apply_patch".into(),
            description: "Apply multiple search/replace patches to a file.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the file to patch."
                    },
                    "patch": {
                        "type": "array",
                        "description": "Array of search/replace patch objects.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "search": {
                                    "type": "string",
                                    "description": "Text to search for."
                                },
                                "replace": {
                                    "type": "string",
                                    "description": "Replacement text."
                                }
                            },
                            "required": ["search", "replace"]
                        }
                    }
                },
                "required": ["path", "patch"]
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
        // Mirror the bounded-read policy used by `read_file` so that applying a
        // patch cannot unboundedly grow memory.
        const MAX_SIZE: u64 = 1024 * 1024; // 1 MiB

        if cancel.is_cancelled() {
            return Err(ToolError::new("apply_patch cancelled"));
        }

        let path = resolve_tool_path(call, ctx)?;
        let patches = parse_patches(call)?;

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

        if cancel.is_cancelled() {
            return Err(ToolError::new("apply_patch cancelled"));
        }

        let content = read_bounded(&mut file, MAX_SIZE, cancel, &path).await?;

        let modified = apply_patches(content, &patches, MAX_SIZE, cancel, &path)?;

        if modified.len() as u64 > MAX_SIZE {
            return Err(ToolError::new(format!(
                "patched output for {} exceeds size limit of {} bytes",
                path.display(),
                MAX_SIZE
            )));
        }

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
        let original_metadata = tokio::fs::metadata(&path).await.map_err(|error| {
            ToolError::new(format!(
                "failed to read metadata for {}: {}",
                path.display(),
                error
            ))
        })?;
        let original_permissions = original_metadata.permissions();

        if let Err(error) = tokio::fs::write(&temp_path, &modified).await {
            // Best-effort cleanup; the original file is untouched because the
            // write did not complete.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to write temporary file {}: {}",
                temp_path.display(),
                error
            )));
        }

        if let Err(error) = tokio::fs::set_permissions(&temp_path, original_permissions).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to set permissions on {}: {}",
                temp_path.display(),
                error
            )));
        }

        if let Err(error) = tokio::fs::rename(&temp_path, &path).await {
            // Best-effort cleanup; the original file is untouched because the
            // rename did not complete.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ToolError::new(format!(
                "failed to write {}: {}",
                path.display(),
                error
            )));
        }

        Ok(format!(
            "applied {} patch(es) to {}",
            patches.len(),
            path.display()
        ))
    }
}

/// A single search/replace patch.
#[derive(Debug)]
struct Patch {
    /// Text to search for in the file.
    search: String,
    /// Text to replace the search text with.
    replace: String,
}

/// Read `reader` into a string, aborting if the contents exceed `max_size`.
async fn read_bounded<R>(
    reader: &mut R,
    max_size: u64,
    cancel: &CancellationToken,
    path: &Path,
) -> Result<String, ToolError>
where
    R: AsyncRead + Unpin,
{
    const CHUNK_SIZE: usize = 8192;
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; CHUNK_SIZE];

    loop {
        if cancel.is_cancelled() {
            return Err(ToolError::new("apply_patch cancelled"));
        }
        let n = reader.read(&mut chunk[..]).await.map_err(|error| {
            ToolError::new(format!("failed to read {}: {}", path.display(), error))
        })?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() as u64 > max_size {
            return Err(ToolError::new(format!(
                "file {} exceeds size limit of {} bytes",
                path.display(),
                max_size
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

/// Apply `patches` to `content` in order, enforcing `max_size` and cancellation.
fn apply_patches(
    mut content: String,
    patches: &[Patch],
    max_size: u64,
    cancel: &CancellationToken,
    path: &Path,
) -> Result<String, ToolError> {
    for (index, patch) in patches.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(ToolError::new("apply_patch cancelled"));
        }
        if patch.search.is_empty() {
            return Err(ToolError::new(format!(
                "patch {}: search text must not be empty",
                index + 1
            )));
        }
        if !content.contains(&patch.search) {
            return Err(ToolError::new(format!(
                "patch {}: search text not found in {}",
                index + 1,
                path.display()
            )));
        }
        content = content.replace(&patch.search, &patch.replace);
        if content.len() as u64 > max_size {
            return Err(ToolError::new(format!(
                "patch {}: patched output for {} exceeds size limit of {} bytes",
                index + 1,
                path.display(),
                max_size
            )));
        }
    }
    Ok(content)
}

/// Parse the `patch` argument from `call` into a list of [`Patch`]es.
fn parse_patches(call: &ToolCall) -> Result<Vec<Patch>, ToolError> {
    let patches_value = call
        .arguments
        .get("patch")
        .ok_or_else(|| ToolError::new("missing `patch` argument"))?;

    let patches_array = patches_value
        .as_array()
        .ok_or_else(|| ToolError::new("`patch` must be a JSON array"))?;

    patches_array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let search = value
                .get("search")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::new(format!("patch {}: missing `search`", index + 1)))?;
            let replace = value
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::new(format!("patch {}: missing `replace`", index + 1)))?;
            Ok(Patch {
                search: search.to_owned(),
                replace: replace.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;
    use tokio_util::sync::CancellationToken;

    fn call(path: &str, patch_arg: &serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path": path, "patch": patch_arg}),
        }
    }

    #[tokio::test]
    async fn applies_single_patch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "fn old() {}").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([{"search": "old", "replace": "new"}]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result, format!("applied 1 patch(es) to {}", path.display()));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn new() {}");
    }

    #[tokio::test]
    async fn applies_multiple_patches_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("src/lib.rs");
        tokio::fs::create_dir(temp.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(&path, "fn old_one() {}\nfn old_two() {}\n")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        ApplyPatchTool
            .invoke(
                &call(
                    "src/lib.rs",
                    &serde_json::json!([
                        {"search": "fn old_one() {}", "replace": "fn new_one() {}"},
                        {"search": "fn old_two() {}", "replace": "fn new_two() {}"}
                    ]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn new_one() {}\nfn new_two() {}\n");
    }

    #[tokio::test]
    async fn returns_error_and_leaves_file_unchanged_when_search_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "fn original() {}").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([
                        {"search": "fn original() {}", "replace": "fn changed() {}"},
                        {"search": "missing", "replace": "found"}
                    ]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("patch 2"));
        assert!(message.contains("search text not found"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn original() {}");
    }

    #[tokio::test]
    async fn returns_error_for_empty_search() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "fn original() {}").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([{"search": "", "replace": "x"}]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn original() {}");
    }

    #[tokio::test]
    async fn returns_error_for_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "missing.rs",
                    &serde_json::json!([{"search": "x", "replace": "y"}]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing.rs"));
    }

    #[tokio::test]
    async fn returns_error_for_missing_patch_argument() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path": "lib.rs"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(PathBuf::from("/tmp")),
        };

        let result = ApplyPatchTool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("patch"));
    }

    #[tokio::test]
    async fn applies_patch_atomically_without_leaving_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "fn old() {}").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([{"search": "old", "replace": "new"}]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result, format!("applied 1 patch(es) to {}", path.display()));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn new() {}");

        let mut entries = tokio::fs::read_dir(temp.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert_eq!(
                entry.path(),
                path,
                "unexpected file left in temp dir: {}",
                entry.path().display()
            );
        }
    }

    #[tokio::test]
    async fn returns_error_when_file_exceeds_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("huge.rs");
        // 1 MiB + 1 byte, which exceeds the tool's hard-coded size limit.
        let oversized = vec![b'x'; 1024 * 1024 + 1];
        tokio::fs::write(&path, &oversized).await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        let result = ApplyPatchTool
            .invoke(
                &call(
                    "huge.rs",
                    &serde_json::json!([{"search": "x", "replace": "y"}]),
                ),
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

        let content = tokio::fs::read(&path).await.unwrap();
        assert_eq!(content.len(), oversized.len());
    }

    #[tokio::test]
    async fn returns_error_when_patched_output_exceeds_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("small.rs");
        tokio::fs::write(&path, "x").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        // A small input patched to 1 MiB + 1 byte must be rejected based on the
        // output size, even though the input itself is well under the limit.
        let huge_replace = "x".repeat(1024 * 1024 + 1);
        let result = ApplyPatchTool
            .invoke(
                &call(
                    "small.rs",
                    &serde_json::json!([{"search": "x", "replace": huge_replace}]),
                ),
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

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "x");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn preserves_unix_file_permissions() {
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
            workspace_root: Some(temp.path().to_path_buf()),
        };

        ApplyPatchTool
            .invoke(
                &call(
                    "script.sh",
                    &serde_json::json!([{"search": "old", "replace": "new"}]),
                ),
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

    #[tokio::test]
    async fn replaces_all_occurrences_per_patch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "fn old() {}\nfn old() {}\nfn old() {}\n")
            .await
            .unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([{"search": "old", "replace": "new"}]),
                ),
                &ctx,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn new() {}\nfn new() {}\nfn new() {}\n");
    }

    #[tokio::test]
    async fn rejects_file_that_grows_past_size_limit_during_read() {
        /// A reader that reports a small initial payload and then a payload
        /// large enough to exceed the 1 MiB cap. This simulates the TOCTOU
        /// window where a file grows after its metadata was checked.
        struct GrowingReader {
            state: u8,
            payload: Vec<u8>,
            offset: usize,
        }

        impl AsyncRead for GrowingReader {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                match self.state {
                    0 => {
                        self.state = 1;
                        self.payload = vec![b'x'; 1024 * 1024 + 1];
                        buf.put_slice(b"start");
                    }
                    1 => {
                        let remaining = buf.remaining();
                        let end = (self.offset + remaining).min(self.payload.len());
                        buf.put_slice(&self.payload[self.offset..end]);
                        self.offset = end;
                        if self.offset >= self.payload.len() {
                            self.state = 2;
                        }
                    }
                    _ => {}
                }
                Poll::Ready(Ok(()))
            }
        }

        let mut reader = GrowingReader {
            state: 0,
            payload: Vec::new(),
            offset: 0,
        };
        let result = read_bounded(
            &mut reader,
            1024 * 1024,
            &CancellationToken::new(),
            Path::new("growing.txt"),
        )
        .await;

        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("size limit"),
            "expected size limit error, got {message}"
        );
    }

    #[tokio::test]
    async fn rejects_intermediate_output_that_exceeds_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lib.rs");
        tokio::fs::write(&path, "a").await.unwrap();

        let ctx = SessionContext {
            session_id: None,
            workspace_root: Some(temp.path().to_path_buf()),
        };

        // The first patch inflates the content beyond the 1 MiB limit; the
        // second patch would shrink it back down. The tool must reject the
        // intermediate oversized result rather than silently succeed.
        let huge = "x".repeat(1024 * 1024 + 1);
        let result = ApplyPatchTool
            .invoke(
                &call(
                    "lib.rs",
                    &serde_json::json!([
                        {"search": "a", "replace": &huge},
                        {"search": &huge, "replace": "b"}
                    ]),
                ),
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

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "a");
    }
}
