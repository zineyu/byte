use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use byte_models::{ProviderError, ProviderEvent, ProviderStream};
use byte_protocol::{
    ActivatedSkill, BlockDelta, CancelRunResult, LlmMessage, Message, MessageBlock, MessageBody,
    MessageRole, RunStatus, RuntimeEventKind, SendMessageParams, SessionContext, ToolCall,
};
use byte_session::SessionError;
use byte_skills::SkillError;
use byte_tools::{AllowAllPolicy, ToolOutputResult, ToolStreamEvent};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

use crate::SessionViewError;
use crate::llm_context::{LlmContextBuilder, LlmContextInput};
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
    /// An error originating from the session view repository.
    #[error(transparent)]
    SessionView(#[from] SessionViewError),
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
    /// Create a new runner with aggregated runtime services and no active
    /// skills.
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        Self::with_active_skills(services, Arc::new(Mutex::new(Vec::new())))
    }

    /// Create a new runner with the given pre-loaded active skills.
    #[must_use]
    pub fn with_active_skills(
        services: RuntimeServices,
        active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
    ) -> Self {
        let tool_registry = Arc::new(crate::activate_skill::SessionToolRegistry::new(
            Arc::clone(&services.tool_registry),
            Arc::new(crate::activate_skill::ActivateSkillTool::new(
                Arc::clone(&services.skill_registry),
                Arc::clone(&active_skills),
                Arc::clone(&services.store),
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

        let view = match self
            .services
            .view_repo
            .load_session(&params.session_id)
            .await
        {
            Ok(view) => view,
            Err(error) => {
                self.clear_active_run().await;
                return Err(RunnerError::SessionView(error));
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
                MessageBody::text(&params.message),
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
    history: Vec<Message>,
    /// Raw content of the workspace's AGENTS.md instruction file, if found.
    workspace_instructions: Option<String>,
    /// Token used to cancel this run.
    cancel_token: CancellationToken,
}

/// Errors that can occur inside a run after it has started.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// An error originating from the session store.
    #[error(transparent)]
    SessionStore(#[from] SessionError),
    /// An error originating from the session view repository.
    #[error(transparent)]
    SessionView(#[from] SessionViewError),
    /// An error originating from the skill registry.
    #[error(transparent)]
    SkillRegistry(#[from] SkillError),
    /// An error originating from the model provider.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An error originating from a tool invocation.
    #[error(transparent)]
    Tool(#[from] byte_tools::ToolError),
    /// Any other fatal run error.
    #[error("{0}")]
    Other(String),
}

/// The final result of a run.
#[derive(Debug)]
enum RunOutcome {
    /// The run completed with an assistant response.
    Succeeded,
    /// The run was cancelled by the user.
    Cancelled {
        /// Whether to emit a `RunCancelled` event before `RunFinished`.
        emit_event: bool,
    },
}

/// Mutable state carried through a single provider response stream.
#[derive(Debug)]
struct StreamState {
    /// Optional id for the assistant message being built.
    message_id: Option<String>,
    /// Accumulated assistant message content.
    assistant_content: String,
    /// Tool calls extracted from the stream.
    tool_calls: Option<Vec<ToolCall>>,
    /// Whether the stream has produced a complete response.
    completed: bool,
    /// Whether the stream has produced a `MessageStarted` event.
    saw_message: bool,
    /// Buffer for small provider deltas.
    delta_buffer: DeltaBuffer,
}

impl StreamState {
    /// Create a fresh stream state with the given delta buffer threshold.
    const fn new(threshold: usize) -> Self {
        Self {
            message_id: None,
            assistant_content: String::new(),
            tool_calls: None,
            completed: false,
            saw_message: false,
            delta_buffer: DeltaBuffer::new(threshold),
        }
    }
}

/// Result of consuming one provider stream.
#[derive(Debug)]
enum StreamOutcome {
    /// The stream produced a complete assistant message.
    Assistant(AssistantOutcome),
    /// The stream was cancelled while a message was in flight.
    Cancelled,
}

/// The assistant message produced by a single provider stream.
#[derive(Debug)]
struct AssistantOutcome {
    /// Accumulated assistant message content.
    content: String,
    /// Tool calls extracted from the stream, if any.
    tool_calls: Option<Vec<ToolCall>>,
    /// Whether the stream produced a `MessageStarted` event.
    saw_message: bool,
}

impl RunExecutor {
    /// Execute the run loop.
    #[instrument(skip_all, fields(run_id, session_id))]
    async fn run(self, runner: Arc<SessionRunner>) {
        let _ = tracing::Span::current().record("run_id", &self.run_id);
        let _ = tracing::Span::current().record("session_id", &self.session_id);
        info!("starting run");

        let outcome = self.run_inner(&runner).await;
        runner.clear_active_run().await;

        match outcome {
            Ok(RunOutcome::Succeeded) => {
                info!("run finished successfully");
                runner
                    .emit(RuntimeEventKind::RunFinished {
                        run_id: self.run_id.clone(),
                        status: RunStatus::Succeeded,
                        error: None,
                    })
                    .await;
            }
            Ok(RunOutcome::Cancelled { emit_event }) => {
                if emit_event {
                    runner
                        .emit(RuntimeEventKind::RunCancelled {
                            run_id: self.run_id.clone(),
                        })
                        .await;
                }
                info!("run cancelled");
                runner
                    .emit(RuntimeEventKind::RunFinished {
                        run_id: self.run_id.clone(),
                        status: RunStatus::Cancelled,
                        error: None,
                    })
                    .await;
            }
            Err(error) => {
                let message = error.to_string();
                error!(%self.run_id, %message, "run failed");
                runner
                    .emit(RuntimeEventKind::Error {
                        run_id: Some(self.run_id.clone()),
                        message: message.clone(),
                    })
                    .await;
                runner
                    .emit(RuntimeEventKind::RunFinished {
                        run_id: self.run_id.clone(),
                        status: RunStatus::Failed,
                        error: Some(message),
                    })
                    .await;
            }
        }
    }

    /// Run the conversation loop until the run succeeds, is cancelled, or
    /// encounters a fatal error.
    async fn run_inner(&self, runner: &Arc<SessionRunner>) -> Result<RunOutcome, RunError> {
        runner
            .emit(RuntimeEventKind::RunStarted {
                session_id: self.session_id.clone(),
                run_id: self.run_id.clone(),
            })
            .await;

        let view = runner
            .services
            .view_repo
            .load_session(&self.session_id)
            .await?;
        let session_ctx = SessionContext {
            session_id: Some(self.session_id.clone()),
            workspace_root: PathBuf::from(view.workspace),
        };

        let available_skills = runner
            .services
            .skill_registry
            .catalog(Some(session_ctx.workspace_root.as_path()))
            .await?;

        let tools = runner.services.tool_registry.definitions();
        let active_skills = runner.active_skills.lock().await.clone();
        let prompt_context = LlmContextInput {
            user_message: self.message.clone(),
            history: self.history.clone(),
            tools,
            active_skills,
            available_skills: available_skills.clone(),
            workspace_instructions: self.workspace_instructions.clone(),
        };
        let mut messages = LlmContextBuilder::new().build(prompt_context);
        let mut last_entry_id = self.developer_message_id.clone();
        let mut turn_messages: Vec<LlmMessage> = Vec::new();
        let mut saw_message = false;

        loop {
            if self.cancel_token.is_cancelled() {
                return Ok(RunOutcome::Cancelled {
                    emit_event: saw_message,
                });
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
            if !messages.is_empty() && messages[0].role == MessageRole::System {
                messages[0].body = MessageBody::text(LlmContextBuilder::build_system_prompt(
                    &tools,
                    &active_skills,
                    &available_skills,
                ));
            }

            let stream = runner
                .services
                .provider
                .send_message(messages.clone(), tools)
                .await?;

            let outcome = self
                .consume_provider_stream(runner, stream, &mut last_entry_id)
                .await?;

            let assistant = match outcome {
                StreamOutcome::Assistant(assistant) => assistant,
                StreamOutcome::Cancelled => {
                    return Ok(RunOutcome::Cancelled { emit_event: false });
                }
            };
            saw_message |= assistant.saw_message;

            let Some(calls) = assistant.tool_calls else {
                return Ok(RunOutcome::Succeeded);
            };

            if self.cancel_token.is_cancelled() {
                return Ok(RunOutcome::Cancelled { emit_event: true });
            }

            turn_messages.push(LlmMessage::assistant(
                assistant.content,
                Some(calls.clone()),
            ));

            for call in calls {
                let tool_result = self
                    .execute_tool_call(runner, &call, &session_ctx, &mut last_entry_id)
                    .await?;
                turn_messages.push(tool_result);
            }
        }
    }

    /// Consumes a provider stream, emits message lifecycle events, persists the
    /// assistant message, and returns the assistant outcome. Returns
    /// `StreamOutcome::Cancelled` when the run is cancelled while a message is
    /// in flight.
    async fn consume_provider_stream(
        &self,
        runner: &Arc<SessionRunner>,
        mut stream: ProviderStream,
        last_entry_id: &mut String,
    ) -> Result<StreamOutcome, RunError> {
        let mut state = StreamState::new(8);

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    if let Some(id) = state.message_id.as_ref() {
                        if let Some(flush) = state.delta_buffer.flush() {
                            runner.emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id.clone(),
                                block_index: 0,
                                delta: BlockDelta::TextDelta { delta: flush },
                            }).await;
                        }

                        // Persist any partial assistant message so that cancelled
                        // runs still leave a recoverable history entry.
                        if !state.assistant_content.is_empty() {
                            let _ = runner
                                .services
                                .store
                                .append_message(
                                    &self.session_id,
                                    Some(id),
                                    Some(last_entry_id),
                                    MessageRole::Assistant,
                                    MessageBody::text(&state.assistant_content),
                                    None,
                                )
                                .await?;
                            last_entry_id.clone_from(id);
                        }

                        runner.emit(RuntimeEventKind::RunCancelled {
                            run_id: self.run_id.clone(),
                        }).await;
                    }
                    return Ok(StreamOutcome::Cancelled);
                }
                maybe_event = stream.next() => {
                    match maybe_event {
                        Some(event) => {
                            self.handle_provider_event(
                                runner,
                                event?,
                                &mut state,
                                last_entry_id,
                            ).await?;
                        }
                        None => break,
                    }
                }
            }
        }

        if !state.completed {
            return Err(RunError::Other(
                "provider stream ended without completing the assistant message".into(),
            ));
        }

        Ok(StreamOutcome::Assistant(AssistantOutcome {
            content: state.assistant_content,
            tool_calls: state.tool_calls,
            saw_message: state.saw_message,
        }))
    }

    /// Handles a single provider event inside the stream loop. Updates the
    /// per-turn state and returns an error when the run should finish.
    async fn handle_provider_event(
        &self,
        runner: &Arc<SessionRunner>,
        event: ProviderEvent,
        state: &mut StreamState,
        last_entry_id: &mut String,
    ) -> Result<(), RunError> {
        match event {
            ProviderEvent::MessageStarted { message_id: id } => {
                debug!(message_id = %id, "assistant message started");
                state.saw_message = true;
                state.message_id.clone_from(&Some(id.clone()));
                state.assistant_content.clear();
                state.tool_calls = None;
                state.completed = false;
                runner
                    .emit(RuntimeEventKind::MessageStarted {
                        run_id: self.run_id.clone(),
                        message_id: id,
                        role: MessageRole::Assistant,
                    })
                    .await;
            }
            ProviderEvent::TextDelta {
                message_id: id,
                delta,
            } => {
                if state.message_id.as_ref() == Some(&id) {
                    state.assistant_content.push_str(&delta);
                    if let Some(flush) = state.delta_buffer.push(&delta) {
                        runner
                            .emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id,
                                block_index: 0,
                                delta: BlockDelta::TextDelta { delta: flush },
                            })
                            .await;
                    }
                }
            }
            ProviderEvent::MessageCompleted {
                message_id: id,
                tool_calls: calls,
            } => {
                if state.message_id.as_ref() == Some(&id) {
                    if let Some(flush) = state.delta_buffer.flush() {
                        runner
                            .emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id.clone(),
                                block_index: 0,
                                delta: BlockDelta::TextDelta { delta: flush },
                            })
                            .await;
                    }

                    let mut blocks = vec![MessageBlock::Text {
                        text: state.assistant_content.clone(),
                    }];
                    if let Some(calls) = &calls {
                        for call in calls {
                            blocks.push(MessageBlock::ToolCall(call.clone()));
                        }
                    }
                    let body = MessageBody(blocks);
                    let completed_body = calls.as_ref().map(|calls| {
                        MessageBody(
                            calls
                                .iter()
                                .map(|call| MessageBlock::ToolCall(call.clone()))
                                .collect(),
                        )
                    });

                    runner
                        .emit(RuntimeEventKind::message_completed(
                            self.run_id.clone(),
                            id.clone(),
                            completed_body.clone(),
                        ))
                        .await;

                    let _ = runner
                        .services
                        .store
                        .append_message(
                            &self.session_id,
                            Some(&id),
                            Some(last_entry_id),
                            MessageRole::Assistant,
                            body,
                            None,
                        )
                        .await?;

                    last_entry_id.clone_from(&id);

                    state.tool_calls = calls;
                    state.completed = true;
                }
            }
        }

        Ok(())
    }

    /// Executes a single tool call, emits its lifecycle events, persists the
    /// result, and returns the tool-result message to append to the turn.
    async fn execute_tool_call(
        &self,
        runner: &Arc<SessionRunner>,
        call: &ToolCall,
        session_ctx: &SessionContext,
        last_entry_id: &mut String,
    ) -> Result<LlmMessage, RunError> {
        runner
            .emit(RuntimeEventKind::ToolStarted {
                run_id: self.run_id.clone(),
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
            })
            .await;

        let result = match runner
            .services
            .tool_registry
            .invoke(call, session_ctx, &self.cancel_token)
            .await
        {
            Ok(mut stream) => {
                let mut chunks = Vec::new();
                let mut final_result: Option<ToolOutputResult> = None;
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(ToolStreamEvent::Chunk { chunk }) => {
                            runner
                                .emit(RuntimeEventKind::tool_output_delta(
                                    self.run_id.clone(),
                                    call.id.clone(),
                                    chunk.clone(),
                                ))
                                .await;
                            chunks.push(chunk);
                        }
                        Ok(ToolStreamEvent::Done { result }) => {
                            final_result = Some(result);
                        }
                        Err(error) => {
                            final_result = Some(ToolOutputResult::error(error.to_string()));
                            break;
                        }
                    }
                }
                let result = final_result.ok_or_else(|| {
                    RunError::Other("tool stream ended without producing a final result".into())
                })?;
                if result.is_error && result.exit_code.is_none() {
                    // For non-streaming errors that already encode a ToolError
                    // message, emit the accumulated chunks and keep the error.
                    let _ = chunks;
                }
                result
            }
            Err(error) => ToolOutputResult::error(error.to_string()),
        };

        runner
            .emit(RuntimeEventKind::ToolFinished {
                run_id: self.run_id.clone(),
                tool_call_id: call.id.clone(),
                output: result.output.clone(),
                is_error: result.is_error,
                exit_code: result.exit_code,
            })
            .await;

        let tool_result_id = runner
            .services
            .store
            .append_message(
                &self.session_id,
                None,
                Some(last_entry_id),
                MessageRole::Tool,
                MessageBody::text(&result.output),
                Some(&call.id),
            )
            .await?;

        last_entry_id.clone_from(&tool_result_id);

        Ok(LlmMessage::tool_result(&call.id, result.output))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use byte_models::{EchoProvider, ModelProvider, ProviderError, ProviderStream};
    use byte_protocol::{
        BlockDelta, LlmMessage, Message, MessageBlock, MessageRole, RunStatus, RuntimeEventKind,
        SendMessageParams, SessionView,
    };
    use byte_session::SessionStore;
    use byte_skills::MvpSkillRegistry;
    use byte_tools::{AllowAllPolicy, MvpToolRegistry, ReadFileTool, ToolRegistry, WriteFileTool};
    use tempfile::tempdir;

    fn message_text(message: &Message) -> &str {
        match &message.body.0[..] {
            [MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }

    use crate::SessionViewRepository;
    use crate::event_bus::RecordingEventBus;
    use crate::runtime_services::RuntimeServices;

    use super::{DeltaBuffer, RunnerError, SessionRunner};

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    async fn load_view(store: Arc<SessionStore>, session_id: &str) -> SessionView {
        SessionViewRepository::new(store)
            .load_session(session_id)
            .await
            .expect("session loads")
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
            matches!(&events[0].kind,
                RuntimeEventKind::RunStarted { session_id, run_id: rid } if session_id == "s1" && rid == &run_id
            ),
            "first event should be run_started"
        );

        let deltas: Vec<String> = events
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::MessageDelta {
                    delta: BlockDelta::TextDelta { delta },
                    ..
                } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert!(!deltas.is_empty(), "should emit at least one delta");
        assert_eq!(deltas.concat(), "Echo: hello");

        assert!(
            events.iter().any(|event| matches!(&event.kind,
                RuntimeEventKind::MessageStarted { role, run_id: rid, .. } if rid == &run_id && *role == MessageRole::Assistant
            )),
            "should emit assistant message_started"
        );
        assert!(
            events.iter().any(|event| matches!(&event.kind,
                RuntimeEventKind::MessageCompleted { run_id: rid, .. } if rid == &run_id
            )),
            "should emit message_completed"
        );
        assert!(
            matches!(
                &events.last().unwrap().kind,
                RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, error: None, run_id: rid } if rid == &run_id
            ),
            "last event should be successful run_finished"
        );

        let view = load_view(store.clone(), "s1").await;
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "hello");
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(message_text(&view.messages[1]), "Echo: hello");
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
                    delta: BlockDelta::TextDelta { delta },
                    run_id: rid,
                    ..
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
            _messages: Vec<LlmMessage>,
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
        assert!(matches!(&events[0].kind,
            RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id
        ));
        assert!(
            events.iter().any(|event| matches!(&event.kind,
                RuntimeEventKind::Error { run_id: Some(rid), message } if rid == &run_id && message.contains("boom")
            )),
            "should emit error event containing boom"
        );
        assert!(
            matches!(
                &events.last().unwrap().kind,
                RuntimeEventKind::RunFinished { status: RunStatus::Failed, error: Some(msg), run_id: rid } if rid == &run_id && msg.contains("boom")
            ),
            "last event should be failed run_finished with boom"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(&event.kind, RuntimeEventKind::MessageStarted { .. })),
            "should not emit message events on provider error"
        );

        let view = load_view(store.clone(), "s1").await;
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

        assert!(matches!(&run_events[0].kind,
            RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id
        ));
        assert!(
            run_events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::MessageStarted { run_id: rid, .. } if rid == &run_id
            )),
            "should emit message_started"
        );

        let last_three: Vec<_> = run_events.iter().rev().take(3).rev().copied().collect();
        assert_eq!(last_three.len(), 3, "should have at least three run events");
        assert!(
            matches!(&last_three[0].kind,
                RuntimeEventKind::MessageDelta { run_id: rid, .. } if rid == &run_id
            ),
            "second-to-last event before run_cancelled should be a message_delta flush"
        );
        assert!(
            matches!(&last_three[1].kind,
                RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id
            ),
            "should emit run_cancelled"
        );
        assert!(
            matches!(
                &last_three[2].kind,
                RuntimeEventKind::RunFinished { run_id: rid, status: RunStatus::Cancelled, error: None } if rid == &run_id
            ),
            "last event should be run_finished(Cancelled)"
        );

        let view = load_view(store.clone(), "s1").await;
        assert_eq!(
            view.messages.len(),
            2,
            "developer message and partial assistant message should be persisted"
        );
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "hello");
        assert_eq!(view.messages[1].role, MessageRole::Assistant);
        assert_eq!(
            view.messages[1].parent_id,
            Some(view.messages[0].id.clone())
        );
        assert!(
            !message_text(&view.messages[1]).is_empty(),
            "partial assistant message should contain streamed content"
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

        // Wait until at least one delta has been emitted so the first run is
        // guaranteed to produce a partial assistant message that should be
        // persisted on cancellation.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let mut accumulated = Vec::new();
        let mut saw_delta = false;
        while !saw_delta {
            tokio::time::sleep(Duration::from_millis(5)).await;
            accumulated.append(&mut bus.take_events().await);
            saw_delta = accumulated.iter().any(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::MessageDelta { run_id: rid, .. } if rid == &first_id
                )
            });
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout waiting for message_delta"
            );
        }

        runner.cancel_run().await.expect("cancel succeeds");

        let second_id = runner
            .send_message(params)
            .await
            .expect("second send accepted");
        assert_ne!(first_id, second_id);
        runner.wait_until_idle().await;

        let mut events = accumulated;
        events.append(&mut bus.take_events().await);
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

        let view = load_view(store.clone(), "s1").await;
        assert_eq!(
            view.messages.len(),
            4,
            "should persist two developer messages, one partial assistant, and one final assistant"
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
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id
                )
            })
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
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunStarted { run_id: rid, .. } if rid == &run_id
            )),
            "should emit RunStarted"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolStarted { name, .. } if name == "read_file"
            )),
            "should emit ToolStarted for read_file"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolFinished { tool_call_id, output, .. } if output.contains("fn main() {}")
            )),
            "should emit ToolFinished for read_file"
        );
        assert!(
            matches!(
                events.last().unwrap().kind,
                RuntimeEventKind::RunFinished {
                    status: RunStatus::Succeeded,
                    ..
                }
            ),
            "run should succeed"
        );

        let view = load_view(store.clone(), "s1").await;
        assert!(view.messages.len() >= 3, "expected at least three messages");
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "读一下 main.rs");
        let tool_message = view
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool message");
        assert!(message_text(tool_message).contains("fn main() {}"));
    }

    #[tokio::test]
    async fn write_file_tool_loop_end_to_end() {
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "write hello.txt".into(),
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

        let _run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolStarted { name, .. } if name == "write_file"
            )),
            "should emit ToolStarted for write_file"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::ToolFinished { output, .. } if output.contains("Hello, world!")
            )),
            "should emit ToolFinished for write_file"
        );

        let content = tokio::fs::read_to_string(workspace.join("hello.txt"))
            .await
            .expect("file was written");
        assert_eq!(content, "Hello, world!");
    }
}
