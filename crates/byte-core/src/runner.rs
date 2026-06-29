use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use byte_models::{ProviderError, ProviderEvent, ProviderStream};
use byte_protocol::{
    ActivatedSkill, CancelRunResult, CompactionSummary, MessageRole, RunMessage, RunStatus,
    RuntimeEventKind, SendMessageParams, SessionContext, SessionMessage, ToolCall, ToolDefinition,
};
use byte_session::SessionError;
use byte_tools::AllowAllPolicy;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

use crate::prompt::{PromptBuilder, PromptContext};
use crate::runtime_services::RuntimeServices;
/// Buffers small provider deltas so that a run can be cancelled cleanly: the
/// cancellation can flush any remaining content as a final `message_delta`
/// before emitting `run_cancelled`.
#[derive(Debug)]
pub struct DeltaBuffer {
    /// Character threshold that triggers a buffer flush.
    threshold: usize,
    /// Accumulated delta content waiting to be flushed.
    buffer: String,
}

impl DeltaBuffer {
    /// Create a new buffer with the given character threshold.
    #[must_use]
    pub const fn new(threshold: usize) -> Self {
        Self {
            threshold,
            buffer: String::new(),
        }
    }

    /// Append a delta to the buffer.
    ///
    /// Returns `Some(flush)` when the buffered content reaches the threshold.
    /// The returned string contains all buffered content and the buffer is
    /// reset.
    pub fn push(&mut self, delta: &str) -> Option<String> {
        self.buffer.push_str(delta);
        if self.buffer.len() >= self.threshold {
            Some(std::mem::take(&mut self.buffer))
        } else {
            None
        }
    }

    /// Force-flush any remaining buffered content.
    ///
    /// Returns `None` if the buffer is empty.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }
}

/// Identifier for an in-progress run within a session.
pub type RunId = String;

/// Errors that can occur when starting or executing a run.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// The session already has an active run.
    #[error("session is busy with another run")]
    Busy,
    /// An error originating from the session store.
    #[error(transparent)]
    SessionStore(#[from] SessionError),
    /// An error originating from the model provider.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Owns the conversation loop for a single session.
///
/// A runner instance enforces the "one active run per session" constraint and
/// emits lifecycle runtime events during the run.
#[derive(Clone, Debug)]
pub struct SessionRunner {
    /// Aggregated runtime services used by the runner.
    services: RuntimeServices,
    /// Skills currently activated for this session.
    active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
    /// Optional in-progress run id and its cancellation token.
    active_run: Arc<Mutex<Option<(RunId, CancellationToken)>>>,
}

impl SessionRunner {
    /// Create a new runner with aggregated runtime services.
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        let active_skills = Arc::new(Mutex::new(Vec::new()));
        let tool_registry = Arc::new(crate::activate_skill::SessionToolRegistry::new(
            Arc::clone(&services.tool_registry),
            Arc::new(crate::activate_skill::ActivateSkillTool::new(
                Arc::clone(&services.skill_registry),
                Arc::clone(&active_skills),
            )),
            Arc::new(AllowAllPolicy),
        ));
        let mut services = services;
        services.tool_registry = tool_registry;
        Self {
            services,
            active_skills,
            active_run: Arc::new(Mutex::new(None)),
        }
    }

    /// Start a single-turn model run.
    ///
    /// Returns immediately with the run id; the run itself is executed on a
    /// background task.
    ///
    /// # Errors
    ///
    /// Returns an error if a run is already active for this session or the
    /// session store cannot be accessed.
    pub async fn send_message(&self, params: SendMessageParams) -> Result<RunId, RunnerError> {
        let mut active = self.active_run.lock().await;
        if active.is_some() {
            return Err(RunnerError::Busy);
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        let _ = active.replace((run_id.clone(), token.clone()));
        drop(active);

        let view = match self.services.store.load_session(&params.session_id).await {
            Ok(view) => view,
            Err(error) => {
                self.clear_active_run().await;
                return Err(RunnerError::SessionStore(error));
            }
        };
        let parent_id = view.messages.last().map(|message| message.id.clone());

        let developer_message_id = match self
            .services
            .store
            .append_message(
                &params.session_id,
                None,
                parent_id.as_deref(),
                MessageRole::Developer,
                &params.message,
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(error) => {
                self.clear_active_run().await;
                return Err(RunnerError::SessionStore(error));
            }
        };

        let runner = Arc::new(self.clone());
        let executor = RunExecutor {
            run_id: run_id.clone(),
            session_id: params.session_id,
            message: params.message,
            developer_message_id,
            history: view.messages,
            compactions: view.compactions,
            workspace_instructions: view.workspace_instructions,
            cancel_token: token.child_token(),
        };

        let _handle = tokio::spawn(async move {
            executor.run(runner).await;
        });

        Ok(run_id)
    }

    /// Cancel the active run for this session, if any.
    ///
    /// This is an idempotent no-op when there is no active run. When a run is
    /// active, the cancellation token is triggered and the caller waits until
    /// the run task has cleaned up `active_run` before returning.
    ///
    /// # Errors
    ///
    /// Returns an error if the runner cannot determine whether a run is active.
    pub async fn cancel_run(&self) -> Result<CancelRunResult, RunnerError> {
        let token = {
            let active = self.active_run.lock().await;
            active.as_ref().map(|(_, token)| token.clone())
        };

        if let Some(token) = token {
            token.cancel();
            // Wait until the run task clears `active_run`. The task always
            // clears it, so a busy-wait with a short sleep is sufficient.
            loop {
                let active = self.active_run.lock().await;
                if active.is_none() {
                    break;
                }
                drop(active);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }

        Ok(CancelRunResult {})
    }

    /// Return true if the runner currently has an active run.
    pub async fn is_running(&self) -> bool {
        self.active_run.lock().await.is_some()
    }

    /// Acquire the active-run mutex guard.
    ///
    /// The guard is used by `SessionManager::delete_session` to hold the lock
    /// across the file deletion so that no run can start on this session while
    /// the session file is being removed.
    pub(crate) async fn active_run_guard(
        &self,
    ) -> tokio::sync::MutexGuard<'_, Option<(RunId, CancellationToken)>> {
        self.active_run.lock().await
    }

    /// Wait until there is no active run.
    ///
    /// Useful in tests to observe the full event sequence.
    #[cfg(test)]
    pub async fn wait_until_idle(&self) {
        loop {
            let active = self.active_run.lock().await;
            if active.is_none() {
                return;
            }
            drop(active);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Emit a runtime event.
    async fn emit(&self, kind: RuntimeEventKind) {
        self.services.event_bus.emit(kind).await;
    }

    /// Mark the runner as having no active run.
    async fn clear_active_run(&self) {
        *self.active_run.lock().await = None;
    }

    /// Emit error and finished events for a failed run.
    #[instrument(skip_all, fields(run_id))]
    async fn emit_run_error(&self, run_id: &RunId, message: String) {
        error!(%run_id, %message, "run failed");
        self.emit(RuntimeEventKind::Error {
            run_id: Some(run_id.to_owned()),
            message: message.clone(),
        })
        .await;
        self.emit(RuntimeEventKind::RunFinished {
            run_id: run_id.to_owned(),
            status: RunStatus::Failed,
            error: Some(message),
        })
        .await;
    }
}

/// One-shot executor for a single provider run.
struct RunExecutor {
    /// Identifier for this run.
    run_id: RunId,
    /// Identifier for the session being run.
    session_id: String,
    /// User message for this run.
    message: String,
    /// Stable id assigned to the user/developer message.
    developer_message_id: String,
    /// Prior session messages.
    history: Vec<SessionMessage>,
    /// Summaries of compacted conversation ranges.
    compactions: Vec<CompactionSummary>,
    /// Raw content of the workspace's AGENTS.md instruction file, if found.
    workspace_instructions: Option<String>,
    /// Token used to cancel this run.
    cancel_token: CancellationToken,
}

/// Mutable state carried through a single provider response stream.
struct StreamState<'a> {
    /// Optional id for the assistant message being built.
    message_id: &'a mut Option<String>,
    /// Accumulated assistant message content.
    assistant_content: &'a mut String,
    /// Tool calls extracted from the stream.
    tool_calls: &'a mut Option<Vec<ToolCall>>,
    /// Whether the stream has produced a complete response.
    completed: &'a mut bool,
    /// Buffer for small provider deltas.
    delta_buffer: &'a mut DeltaBuffer,
}

impl RunExecutor {
    /// Execute the run loop.
    #[instrument(skip_all, fields(run_id, session_id))]
    async fn run(self, runner: Arc<SessionRunner>) {
        let _ = tracing::Span::current().record("run_id", &self.run_id);
        let _ = tracing::Span::current().record("session_id", &self.session_id);
        info!("starting run");

        runner
            .emit(RuntimeEventKind::RunStarted {
                session_id: self.session_id.clone(),
                run_id: self.run_id.clone(),
            })
            .await;

        let Some(session_ctx) = self.load_session_context(&runner).await else {
            return;
        };

        let tools = runner.services.tool_registry.definitions();
        let active_skills = runner.active_skills.lock().await.clone();
        let Some(available_skills) = self.load_available_skills(&runner, &session_ctx).await else {
            return;
        };
        let prompt_context = PromptContext {
            user_message: self.message.clone(),
            history: self.history.clone(),
            compactions: self.compactions.clone(),
            tools,
            active_skills,
            available_skills: available_skills.clone(),
            workspace_instructions: self.workspace_instructions.clone(),
        };
        let mut messages = PromptBuilder::new().build(prompt_context);
        let mut turn_messages: Vec<RunMessage> = Vec::new();
        // ID of the most recently persisted entry in this run. Used to chain
        // assistant messages and tool results into a single linear history.
        let mut last_entry_id = self.developer_message_id.clone();
        let mut saw_message = false;

        loop {
            if self.cancel_token.is_cancelled() {
                self.finish_cancelled(&runner, saw_message).await;
                return;
            }

            if !turn_messages.is_empty() {
                // Subsequent request after tool calls: append the last turn's
                // assistant message and tool results to the conversation.
                messages.extend(turn_messages);
                turn_messages = Vec::new();
            }
            // Rebuild the system prompt on every turn so skills activated
            // mid-run are reflected in subsequent provider requests. Available
            // skills are cached from the initial turn; they are stable for a run.
            let tools = runner.services.tool_registry.definitions();
            let active_skills = runner.active_skills.lock().await.clone();
            let available_skills = available_skills.clone();
            if !messages.is_empty() && messages[0].role == MessageRole::System {
                messages[0].content =
                    PromptBuilder::build_system_prompt(&tools, &active_skills, &available_skills);
            }
            let Some(mut stream) = self
                .request_provider_stream(&runner, messages.clone(), tools)
                .await
            else {
                return;
            };

            let Some((assistant_content, tool_calls)) = self
                .process_provider_stream(&runner, &mut stream, &mut last_entry_id, &mut saw_message)
                .await
            else {
                return;
            };

            let Some(calls) = tool_calls else {
                self.finish_succeeded(&runner).await;
                return;
            };

            if self.cancel_token.is_cancelled() {
                self.finish_cancelled(&runner, true).await;
                return;
            }

            turn_messages.push(RunMessage {
                role: MessageRole::Assistant,
                content: assistant_content,
                tool_call_id: None,
                tool_calls: Some(calls.clone()),
            });

            for call in calls {
                if self
                    .execute_tool_call(
                        &runner,
                        &call,
                        &session_ctx,
                        &mut last_entry_id,
                        &mut turn_messages,
                    )
                    .await
                    .is_none()
                {
                    return;
                }
            }
        }
    }

    /// Loads the available skill catalog for the current workspace.
    /// Returns `None` after emitting a run error and clearing the active run.
    async fn load_available_skills(
        &self,
        runner: &Arc<SessionRunner>,
        session_ctx: &SessionContext,
    ) -> Option<Vec<byte_protocol::SkillEntry>> {
        match runner
            .services
            .skill_registry
            .catalog(Some(session_ctx.workspace_root.as_path()))
            .await
        {
            Ok(skills) => Some(skills),
            Err(error) => {
                error!(%error, "failed to load skill catalog");
                runner
                    .emit_run_error(
                        &self.run_id,
                        format!("failed to load skill catalog: {error}"),
                    )
                    .await;
                runner.clear_active_run().await;
                None
            }
        }
    }

    /// Loads the session view and builds the runtime context for tool calls.
    /// Returns `None` after emitting a run error and clearing the active run.
    async fn load_session_context(&self, runner: &Arc<SessionRunner>) -> Option<SessionContext> {
        match runner.services.store.load_session(&self.session_id).await {
            Ok(view) => Some(SessionContext {
                session_id: Some(self.session_id.clone()),
                workspace_root: PathBuf::from(view.workspace),
            }),
            Err(error) => {
                error!(%error, "failed to load session view");
                runner.emit_run_error(&self.run_id, error.to_string()).await;
                runner.clear_active_run().await;
                None
            }
        }
    }

    /// Sends the current conversation to the provider and returns the response
    /// stream, or `None` after emitting a run error and clearing the active run.
    async fn request_provider_stream(
        &self,
        runner: &Arc<SessionRunner>,
        messages: Vec<RunMessage>,
        tools: Vec<ToolDefinition>,
    ) -> Option<ProviderStream> {
        match runner.services.provider.send_message(messages, tools).await {
            Ok(stream) => Some(stream),
            Err(error) => {
                error!(%error, "provider request failed");
                runner.emit_run_error(&self.run_id, error.to_string()).await;
                runner.clear_active_run().await;
                None
            }
        }
    }

    /// Consumes a provider stream, emits message lifecycle events, persists the
    /// assistant message, and returns its content plus any tool calls. Returns
    /// `None` if the run finished (success, cancellation, or error) inside the
    /// stream loop.
    async fn process_provider_stream(
        &self,
        runner: &Arc<SessionRunner>,
        stream: &mut ProviderStream,
        last_entry_id: &mut String,
        saw_message: &mut bool,
    ) -> Option<(String, Option<Vec<ToolCall>>)> {
        let mut message_id: Option<String> = None;
        let mut assistant_content = String::new();
        let mut tool_calls: Option<Vec<ToolCall>> = None;
        let mut completed = false;
        let mut delta_buffer = DeltaBuffer::new(8);

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    if let Some(id) = message_id.as_ref() {
                        if let Some(flush) = delta_buffer.flush() {
                            runner.emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id.clone(),
                                delta: flush,
                            }).await;
                        }
                        runner.emit(RuntimeEventKind::RunCancelled {
                            run_id: self.run_id.clone(),
                        }).await;
                    }
                    self.finish_cancelled(runner, false).await;
                    return None;
                }
                maybe_event = stream.next() => {
                    if let Some(event) = maybe_event {
                        self.handle_provider_event(
                            runner,
                            event,
                            &mut StreamState {
                                message_id: &mut message_id,
                                assistant_content: &mut assistant_content,
                                tool_calls: &mut tool_calls,
                                completed: &mut completed,
                                delta_buffer: &mut delta_buffer,
                            },
                            last_entry_id,
                            saw_message,
                        )
                        .await?;
                    } else {
                        break;
                    }
                }
            }
        }

        if !completed {
            runner
                .emit_run_error(
                    &self.run_id,
                    "provider stream ended without completing the assistant message".into(),
                )
                .await;
            runner.clear_active_run().await;
            return None;
        }

        Some((assistant_content, tool_calls))
    }

    /// Handles a single provider event inside the stream loop. Updates the
    /// per-turn state and returns `None` when the run should finish.
    async fn handle_provider_event(
        &self,
        runner: &Arc<SessionRunner>,
        event: Result<ProviderEvent, ProviderError>,
        state: &mut StreamState<'_>,
        last_entry_id: &mut String,
        saw_message: &mut bool,
    ) -> Option<()> {
        match event {
            Ok(ProviderEvent::MessageStarted { message_id: id }) => {
                debug!(message_id = %id, "assistant message started");
                *saw_message = true;
                state.message_id.clone_from(&Some(id.clone()));
                state.assistant_content.clear();
                *state.tool_calls = None;
                *state.completed = false;
                runner
                    .emit(RuntimeEventKind::MessageStarted {
                        run_id: self.run_id.clone(),
                        message_id: id,
                        role: MessageRole::Assistant,
                    })
                    .await;
            }
            Ok(ProviderEvent::TextDelta {
                message_id: id,
                delta,
            }) => {
                if state.message_id.as_ref() == Some(&id) {
                    state.assistant_content.push_str(&delta);
                    if let Some(flush) = state.delta_buffer.push(&delta) {
                        runner
                            .emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id,
                                delta: flush,
                            })
                            .await;
                    }
                }
            }
            Ok(ProviderEvent::MessageCompleted {
                message_id: id,
                tool_calls: calls,
            }) => {
                if state.message_id.as_ref() == Some(&id) {
                    if let Some(flush) = state.delta_buffer.flush() {
                        runner
                            .emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id.clone(),
                                delta: flush,
                            })
                            .await;
                    }
                    runner
                        .emit(RuntimeEventKind::MessageCompleted {
                            run_id: self.run_id.clone(),
                            message_id: id.clone(),
                            tool_calls: calls.clone(),
                        })
                        .await;

                    if let Err(error) = runner
                        .services
                        .store
                        .append_message(
                            &self.session_id,
                            Some(&id),
                            Some(last_entry_id),
                            MessageRole::Assistant,
                            &**state.assistant_content,
                            calls.clone(),
                        )
                        .await
                    {
                        error!(%error, "failed to persist assistant message");
                        runner
                            .emit_run_error(
                                &self.run_id,
                                format!("failed to persist assistant message: {error}"),
                            )
                            .await;
                        runner.clear_active_run().await;
                        return None;
                    }
                    last_entry_id.clone_from(&id);

                    *state.tool_calls = calls;
                    *state.completed = true;
                }
            }
            Err(error) => {
                error!(%error, "provider stream error");
                runner.emit_run_error(&self.run_id, error.to_string()).await;
                runner.clear_active_run().await;
                return None;
            }
        }

        Some(())
    }

    /// Executes a single tool call, emits its lifecycle events, persists the
    /// result, and appends a tool-result message to the turn. Returns `None`
    /// after emitting a run error and clearing the active run.
    async fn execute_tool_call(
        &self,
        runner: &Arc<SessionRunner>,
        call: &ToolCall,
        session_ctx: &SessionContext,
        last_entry_id: &mut String,
        turn_messages: &mut Vec<RunMessage>,
    ) -> Option<()> {
        runner
            .emit(RuntimeEventKind::ToolStarted {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
            })
            .await;

        let (output, is_error) = match runner
            .services
            .tool_registry
            .invoke(call, session_ctx, &self.cancel_token)
            .await
        {
            Ok(output) => (output, false),
            Err(error) => (error.to_string(), true),
        };

        runner
            .emit(RuntimeEventKind::ToolFinished {
                tool_call_id: call.id.clone(),
                output: output.clone(),
                is_error,
            })
            .await;

        let tool_result_id = match runner
            .services
            .store
            .append_tool_result(&self.session_id, None, last_entry_id, &call.id, &output)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                error!(%error, "failed to persist tool result");
                runner
                    .emit_run_error(
                        &self.run_id,
                        format!("failed to persist tool result: {error}"),
                    )
                    .await;
                runner.clear_active_run().await;
                return None;
            }
        };
        last_entry_id.clone_from(&tool_result_id);

        turn_messages.push(RunMessage::tool_result(&call.id, output));
        Some(())
    }

    /// Emits a successful run-finished event and clears the active run.
    async fn finish_succeeded(&self, runner: &Arc<SessionRunner>) {
        runner
            .emit(RuntimeEventKind::RunFinished {
                run_id: self.run_id.clone(),
                status: RunStatus::Succeeded,
                error: None,
            })
            .await;
        info!("run finished successfully");
        runner.clear_active_run().await;
    }

    /// Emits a cancelled run-finished event and clears the active run.
    /// `emit_run_cancelled` controls whether a preceding `RunCancelled` event
    /// should also be emitted (the stream loop already emits one when a message
    /// is in flight).
    async fn finish_cancelled(&self, runner: &Arc<SessionRunner>, emit_run_cancelled: bool) {
        if emit_run_cancelled {
            runner
                .emit(RuntimeEventKind::RunCancelled {
                    run_id: self.run_id.clone(),
                })
                .await;
        }
        runner
            .emit(RuntimeEventKind::RunFinished {
                run_id: self.run_id.clone(),
                status: RunStatus::Cancelled,
                error: None,
            })
            .await;
        info!("run cancelled");
        runner.clear_active_run().await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use byte_models::{EchoProvider, ModelProvider, ProviderError, ProviderEvent, ProviderStream};
    use byte_protocol::{MessageRole, RunMessage, RunStatus, RuntimeEventKind, SendMessageParams};
    use byte_session::SessionStore;
    use byte_skills::MvpSkillRegistry;
    use byte_tools::{
        AllowAllPolicy, ApplyPatchTool, FindFilesTool, GrepTool, ListDirectoryTool,
        MvpToolRegistry, ReadFileTool, RunCommandTool, ToolRegistry, WriteFileTool,
    };
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    use crate::event_bus::RecordingEventBus;
    use crate::runtime_services::RuntimeServices;

    use super::{DeltaBuffer, RunnerError, SessionRunner};

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }
    fn runner_with_tools(store: Arc<SessionStore>, bus: Arc<RecordingEventBus>) -> SessionRunner {
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );

        let services = RuntimeServices::new(
            Arc::new(EchoProvider::default()),
            store,
            bus,
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        SessionRunner::new(services)
    }

    fn runner_without_tools(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<RecordingEventBus>,
    ) -> SessionRunner {
        let services = RuntimeServices::new(
            provider,
            store,
            bus,
            Arc::new(MvpToolRegistry::new()),
            Arc::new(MvpSkillRegistry::new()),
        );
        SessionRunner::new(services)
    }

    #[test]
    fn delta_buffer_flushes_when_threshold_reached() {
        let mut buffer = DeltaBuffer::new(8);
        assert_eq!(buffer.push("hello "), None);
        assert_eq!(buffer.push("world"), Some("hello world".to_owned()));
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn delta_buffer_flush_returns_remaining_content() {
        let mut buffer = DeltaBuffer::new(8);
        assert_eq!(buffer.push("hi"), None);
        assert_eq!(buffer.flush(), Some("hi".to_owned()));
        assert_eq!(buffer.flush(), None);
    }

    #[tokio::test]
    async fn concurrent_send_message_returns_busy() {
        let store = temp_store();
        let bus = Arc::new(RecordingEventBus::new());
        let runner = runner_with_tools(store.clone(), bus.clone());

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();
        let first = runner
            .send_message(params.clone())
            .await
            .expect("first accepted");
        let second = runner.send_message(params).await;

        assert!(matches!(second, Err(RunnerError::Busy)));
        assert!(!first.is_empty());

        runner.wait_until_idle().await;
    }

    #[tokio::test]
    async fn echo_run_emits_lifecycle_events_and_persists_messages() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let runner = runner_without_tools(
            Arc::new(EchoProvider::default()),
            store.clone(),
            bus.clone(),
        );
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(!events.is_empty());

        assert!(
            matches!(&events[0].kind, RuntimeEventKind::RunStarted { session_id, run_id: rid } if session_id == "s1" && rid == &run_id),
            "first event should be run_started"
        );

        let deltas: Vec<String> = events
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::MessageDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert!(!deltas.is_empty(), "should emit at least one delta");
        assert_eq!(deltas.concat(), "Echo: hello");

        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::MessageStarted { role, run_id: rid, .. } if rid == &run_id && *role == MessageRole::Assistant)),
            "should emit assistant message_started"
        );
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::MessageCompleted { run_id: rid, .. } if rid == &run_id)),
            "should emit message_completed"
        );
        assert!(
            matches!(&events.last().unwrap().kind, RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, error: None, run_id: rid } if rid == &run_id),
            "last event should be successful run_finished"
        );

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(view.messages[0].content, "hello");
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(view.messages[1].content, "Echo: hello");
        assert_eq!(
            view.messages[1].parent_id,
            Some(view.messages[0].id.clone())
        );
    }

    #[tokio::test]
    async fn echo_run_with_small_chunk_size_emits_multiple_deltas() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let provider = Arc::new(EchoProvider {
            chunk_size: 3,
            ..Default::default()
        });
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            provider,
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello world".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        let deltas: Vec<String> = events
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::MessageDelta {
                    delta, run_id: rid, ..
                } if rid == &run_id => Some(delta.clone()),
                _ => None,
            })
            .collect();

        assert!(deltas.len() > 1, "should emit multiple deltas");
        assert_eq!(deltas.concat(), "Echo: hello world");
    }

    struct BoomProvider;

    #[async_trait]
    impl ModelProvider for BoomProvider {
        async fn send_message(
            &self,
            _messages: Vec<RunMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            Err(ProviderError::Request("boom".into()))
        }
    }

    #[tokio::test]
    async fn provider_error_emits_error_and_failed_run() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(BoomProvider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            matches!(&events[0].kind, RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id)
        );
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::Error { run_id: Some(rid), message } if rid == &run_id && message.contains("boom"))),
            "should emit error event containing boom"
        );
        assert!(
            matches!(&events.last().unwrap().kind, RuntimeEventKind::RunFinished { status: RunStatus::Failed, error: Some(msg), run_id: rid } if rid == &run_id && msg.contains("boom")),
            "last event should be failed run_finished with boom"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(&event.kind, RuntimeEventKind::MessageStarted { .. })),
            "should not emit message events on provider error"
        );

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(view.messages.len(), 1);
        assert_eq!(view.messages[0].role, MessageRole::Developer);
    }

    #[tokio::test]
    async fn cancel_run_flushes_buffer_and_emits_ordered_events() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let provider = Arc::new(EchoProvider {
            chunk_size: 1,
            delay: Duration::from_millis(10),
        });
        let runner = runner_without_tools(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let run_id = runner.send_message(params).await.expect("send accepted");
        // Wait until at least one message delta has been emitted so the
        // cancellation is guaranteed to observe an in-flight message and the
        // final event sequence contains a delta flush before RunCancelled.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let mut accumulated = Vec::new();
        let mut saw_delta = false;
        while !saw_delta {
            tokio::time::sleep(Duration::from_millis(5)).await;
            accumulated.append(&mut bus.take_events().await);
            saw_delta = accumulated.iter().any(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::MessageDelta { run_id: rid, .. } if rid == &run_id
                )
            });
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout waiting for message_delta"
            );
        }

        runner.cancel_run().await.expect("cancel succeeds");

        let mut events = accumulated;
        events.append(&mut bus.take_events().await);
        let run_events: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunStarted { .. }
                        | RuntimeEventKind::RunFinished { .. }
                        | RuntimeEventKind::RunCancelled { .. }
                        | RuntimeEventKind::MessageStarted { .. }
                        | RuntimeEventKind::MessageDelta { .. }
                )
            })
            .collect();

        assert!(
            matches!(&run_events[0].kind, RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id)
        );
        assert!(
            run_events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::MessageStarted { run_id: rid, .. } if rid == &run_id)),
            "should emit message_started"
        );

        let last_three: Vec<_> = run_events.iter().rev().take(3).rev().copied().collect();
        assert_eq!(last_three.len(), 3, "should have at least three run events");
        assert!(
            matches!(&last_three[0].kind, RuntimeEventKind::MessageDelta { run_id: rid, .. } if rid == &run_id),
            "second-to-last event before run_cancelled should be a message_delta flush"
        );
        assert!(
            matches!(&last_three[1].kind, RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id),
            "should emit run_cancelled"
        );
        assert!(
            matches!(&last_three[2].kind, RuntimeEventKind::RunFinished { run_id: rid, status: RunStatus::Cancelled, error: None } if rid == &run_id),
            "last event should be run_finished(Cancelled)"
        );

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(
            view.messages.len(),
            1,
            "assistant message should not be persisted"
        );
    }

    #[tokio::test]
    async fn cancel_run_is_idempotent_when_idle() {
        let bus = Arc::new(RecordingEventBus::new());
        let runner = runner_with_tools(temp_store(), bus.clone());

        runner.cancel_run().await.expect("first cancel succeeds");
        runner.cancel_run().await.expect("second cancel succeeds");
    }

    #[tokio::test]
    async fn cancel_run_then_send_message_starts_new_run() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let provider = Arc::new(EchoProvider {
            chunk_size: 1,
            delay: Duration::from_millis(10),
        });
        let runner = runner_without_tools(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let first_id = runner
            .send_message(params.clone())
            .await
            .expect("send accepted");
        tokio::time::sleep(Duration::from_millis(20)).await;
        runner.cancel_run().await.expect("cancel succeeds");

        let second_id = runner
            .send_message(params)
            .await
            .expect("second send accepted");
        assert_ne!(first_id, second_id);
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        let finished_runs: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunFinished { run_id: rid, .. } if rid == &second_id
                )
            })
            .collect();
        assert_eq!(finished_runs.len(), 1, "second run should finish");

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(
            view.messages.len(),
            3,
            "should persist two developer messages and one assistant message"
        );
    }

    #[tokio::test]
    async fn concurrent_cancel_run_waits_for_run_to_finish() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let provider = Arc::new(EchoProvider {
            chunk_size: 1,
            delay: Duration::from_millis(20),
        });
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            provider,
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
        store.new_session("s1", "/workspace").await.unwrap();

        let run_id = runner.send_message(params).await.expect("send accepted");

        // Wait until the assistant message has started so the cancellation is
        // guaranteed to observe an in-flight message and emit a RunCancelled.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let mut accumulated = Vec::new();
        let mut saw_started = false;
        while !saw_started {
            tokio::time::sleep(Duration::from_millis(5)).await;
            accumulated.append(&mut bus.take_events().await);
            saw_started = accumulated.iter().any(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::MessageStarted { run_id: rid, .. } if rid == &run_id
                )
            });
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout waiting for message_started"
            );
        }

        let runner2 = runner.clone();
        let cancel1 = tokio::spawn(async move { runner.cancel_run().await });
        let cancel2 = tokio::spawn(async move { runner2.cancel_run().await });

        let (result1, result2) = tokio::join!(cancel1, cancel2);
        result1
            .expect("cancel1 task joins")
            .expect("cancel1 succeeds");
        result2
            .expect("cancel2 task joins")
            .expect("cancel2 succeeds");

        let mut events = accumulated;
        events.extend(bus.take_events().await);
        let cancelled_events: Vec<_> = events
            .iter()
            .filter(|event| matches!(&event.kind, RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id))
            .collect();
        assert_eq!(
            cancelled_events.len(),
            1,
            "should emit exactly one run_cancelled event"
        );
    }
    #[tokio::test]
    async fn read_file_tool_loop_end_to_end() {
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "读一下 main.rs".into(),
        };
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session(&params.session_id, workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(workspace.join("main.rs"), "fn main() {}")
            .await
            .expect("write main.rs");

        let runner = runner_with_tools(store.clone(), bus.clone());

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;

        // Expected sequence:
        // RunStarted, MessageStarted(Assistant), MessageCompleted(tool_calls),
        // ToolStarted, ToolFinished, MessageStarted(Assistant), MessageCompleted(no tool_calls),
        // RunFinished(Succeeded)
        let run_event_kinds: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunStarted { .. }
                        | RuntimeEventKind::MessageStarted { .. }
                        | RuntimeEventKind::MessageCompleted { .. }
                        | RuntimeEventKind::ToolStarted { .. }
                        | RuntimeEventKind::ToolFinished { .. }
                        | RuntimeEventKind::RunFinished { .. }
                )
            })
            .map(|event| &event.kind)
            .collect();

        assert!(
            matches!(&run_event_kinds[0], RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id),
            "first event should be run_started"
        );
        assert!(
            matches!(&run_event_kinds[1], RuntimeEventKind::MessageStarted { role: MessageRole::Assistant, run_id: rid, .. } if rid == &run_id),
            "second event should be assistant message_started"
        );
        assert!(
            matches!(&run_event_kinds[2], RuntimeEventKind::MessageCompleted { run_id: rid, tool_calls: Some(_), .. } if rid == &run_id),
            "third event should be message_completed with tool_calls"
        );
        assert!(
            matches!(&run_event_kinds[3], RuntimeEventKind::ToolStarted { .. }),
            "fourth event should be tool_started"
        );
        assert!(
            matches!(&run_event_kinds[4], RuntimeEventKind::ToolFinished { output: out, is_error: false, .. } if out == "fn main() {}"),
            "fifth event should be tool_finished with file contents"
        );
        assert!(
            matches!(&run_event_kinds[6], RuntimeEventKind::MessageCompleted { run_id: rid, tool_calls: None, .. } if rid == &run_id),
            "seventh event should be message_completed without tool_calls"
        );
        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(
            view.messages.len(),
            4,
            "developer + assistant + tool result + final assistant"
        );
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(view.messages[0].content, "读一下 main.rs");
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert!(
            view.messages[1].tool_calls.is_some(),
            "assistant message should keep its tool_calls"
        );
        assert_eq!(view.messages[2].role, MessageRole::Tool);
        assert_eq!(
            view.messages[2].tool_call_id.as_deref(),
            Some("echo-call-1")
        );
        assert_eq!(view.messages[2].content, "fn main() {}");
        assert_eq!(view.messages[3].role, MessageRole::Assistant);
        assert_eq!(view.messages[3].content, "Echo: 读一下 main.rs");

        let contents = tokio::fs::read_to_string(store_dir.path().join("s1.jsonl"))
            .await
            .expect("read session file");
        assert!(contents.contains("echo-call-1"));
        assert!(contents.contains("fn main() {}"));
    }

    /// A provider that records every `messages` slice it receives and drives a
    /// deterministic two-turn conversation: turn 0 requests one tool call, turn 1
    /// returns a plain text answer.
    struct RecordingProvider {
        calls: Arc<Mutex<Vec<Vec<RunMessage>>>>,
    }

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        async fn send_message(
            &self,
            messages: Vec<RunMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let mut calls = self.calls.lock().await;
            let turn = calls.len();
            calls.push(messages);
            drop(calls);

            let events: Vec<Result<ProviderEvent, ProviderError>> = if turn == 0 {
                let message_id = "assistant-1".to_string();
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(vec![byte_protocol::ToolCall {
                            id: "call-1".into(),
                            name: "read_file".into(),
                            arguments: serde_json::json!({"path": "main.rs"}),
                        }]),
                    }),
                ]
            } else {
                let message_id = "assistant-2".to_string();
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        message_id: message_id.clone(),
                        delta: "done".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: None,
                    }),
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn assistant_tool_calls_are_passed_to_next_provider_call() {
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "读一下 main.rs".into(),
        };
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session(&params.session_id, workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(workspace.join("main.rs"), "fn main() {}")
            .await
            .expect("write main.rs");

        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(RecordingProvider {
            calls: calls.clone(),
        });
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            provider,
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, run_id: rid, .. } if rid == &run_id)),
            "run should finish successfully"
        );

        let calls = calls.lock().await;
        assert_eq!(calls.len(), 2, "should make exactly two provider calls");

        // First turn: system prompt + current user message.
        assert_eq!(calls[0].len(), 2);
        assert_eq!(calls[0][0].role, MessageRole::System);
        assert_eq!(calls[0][1].role, MessageRole::Developer);

        // Second turn: system, user, assistant (must carry tool_calls), tool result.
        assert_eq!(calls[1].len(), 4);
        assert_eq!(calls[1][2].role, MessageRole::Assistant);
        assert!(
            calls[1][2].tool_calls.is_some(),
            "assistant RunMessage passed to the next turn must carry tool_calls"
        );
        let tool_calls = calls[1][2].tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "read_file");

        assert_eq!(calls[1][3].role, MessageRole::Tool);
        assert_eq!(calls[1][3].tool_call_id, Some("call-1".into()));
    }
    #[tokio::test]
    async fn write_file_tool_loop_end_to_end() {
        let params = SendMessageParams {
            session_id: "write-s1".into(),
            message: "创建 hello.txt".into(),
        };
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session(&params.session_id, workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "write_file".to_string(),
            Arc::new(WriteFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(EchoProvider::default()),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;

        assert!(
            events.iter().any(|event| matches!(&event.kind,
                RuntimeEventKind::ToolStarted { name, .. } if name == "write_file"
            )),
            "should emit tool_started for write_file"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolFinished { output, is_error: false, .. } if output.contains("wrote")
            )),
            "should emit tool_finished with success"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, run_id: rid, .. } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let written_path = workspace.join("hello.txt");
        assert!(written_path.exists());
        let content = tokio::fs::read_to_string(&written_path).await.unwrap();
        assert_eq!(content, "Hello, world!");

        let jsonl = tokio::fs::read_to_string(store_dir.path().join("write-s1.jsonl"))
            .await
            .expect("read session jsonl");
        assert!(jsonl.contains("tool_result"));
        assert!(jsonl.contains("Hello, world!"));
    }

    #[tokio::test]
    async fn apply_patch_tool_loop_end_to_end() {
        let params = SendMessageParams {
            session_id: "patch-s1".into(),
            message: "apply_patch src/lib.rs".into(),
        };
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session(&params.session_id, workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        tokio::fs::create_dir_all(workspace.join("src"))
            .await
            .unwrap();
        tokio::fs::write(
            workspace.join("src/lib.rs"),
            "fn old_one() {}\nfn old_two() {}\n",
        )
        .await
        .unwrap();

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "apply_patch".to_string(),
            Arc::new(ApplyPatchTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(EchoProvider::default()),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);

        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;

        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolStarted { name, .. } if name == "apply_patch"
            )),
            "should emit tool_started for apply_patch"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolFinished {
                    is_error: false,
                    ..
                }
            )),
            "should emit tool_finished with success"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, run_id: rid, .. } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let content = tokio::fs::read_to_string(workspace.join("src/lib.rs"))
            .await
            .unwrap();
        assert_eq!(content, "fn new_one() {}\nfn new_two() {}\n");

        let jsonl = tokio::fs::read_to_string(store_dir.path().join("patch-s1.jsonl"))
            .await
            .expect("read session jsonl");
        assert!(jsonl.contains("tool_result"));
        assert!(jsonl.contains("applied 2 patch(es)"));
    }
    /// A provider that deterministically requests a single tool call on turn 0
    /// and replies with plain text on turn 1.
    struct ToolCallProvider {
        tool_call: byte_protocol::ToolCall,
        turn: Mutex<usize>,
    }

    #[async_trait]
    impl ModelProvider for ToolCallProvider {
        async fn send_message(
            &self,
            _messages: Vec<RunMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let mut turn = self.turn.lock().await;
            let current_turn = *turn;
            *turn += 1;
            drop(turn);

            let events: Vec<Result<ProviderEvent, ProviderError>> = if current_turn == 0 {
                let message_id = "assistant-1".to_string();
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(vec![self.tool_call.clone()]),
                    }),
                ]
            } else {
                let message_id = "assistant-2".to_string();
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        message_id: message_id.clone(),
                        delta: "done".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: None,
                    }),
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    async fn run_tool_end_to_end(
        tool_name: &str,
        tool_call: byte_protocol::ToolCall,
        workspace_setup: impl AsyncFnOnce(&std::path::Path),
        output_assertion: impl Fn(&str),
    ) {
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("tool-session", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        workspace_setup(temp.path()).await;

        let mut registry = MvpToolRegistry::new();
        match tool_name {
            "list_directory" => registry.register(
                "list_directory".to_string(),
                Arc::new(ListDirectoryTool),
                Arc::new(AllowAllPolicy),
            ),
            "grep" => registry.register(
                "grep".to_string(),
                Arc::new(GrepTool),
                Arc::new(AllowAllPolicy),
            ),
            "find_files" => registry.register(
                "find_files".to_string(),
                Arc::new(FindFilesTool),
                Arc::new(AllowAllPolicy),
            ),
            "run_command" => registry.register(
                "run_command".to_string(),
                Arc::new(RunCommandTool),
                Arc::new(AllowAllPolicy),
            ),
            _ => panic!("unknown tool: {tool_name}"),
        }

        let services = RuntimeServices::new(
            Arc::new(ToolCallProvider {
                tool_call,
                turn: Mutex::new(0),
            }),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);

        let run_id = runner
            .send_message(SendMessageParams {
                session_id: "tool-session".into(),
                message: format!("use {tool_name}"),
            })
            .await
            .expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolStarted { name, .. } if name == tool_name
            )),
            "should emit tool_started for {tool_name}"
        );

        let tool_finished = events.iter().find_map(|event| match &event.kind {
            RuntimeEventKind::ToolFinished {
                output,
                is_error: false,
                ..
            } => Some(output.clone()),
            _ => None,
        });
        let output = tool_finished.expect("tool should finish successfully");
        output_assertion(&output);

        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, run_id: rid, .. } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let jsonl = tokio::fs::read_to_string(store_dir.path().join("tool-session.jsonl"))
            .await
            .expect("read session jsonl");
        assert!(
            jsonl.contains("tool_result"),
            "session jsonl should contain tool_result"
        );
    }

    #[tokio::test]
    async fn list_directory_tool_loop_end_to_end() {
        run_tool_end_to_end(
            "list_directory",
            byte_protocol::ToolCall {
                id: "call-1".into(),
                name: "list_directory".into(),
                arguments: serde_json::json!({"path": "."}),
            },
            async |path| {
                tokio::fs::create_dir(path.join("src")).await.unwrap();
                tokio::fs::write(path.join("README.md"), "# hi")
                    .await
                    .unwrap();
            },
            |output| {
                assert!(output.contains("README.md"), "output should list README.md");
                assert!(
                    output.contains("\"type\": \"file\""),
                    "README.md should be a file"
                );
                assert!(output.contains("src"), "output should list src");
                assert!(
                    output.contains("\"type\": \"directory\""),
                    "src should be a directory"
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn grep_tool_loop_end_to_end() {
        run_tool_end_to_end(
            "grep",
            byte_protocol::ToolCall {
                id: "call-1".into(),
                name: "grep".into(),
                arguments: serde_json::json!({"pattern": "fn main", "path": "."}),
            },
            async |path| {
                tokio::fs::write(path.join("main.rs"), "fn main() {}\n")
                    .await
                    .unwrap();
            },
            |output| {
                assert!(output.contains("main.rs"), "output should contain main.rs");
                assert!(output.contains("\"line\": 1"), "match should be on line 1");
                assert!(
                    output.contains("fn main() {}"),
                    "output should contain matched line"
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn find_files_tool_loop_end_to_end() {
        run_tool_end_to_end(
            "find_files",
            byte_protocol::ToolCall {
                id: "call-1".into(),
                name: "find_files".into(),
                arguments: serde_json::json!({"pattern": "**/*.rs", "path": "."}),
            },
            async |path| {
                tokio::fs::create_dir(path.join("src")).await.unwrap();
                tokio::fs::write(path.join("src/lib.rs"), "").await.unwrap();
                tokio::fs::write(path.join("Cargo.toml"), "").await.unwrap();
            },
            |output| {
                assert!(
                    output.contains("src/lib.rs"),
                    "output should contain src/lib.rs"
                );
                assert!(
                    !output.contains("Cargo.toml"),
                    "output should not contain Cargo.toml"
                );
            },
        )
        .await;
    }
    #[tokio::test]
    async fn run_command_tool_loop_end_to_end() {
        run_tool_end_to_end(
            "run_command",
            byte_protocol::ToolCall {
                id: "call-1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({"command": "cat file.txt"}),
            },
            async |path| {
                tokio::fs::write(path.join("file.txt"), "hello command")
                    .await
                    .unwrap();
            },
            |output| {
                assert!(
                    output.contains("hello command"),
                    "output should contain file contents"
                );
            },
        )
        .await;
    }

    /// A provider that requests `activate_skill("review")` on turn 0 and
    /// returns plain text on turn 1, recording every message slice it receives.
    struct SkillActivatingProvider {
        calls: Arc<Mutex<Vec<Vec<RunMessage>>>>,
    }

    #[async_trait]
    impl ModelProvider for SkillActivatingProvider {
        async fn send_message(
            &self,
            messages: Vec<RunMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let mut calls = self.calls.lock().await;
            let turn = calls.len();
            calls.push(messages);
            drop(calls);

            let events: Vec<Result<ProviderEvent, ProviderError>> = if turn == 0 {
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: "assistant-1".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id: "assistant-1".into(),
                        tool_calls: Some(vec![byte_protocol::ToolCall {
                            id: "skill-call-1".into(),
                            name: "activate_skill".into(),
                            arguments: serde_json::json!({"name": "review"}),
                        }]),
                    }),
                ]
            } else {
                vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: "assistant-2".into(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        message_id: "assistant-2".into(),
                        delta: "done".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id: "assistant-2".into(),
                        tool_calls: None,
                    }),
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn activate_skill_injects_content_into_later_run_system_prompt() {
        let bus = Arc::new(RecordingEventBus::new());
        let store_dir = tempdir().expect("temp dir");
        let store =
            Arc::new(SessionStore::new(store_dir.path().to_path_buf()).expect("store creates"));
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();

        let skill_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: Review skill\n---\n# Review\n\nAlways review carefully.",
        )
        .unwrap();

        store
            .new_session("skill-session", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(SkillActivatingProvider {
            calls: calls.clone(),
        });
        let services = RuntimeServices::new(
            provider,
            store.clone(),
            bus.clone(),
            Arc::new(MvpToolRegistry::new()),
            Arc::new(MvpSkillRegistry::new()),
        );
        let runner = SessionRunner::new(services);

        let first_params = SendMessageParams {
            session_id: "skill-session".into(),
            message: "activate review".into(),
        };
        runner
            .send_message(first_params)
            .await
            .expect("first run accepted");
        runner.wait_until_idle().await;

        let second_params = SendMessageParams {
            session_id: "skill-session".into(),
            message: "now what".into(),
        };
        runner
            .send_message(second_params)
            .await
            .expect("second run accepted");
        runner.wait_until_idle().await;

        let calls = calls.lock().await;
        assert_eq!(
            calls.len(),
            3,
            "should make three provider calls: two turns for the first run and one for the second"
        );

        // First run turn 0: system prompt only lists tools.
        assert_eq!(calls[0][0].role, MessageRole::System);
        assert!(
            !calls[0][0].content.contains("Always review carefully."),
            "first run turn 0 should not yet contain skill content"
        );

        // First run turn 1 rebuilds the system prompt to include the skill
        // activated during the previous turn.
        assert_eq!(calls[1][0].role, MessageRole::System);
        assert!(
            calls[1][0].content.contains("Always review carefully."),
            "first run turn 1 should reflect the newly activated skill"
        );

        // Second run rebuilds the system prompt with the activated skill.
        assert_eq!(calls[2][0].role, MessageRole::System);
        assert!(
            calls[2][0].content.contains("Always review carefully."),
            "second run system prompt should contain skill content"
        );

        drop(calls);

        let jsonl = tokio::fs::read_to_string(store_dir.path().join("skill-session.jsonl"))
            .await
            .expect("read session jsonl");
        assert!(
            jsonl.contains("activate_skill"),
            "session jsonl should persist activate_skill tool call"
        );
        assert!(
            jsonl.contains("skill-call-1"),
            "session jsonl should persist assistant tool_call id"
        );
    }
}
