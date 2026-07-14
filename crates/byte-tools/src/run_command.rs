#![allow(unsafe_code)]

use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use futures::channel::mpsc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, ToolOutputStream, ToolStreamEvent};

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
/// Maximum number of bytes of combined stdout/stderr to capture.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// A tool that runs a non-interactive shell command and returns its output.
///
/// `stdout` and `stderr` are merged into a single string. The command is run
/// through `/bin/sh -c`. A default timeout of 30 seconds is enforced; on
/// timeout the child process and its process group are killed and reaped.
#[derive(Debug, Clone, Copy)]
pub struct RunCommandTool;

#[allow(clippy::too_many_lines)]
#[async_trait]
impl Tool for RunCommandTool {
    /// Return the protocol definition for this tool.
    fn definition(&self) -> byte_protocol::ToolDefinition {
        byte_protocol::ToolDefinition {
            name: "run_command".into(),
            description:
                "Run a non-interactive shell command and return its combined stdout/stderr output."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Optional working directory for the command. Relative paths are resolved against the workspace root; if omitted, the workspace root is used when available."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Maximum time to allow the command to run, in seconds. Defaults to 30.",
                        "minimum": 1
                    }
                },
                "required": ["command"]
            }),
        }
    }

    /// Invoke the tool and return a stream of stdout/stderr chunks.
    ///
    /// The returned stream emits each chunk as it arrives, followed by a
    /// [`ToolStreamEvent::Done`] event when the process exits. On timeout,
    /// cancellation, or output limit, the process group is killed and the
    /// final `Done` event contains an error result.
    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<ToolOutputStream, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::new("run_command cancelled"));
        }

        let command = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `command` argument"))?;

        let cwd = resolve_cwd(call, ctx);
        let timeout_seconds = resolve_timeout(call)?;
        let timeout = Duration::from_secs(timeout_seconds);

        let (tx, rx) = mpsc::unbounded();

        let mut std_cmd = std::process::Command::new("/bin/sh");
        // Run the command in a subshell and redirect stderr to stdout so the
        // returned string contains both streams merged together.
        let _ = std_cmd.arg("-c").arg(format!("({command}) 2>&1"));
        let _ = std_cmd.current_dir(&cwd);
        let _ = std_cmd.stdin(Stdio::null());
        let _ = std_cmd.stdout(Stdio::piped());

        // Place the child in its own process group so that timeouts and
        // cancellations can kill any grandchildren (e.g. `sleep` spawned by
        // the shell) without leaving zombies behind.
        #[cfg(unix)]
        unsafe {
            let _ = std_cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut cmd = Command::from(std_cmd);
        let mut child = cmd
            .spawn()
            .map_err(|error| ToolError::new(format!("failed to spawn command: {error}")))?;

        let pgid = child.id().map(u32::cast_signed);
        let cancel = cancel.clone();

        let _handle = tokio::spawn(async move {
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| ToolError::new("failed to capture command stdout"));
            let mut stdout = match stdout {
                Ok(s) => s,
                Err(error) => {
                    let _ = tx
                        .unbounded_send(Ok(ToolStreamEvent::done_error(&error)))
                        .ok();
                    return;
                }
            };

            let mut buf = Vec::with_capacity(4096);
            let mut chunk = [0u8; 8192];

            let reason = tokio::select! {
                () = cancel.cancelled() => Some("cancelled"),
                () = tokio::time::sleep(timeout) => Some("timed out"),
                result = read_streaming_output(&mut stdout, &tx, &mut buf, &mut chunk, MAX_OUTPUT_BYTES
                ) => {
                    match result {
                        Ok(()) => {}
                        Err(error) => {
                            kill_and_reap(&mut child, pgid).await;
                            let _ = tx.unbounded_send(Ok(ToolStreamEvent::done_error(&error))).ok();
                            return;
                        }
                    }
                    let output = String::from_utf8_lossy(&buf).into_owned();
                    let status = child.wait().await;
                    let _ = tx
                        .unbounded_send(Ok(ToolStreamEvent::Done {
                            result: finish_with_status(status, output),
                        }))
                        .ok();
                    return;
                }
                status = child.wait() => {
                    let output = match read_streaming_output(
                        &mut stdout, &tx, &mut buf, &mut chunk, MAX_OUTPUT_BYTES
                    ).await {
                        Ok(()) => String::from_utf8_lossy(&buf).into_owned(),
                        Err(error) => {
                            kill_and_reap(&mut child, pgid).await;
                            let _ = tx.unbounded_send(Ok(ToolStreamEvent::done_error(&error))).ok();
                            return;
                        }
                    };
                    let _ = tx
                        .unbounded_send(Ok(ToolStreamEvent::Done {
                            result: finish_with_status(status, output),
                        }))
                        .ok();
                    return;
                }
            };

            // Timeout or cancellation path: kill the process group and reap the
            // child so it does not become a zombie.
            kill_and_reap(&mut child, pgid).await;
            let _ = read_streaming_output(&mut stdout, &tx, &mut buf, &mut chunk, MAX_OUTPUT_BYTES)
                .await;

            let message = match reason {
                Some("cancelled") => "run_command cancelled".to_string(),
                Some("timed out") => {
                    format!("run_command timed out after {timeout_seconds} second(s)")
                }
                _ => "run_command terminated".to_string(),
            };
            let _ = tx
                .unbounded_send(Ok(ToolStreamEvent::Done {
                    result: crate::ToolOutputResult::error(message),
                }))
                .ok();
        });

        Ok(Box::pin(rx))
    }
}

/// Build a [`ToolOutputResult`] from the child's exit status and captured output.
fn finish_with_status(
    status: Result<std::process::ExitStatus, std::io::Error>,
    output: String,
) -> crate::ToolOutputResult {
    match status {
        Ok(status) if status.success() => {
            crate::ToolOutputResult::success_with_exit_code(output, status.code().unwrap_or(0))
        }
        Ok(status) => {
            let code = status
                .code()
                .map_or_else(|| "unknown".to_string(), |c| c.to_string());
            crate::ToolOutputResult::error_with_exit_code(
                format!("command exited with code {code}\n{output}"),
                status.code().unwrap_or(0),
            )
        }
        Err(error) => {
            crate::ToolOutputResult::error(format!("failed to wait for command: {error}\n{output}"))
        }
    }
}

/// Send `SIGKILL` to the process group identified by `pgid`, if any.
fn kill_process_group(pgid: Option<i32>) {
    if let Some(pgid) = pgid {
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// Kill the child process and its process group, then reap the child.
async fn kill_and_reap(child: &mut tokio::process::Child, pgid: Option<i32>) {
    kill_process_group(pgid);
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Read command output, emit each chunk through `tx`, and return the full output.
///
/// `buf` accumulates the complete output; `chunk` is the transient read buffer.
/// The function returns early if the output exceeds `limit` bytes.
async fn read_streaming_output(
    stdout: &mut tokio::process::ChildStdout,
    tx: &mpsc::UnboundedSender<Result<ToolStreamEvent, ToolError>>,
    buf: &mut Vec<u8>,
    chunk: &mut [u8; 8192],
    limit: usize,
) -> Result<(), ToolError> {
    loop {
        let n = stdout
            .read(chunk)
            .await
            .map_err(|error| ToolError::new(format!("failed to read command output: {error}")))?;
        if n == 0 {
            break;
        }
        if buf.len() + n > limit {
            return Err(ToolError::new(format!(
                "command output exceeded {limit} byte limit"
            )));
        }

        let text = String::from_utf8_lossy(&chunk[..n]).into_owned();
        let _ = tx
            .unbounded_send(Ok(ToolStreamEvent::Chunk { chunk: text }))
            .ok();

        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(())
}

/// Resolve the working directory for a `run_command` call.
fn resolve_cwd(call: &ToolCall, ctx: &SessionContext) -> PathBuf {
    let raw = call.arguments.get("cwd").and_then(|value| value.as_str());
    let path = match raw {
        Some(raw) => PathBuf::from(raw),
        None => {
            return ctx.workspace_root.clone();
        }
    };

    if path.is_absolute() {
        return path;
    }

    ctx.workspace_root.join(path)
}

/// Resolve the timeout, in seconds, for a `run_command` call.
fn resolve_timeout(call: &ToolCall) -> Result<u64, ToolError> {
    let seconds = call
        .arguments
        .get("timeout_seconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if seconds < 1 {
        return Err(ToolError::new("`timeout_seconds` must be at least 1"));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::pin::Pin;
    use std::time::Instant;

    use futures::Stream;
    use futures::StreamExt;

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};

    use crate::ToolOutputResult;

    fn call_with_args(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "run_command".into(),
            arguments: args,
        }
    }

    fn ctx_with_workspace(temp: &tempfile::TempDir) -> SessionContext {
        SessionContext {
            session_id: None,
            workspace_root: temp.path().to_path_buf(),
        }
    }

    /// Collect all chunks and the final result from a `run_command` stream.
    async fn collect_stream(
        stream: Pin<Box<dyn Stream<Item = Result<ToolStreamEvent, ToolError>> + Send>>,
    ) -> (Vec<String>, ToolOutputResult) {
        let mut chunks = Vec::new();
        let mut result = None;
        let mut stream = stream;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                ToolStreamEvent::Chunk { chunk } => chunks.push(chunk),
                ToolStreamEvent::Done { result: r } => result = Some(r),
            }
        }
        (chunks, result.expect("stream should end with Done"))
    }

    #[tokio::test]
    async fn echo_hello_returns_output_and_zero_exit() {
        let temp = tempfile::tempdir().unwrap();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo hello"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;
        assert_eq!(result.output, "hello\n");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn streaming_echo_emits_chunks_and_success_result() {
        let temp = tempfile::tempdir().unwrap();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo hello"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let (chunks, result) = collect_stream(stream).await;
        assert!(!chunks.is_empty(), "should emit at least one chunk");
        assert_eq!(chunks.concat(), "hello\n");
        assert_eq!(result.output, "hello\n");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn sleep_times_out_and_kills_child() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({
                    "command": "sleep 10",
                    "timeout_seconds": 1
                })),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;
        let elapsed = start.elapsed();

        assert!(result.is_error);
        assert!(
            result.output.contains("timed out"),
            "result should report a timeout"
        );
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3),
            "timeout should fire after about 1 second, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn cancelled_token_returns_cancel_error() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo hello"})),
                &ctx_with_workspace(&temp),
                &cancel,
            )
            .await;

        assert!(result.is_err());
        let error = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(
            error.contains("cancelled"),
            "error should report cancellation"
        );
        assert!(
            !error.contains("second(s)"),
            "cancellation error should not mention timeout duration"
        );
    }

    #[tokio::test]
    async fn zero_timeout_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({
                    "command": "echo hello",
                    "timeout_seconds": 0
                })),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(
            match result {
                Err(error) => error
                    .to_string()
                    .contains("timeout_seconds` must be at least 1"),
                Ok(_) => false,
            },
            "error should report invalid timeout"
        );
    }

    #[tokio::test]
    async fn no_timeout_uses_default_and_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "sleep 1"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;
        let elapsed = start.elapsed();

        assert!(
            !result.is_error,
            "command should succeed using default timeout"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "default timeout should allow short commands to finish, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn non_zero_exit_is_reported_as_error() {
        let temp = tempfile::tempdir().unwrap();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "exit 42"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;

        assert!(result.is_error);
        assert_eq!(result.exit_code, Some(42));
        assert!(
            result.output.contains("exited with code 42"),
            "output should contain exit code"
        );
    }

    #[tokio::test]
    async fn stderr_is_merged_into_output() {
        let temp = tempfile::tempdir().unwrap();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo out; echo err >&2"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;

        assert_eq!(result.output, "out\nerr\n");
    }

    #[tokio::test]
    async fn cwd_resolves_relative_to_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(temp.path().join("sub"))
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("sub/file.txt"), "nested")
            .await
            .unwrap();

        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({
                    "command": "cat file.txt",
                    "cwd": "sub"
                })),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;

        assert_eq!(result.output, "nested");
    }

    #[tokio::test]
    async fn missing_command_argument_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(
            match result {
                Err(error) => error.to_string().contains("missing `command` argument"),
                Ok(_) => false,
            },
            "error should report missing command"
        );
    }

    #[tokio::test]
    async fn stdin_is_null_so_cat_returns_eof_immediately() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "cat"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;
        let elapsed = start.elapsed();

        assert_eq!(result.output, "", "cat with no stdin should return EOF");
        assert!(
            elapsed < Duration::from_secs(2),
            "cat should not block waiting for stdin, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn excessive_output_is_rejected_with_limit_error() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let stream = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "yes"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (_chunks, result) = collect_stream(stream).await;
        let elapsed = start.elapsed();

        assert!(result.is_error);
        assert!(
            result.output.contains("exceeded 1048576 byte limit"),
            "error should report output limit exceeded"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "output limit should stop command quickly, took {elapsed:?}"
        );
    }
}
