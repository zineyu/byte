#![allow(unsafe_code)]

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError};

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// A tool that runs a non-interactive shell command and returns its output.
///
/// `stdout` and `stderr` are merged into a single string. The command is run
/// through `/bin/sh -c`. A default timeout of 30 seconds is enforced; on
/// timeout the child process and its process group are killed and reaped.
#[derive(Debug, Clone, Copy)]
pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
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

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::new("run_command cancelled"));
        }

        let command = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `command` argument"))?;

        let cwd = resolve_cwd(call, ctx)?;
        let timeout_seconds = resolve_timeout(call)?;
        let timeout = Duration::from_secs(timeout_seconds);

        let mut std_cmd = std::process::Command::new("/bin/sh");
        // Run the command in a subshell and redirect stderr to stdout so the
        // returned string contains both streams merged together.
        let _ = std_cmd.arg("-c").arg(format!("({command}) 2>&1"));
        if let Some(cwd) = &cwd {
            let _ = std_cmd.current_dir(cwd);
        }
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

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::new("failed to capture command stdout"))?;
        let mut stdout_handle = tokio::spawn(read_limited_output(stdout, MAX_OUTPUT_BYTES));

        let reason = tokio::select! {
            () = cancel.cancelled() => Some("cancelled"),
            () = tokio::time::sleep(timeout) => Some("timed out"),
            output = &mut stdout_handle => {
                let output = output.map_err(|error| {
                    ToolError::new(format!("stdout reader panicked: {error}"))
                })?;
                match output {
                    Ok(output) => {
                        let status = child.wait().await;
                        return match status {
                            Ok(status) if status.success() => Ok(output),
                            Ok(status) => {
                                let code = status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string());
                                Err(ToolError::new(format!(
                                    "command exited with code {code}\n{output}"
                                )))
                            }
                            Err(error) => {
                                Err(ToolError::new(format!(
                                    "failed to wait for command: {error}\n{output}"
                                )))
                            }
                        };
                    }
                    Err(error) => {
                        kill_and_reap(&mut child, pgid).await;
                        return Err(error);
                    }
                }
            }
            status = child.wait() => {
                let output = (&mut stdout_handle).await.map_err(|error| {
                    ToolError::new(format!("stdout reader panicked: {error}"))
                })?;
                return match status {
                    Ok(status) if status.success() => Ok(output?),
                    Ok(status) => {
                        let code = status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string());
                        let output = output.unwrap_or_default();
                        Err(ToolError::new(format!(
                            "command exited with code {code}\n{output}"
                        )))
                    }
                    Err(error) => {
                        let output = output.unwrap_or_default();
                        Err(ToolError::new(format!(
                            "failed to wait for command: {error}\n{output}"
                        )))
                    }
                };
            }
        };

        // Timeout or cancellation path: kill the process group and reap the
        // child so it does not become a zombie.
        kill_and_reap(&mut child, pgid).await;
        let _ = stdout_handle.await;

        match reason {
            Some("cancelled") => Err(ToolError::new("run_command cancelled")),
            Some("timed out") => Err(ToolError::new(format!(
                "run_command timed out after {timeout_seconds} second(s)"
            ))),
            _ => Err(ToolError::new("run_command terminated")),
        }
    }
}

fn kill_process_group(pgid: Option<i32>) {
    if let Some(pgid) = pgid {
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

async fn kill_and_reap(child: &mut tokio::process::Child, pgid: Option<i32>) {
    kill_process_group(pgid);
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn read_limited_output(
    mut stdout: tokio::process::ChildStdout,
    limit: usize,
) -> Result<String, ToolError> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        let n = stdout
            .read(&mut chunk)
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
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn resolve_cwd(call: &ToolCall, ctx: &SessionContext) -> Result<Option<PathBuf>, ToolError> {
    let raw = call.arguments.get("cwd").and_then(|value| value.as_str());
    let path = match raw {
        Some(raw) => PathBuf::from(raw),
        None => {
            return Ok(ctx.workspace_root.clone());
        }
    };

    if path.is_absolute() {
        return Ok(Some(path));
    }

    match &ctx.workspace_root {
        Some(root) => Ok(Some(root.join(path))),
        None => Err(ToolError::new(
            "relative cwd requires a workspace root in the session context",
        )),
    }
}

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

    use super::*;
    use byte_protocol::{SessionContext, ToolCall};
    use std::time::Instant;

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
            workspace_root: Some(temp.path().to_path_buf()),
        }
    }

    #[tokio::test]
    async fn echo_hello_returns_output_and_zero_exit() {
        let temp = tempfile::tempdir().unwrap();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo hello"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.unwrap(), "hello\n");
    }

    #[tokio::test]
    async fn sleep_times_out_and_kills_child() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({
                    "command": "sleep 10",
                    "timeout_seconds": 1
                })),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("timed out"),
            "error should report a timeout"
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
        let error = result.unwrap_err().to_string();
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
            result
                .unwrap_err()
                .to_string()
                .contains("timeout_seconds` must be at least 1"),
            "error should report invalid timeout"
        );
    }

    #[tokio::test]
    async fn no_timeout_uses_default_and_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "sleep 1"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;
        let elapsed = start.elapsed();

        assert!(
            result.is_ok(),
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
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "exit 42"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exited with code 42"),
            "error should contain exit code"
        );
    }
    #[tokio::test]
    async fn stderr_is_merged_into_output() {
        let temp = tempfile::tempdir().unwrap();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "echo out; echo err >&2"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), "out\nerr\n");
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

        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({
                    "command": "cat file.txt",
                    "cwd": "sub"
                })),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), "nested");
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
            result
                .unwrap_err()
                .to_string()
                .contains("missing `command` argument"),
            "error should report missing command"
        );
    }

    #[tokio::test]
    async fn stdin_is_null_so_cat_returns_eof_immediately() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "cat"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;
        let elapsed = start.elapsed();

        assert_eq!(result.unwrap(), "", "cat with no stdin should return EOF");
        assert!(
            elapsed < Duration::from_secs(2),
            "cat should not block waiting for stdin, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn excessive_output_is_rejected_with_limit_error() {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let result = RunCommandTool
            .invoke(
                &call_with_args(serde_json::json!({"command": "yes"})),
                &ctx_with_workspace(&temp),
                &CancellationToken::new(),
            )
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeded 1048576 byte limit"),
            "error should report output limit exceeded"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "output limit should stop command quickly, took {elapsed:?}"
        );
    }
}
