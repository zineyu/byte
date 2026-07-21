use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use byte_models::{ProviderError, ProviderEvent, ProviderStream};
use byte_protocol::{
    ActivatedSkill, BlockDelta, CancelRunResult, LlmMessage, MessageBlock, MessageBody,
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

/// Character threshold for delta buffering. A low value keeps event latency
/// small (each delta is emitted quickly) while still coalescing tiny provider
/// chunks to avoid excessive per-character events.
const DELTA_BUFFER_THRESHOLD: usize = 8;

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
    /// An error originating from the skill registry.
    #[error(transparent)]
    SkillRegistry(#[from] SkillError),
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
    /// Optional in-progress run id and its cancellation token.
    active_run: Arc<Mutex<Option<(RunId, CancellationToken)>>>,
}

impl SessionRunner {
    /// Create a new runner with aggregated runtime services and no active
    /// skills.
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        Self::with_active_skills(services, &Arc::new(Mutex::new(Vec::new())))
    }

    /// Create a new runner with the given pre-loaded active skills.
    ///
    /// The handle is shared with the session-scoped `activate_skill` tool so
    /// activations are visible to the owning [`crate::runner_pool::RunnerPool`];
    /// the runner itself does not read it.
    #[must_use]
    pub fn with_active_skills(
        services: RuntimeServices,
        active_skills: &Arc<Mutex<Vec<ActivatedSkill>>>,
    ) -> Self {
        let tool_registry = Arc::new(crate::activate_skill::SessionToolRegistry::new(
            Arc::clone(&services.tool_registry),
            Arc::new(crate::activate_skill::ActivateSkillTool::new(
                Arc::clone(&services.skill_registry),
                Arc::clone(active_skills),
                Arc::clone(&services.store),
            )),
            Arc::new(AllowAllPolicy),
        ));
        let mut services = services;
        services.tool_registry = tool_registry;
        Self {
            services,
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
        emit_run_cancelled_event: bool,
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
            Ok(RunOutcome::Cancelled {
                emit_run_cancelled_event,
            }) => {
                if emit_run_cancelled_event {
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

    /// Possibly compact the oldest contiguous block of messages if the active
    /// path exceeds the configured budget threshold. Emits compaction lifecycle
    /// events and returns the (possibly unchanged) entries.
    async fn maybe_compact(
        &self,
        runner: &Arc<SessionRunner>,
        entries: &[byte_protocol::SessionEntry],
    ) -> Result<Vec<byte_protocol::SessionEntry>, RunError> {
        let active_path = crate::session::active_path::build_active_path(entries);
        let compaction_service = crate::compaction::CompactionService::new(
            Arc::clone(&runner.services.provider),
            Arc::clone(&runner.services.store),
            runner.services.compaction_config,
        );

        if !compaction_service.is_compaction_needed(&active_path) {
            return Ok(entries.to_vec());
        }

        let messages: Vec<byte_protocol::Message> = entries
            .iter()
            .filter_map(|entry| match entry {
                byte_protocol::SessionEntry::Message(message) => Some(message.clone()),
                _ => None,
            })
            .collect();
        let compacted_ids = crate::compaction::CompactionService::collect_compacted_ids(entries);
        let range = crate::compaction::CompactionService::select_compaction_range(
            &messages,
            &compacted_ids,
        );

        let Some(range) = range else {
            return Ok(entries.to_vec());
        };

        runner
            .emit(RuntimeEventKind::compaction_started(
                self.run_id.clone(),
                self.session_id.clone(),
                range.clone(),
            ))
            .await;

        let cancel_token = self.cancel_token.child_token();

        match compaction_service
            .compact_if_needed(&self.run_id, &self.session_id, entries, Some(&cancel_token))
            .await
        {
            Ok(Some(entry)) => {
                runner
                    .emit(RuntimeEventKind::compaction_completed(
                        self.run_id.clone(),
                        self.session_id.clone(),
                        entry.id,
                        entry.summary,
                        range,
                    ))
                    .await;
            }
            Ok(None) => {
                // Should not happen because we already checked, but safe.
            }
            Err(error) => {
                runner
                    .emit(RuntimeEventKind::compaction_failed(
                        self.run_id.clone(),
                        self.session_id.clone(),
                        error.to_string(),
                    ))
                    .await;
                return Err(RunError::Other(error.to_string()));
            }
        }

        runner
            .services
            .store
            .read_entries(&self.session_id)
            .await
            .map_err(RunError::SessionStore)
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

        let entries = runner.services.store.read_entries(&self.session_id).await?;
        let entries = self.maybe_compact(runner, &entries).await?;
        let active_path = crate::session::active_path::build_active_path(&entries);

        let tools = runner.services.tool_registry.definitions();
        // The system prompt is built once per run and stays stable across
        // model turns so provider prompt caches remain valid. Activated
        // skill content reaches the model through the message stream (tool
        // results in-run, synthetic activation messages from the active
        // path), never through the system prompt. See ADR 0021.
        let prompt_context = LlmContextInput {
            user_message: self.message.clone(),
            history: active_path,
            tools: tools.clone(),
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
                    emit_run_cancelled_event: saw_message,
                });
            }

            if !turn_messages.is_empty() {
                // Subsequent request after tool calls: append the last turn's
                // assistant message and tool results to the conversation.
                messages.extend(turn_messages);
                turn_messages = Vec::new();
            }

            let stream = runner
                .services
                .provider
                .send_message(messages.clone(), tools.clone())
                .await?;

            let outcome = self
                .consume_provider_stream(runner, stream, &mut last_entry_id)
                .await?;

            let assistant = match outcome {
                StreamOutcome::Assistant(assistant) => assistant,
                StreamOutcome::Cancelled => {
                    return Ok(RunOutcome::Cancelled {
                        emit_run_cancelled_event: false,
                    });
                }
            };
            saw_message |= assistant.saw_message;

            let Some(calls) = assistant.tool_calls else {
                return Ok(RunOutcome::Succeeded);
            };

            if self.cancel_token.is_cancelled() {
                return Ok(RunOutcome::Cancelled {
                    emit_run_cancelled_event: true,
                });
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
        let mut state = StreamState::new(DELTA_BUFFER_THRESHOLD);

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

                        // Partial assistant messages must not be persisted on
                        // cancellation; only completed messages and tool results are
                        // durable. Emit the cancellation and return immediately so
                        // the run terminates without appending a partial entry.
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

                    runner
                        .emit(RuntimeEventKind::message_completed(
                            self.run_id.clone(),
                            id.clone(),
                            Some(body.clone()),
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
                final_result.ok_or_else(|| {
                    RunError::Other("tool stream ended without producing a final result".into())
                })?
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
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        unused_results,
        clippy::redundant_closure,
        clippy::uninlined_format_args,
        clippy::useless_conversion,
        clippy::too_many_lines
    )]

    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use byte_models::{
        CodingLoopProvider, EchoProvider, ModelProvider, ProviderError, ProviderEvent,
        ProviderStream,
    };
    use byte_protocol::{
        BlockDelta, LlmMessage, Message, MessageBlock, MessageBody, MessageRole, RunStatus,
        RuntimeEventKind, SendMessageParams, SessionContext, SessionView, ToolCall, ToolDefinition,
    };
    use byte_session::SessionStore;
    use byte_skills::MvpSkillRegistry;
    use byte_tools::{
        AllowAllPolicy, ApplyPatchTool, MvpToolRegistry, ReadFileTool, RunCommandTool, Tool,
        ToolError, ToolOutputResult, ToolOutputStream, ToolRegistry, ToolStreamEvent,
        WriteFileTool,
    };
    use futures::channel::mpsc;
    use futures::stream;
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    fn message_text(message: &Message) -> &str {
        match &message.body.0[..] {
            [MessageBlock::Text { text }] => text.as_str(),
            _ => "",
        }
    }

    fn role_counts(messages: &[Message]) -> (usize, usize, usize, usize) {
        let mut developer = 0;
        let mut assistant = 0;
        let mut tool = 0;
        let mut summary = 0;
        for message in messages {
            match message.role {
                MessageRole::Developer => developer += 1,
                MessageRole::Assistant => assistant += 1,
                MessageRole::Tool => tool += 1,
                MessageRole::Summary => summary += 1,
                MessageRole::System => {}
            }
        }
        (developer, assistant, tool, summary)
    }

    #[tokio::test]
    async fn core_coding_loop_read_edit_command_demo() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(
            workspace.join("main.rs"),
            "fn main() { println!(\"old\"); }",
        )
        .await
        .expect("write main.rs");

        let runner = runner_with_coding_loop_tools(store.clone(), bus.clone());

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Update the program and verify it".into(),
        };
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

        let tool_started: Vec<String> = events
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::ToolStarted { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_started,
            vec!["read_file", "apply_patch", "run_command"],
            "tools should execute in deterministic order"
        );

        let finished = events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &run_id
            )
        });
        assert!(finished, "run should finish successfully");

        let content = tokio::fs::read_to_string(workspace.join("main.rs"))
            .await
            .expect("file was modified");
        assert_eq!(
            content, "fn main() { println!(\"new\"); }",
            "apply_patch should update the file"
        );

        let view = load_view(store, "s1").await;
        let (developer, assistant, tool, _summary) = role_counts(&view.messages);
        assert_eq!(developer, 1, "history should contain the developer message");
        assert_eq!(
            assistant, 4,
            "history should contain one assistant per model turn"
        );
        assert_eq!(
            tool, 3,
            "history should contain one tool result per tool call"
        );
        assert_eq!(
            view.messages.last().unwrap().role,
            MessageRole::Assistant,
            "history should end with the final assistant message"
        );
        assert!(
            message_text(view.messages.last().unwrap()).contains("Done."),
            "final assistant message should summarize the outcome"
        );
    }

    #[tokio::test]
    async fn tool_results_feed_next_model_turn() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(
            workspace.join("main.rs"),
            "fn main() { println!(\"old\"); }",
        )
        .await
        .expect("write main.rs");

        let (provider, calls) = RecordingCodingLoopProvider::new();
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "apply_patch".to_string(),
            Arc::new(ApplyPatchTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "run_command".to_string(),
            Arc::new(RunCommandTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(provider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Update the program and verify it".into(),
        };
        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let calls = calls.lock().await;
        // First turn has no tool results; second turn should see the read_file result;
        // third turn should see the apply_patch result; fourth turn should see the
        // run_command result.
        assert!(
            calls.len() >= 4,
            "provider should be called at least four times for the full demo"
        );

        for (index, expected_tool_call_id) in [(1, "read-call"), (2, "edit-call"), (3, "cmd-call")]
        {
            let context = &calls[index];
            let has_tool_result = context.iter().any(|message| {
                message.role == MessageRole::Tool
                    && message.tool_call_id.as_deref() == Some(expected_tool_call_id)
            });
            assert!(
                has_tool_result,
                "turn {} should include the tool result for {}",
                index + 1,
                expected_tool_call_id
            );
        }
    }

    /// A provider that activates the `review` skill via an `activate_skill`
    /// tool call on the first Model Turn, then finishes on the second,
    /// recording every request for assertions.
    struct ActivateSkillLoopProvider {
        calls: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    }

    #[async_trait]
    impl ModelProvider for ActivateSkillLoopProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            self.calls.lock().await.push(messages.clone());
            let message_id = uuid::Uuid::new_v4().to_string();
            let has_tool_results = messages
                .iter()
                .any(|message| message.role == MessageRole::Tool);
            if !has_tool_results {
                let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(vec![ToolCall {
                            id: "skill-call".into(),
                            name: "activate_skill".into(),
                            arguments: serde_json::json!({"name": "review"}),
                        }]),
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }
            let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: "Done".into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: None,
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn llm_body_text(message: &LlmMessage) -> String {
        message
            .body
            .0
            .iter()
            .filter_map(|block| match block {
                MessageBlock::Text { text } => Some(text.as_str()),
                MessageBlock::ToolCall(_) => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn mid_run_skill_activation_uses_tool_result_with_stable_system_prompt() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        let skill_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: Review code\n---\nReview carefully.\n",
        )
        .expect("write skill file");
        let home = tempdir().expect("temp home");
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        let provider = ActivateSkillLoopProvider {
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let calls = Arc::clone(&provider.calls);
        let services = RuntimeServices::new(
            Arc::new(provider),
            store.clone(),
            bus.clone(),
            Arc::new(MvpToolRegistry::new()),
            Arc::new(MvpSkillRegistry::with_home_dir(home.path())),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let run_id = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "review my code".into(),
            })
            .await
            .expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished { run_id: rid, status: RunStatus::Succeeded, .. } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let calls = calls.lock().await;
        assert_eq!(calls.len(), 2, "activation turn plus final turn");

        // The system prompt is byte-identical across model turns so provider
        // prompt caches remain valid.
        assert_eq!(
            calls[0][0], calls[1][0],
            "system prompt must stay stable across model turns"
        );
        assert!(
            !llm_body_text(&calls[1][0]).contains("Review carefully."),
            "skill body must not be injected into the system prompt"
        );

        // The structured skill content reaches the model through the tool
        // result message of the activation turn.
        let tool_message = calls[1]
            .iter()
            .find(|message| {
                message.role == MessageRole::Tool
                    && message.tool_call_id.as_deref() == Some("skill-call")
            })
            .expect("activate_skill tool result should be in the second request");
        let output: serde_json::Value =
            serde_json::from_str(&llm_body_text(tool_message)).expect("structured tool output");
        assert_eq!(output["name"], "review");
        assert_eq!(output["content"], "Review carefully.");
    }

    #[tokio::test]
    async fn multiple_tool_calls_in_one_message() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(workspace.join("a.txt"), "A")
            .await
            .expect("write a.txt");
        tokio::fs::write(workspace.join("b.txt"), "B")
            .await
            .expect("write b.txt");

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(TwoReadFileProvider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Read both files".into(),
        };
        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let started: Vec<&RuntimeEventKind> = events
            .iter()
            .filter_map(|event| match &event.kind {
                kind @ RuntimeEventKind::ToolStarted { .. } => Some(kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            started.len(),
            2,
            "two tool calls should be started from one assistant message"
        );
        assert!(started.iter().any(|kind| matches!(
            kind,
            RuntimeEventKind::ToolStarted { tool_call_id, .. } if tool_call_id == "call-a"
        )));
        assert!(started.iter().any(|kind| matches!(
            kind,
            RuntimeEventKind::ToolStarted { tool_call_id, .. } if tool_call_id == "call-b"
        )));

        let view = load_view(store, "s1").await;
        let tool_messages: Vec<&Message> = view
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .collect();
        assert_eq!(
            tool_messages.len(),
            2,
            "two tool results should be persisted"
        );
        assert!(tool_messages.iter().any(|message| {
            message.tool_call_id.as_deref() == Some("call-a") && message_text(message).contains('A')
        }));
        assert!(tool_messages.iter().any(|message| {
            message.tool_call_id.as_deref() == Some("call-b") && message_text(message).contains('B')
        }));
    }

    use crate::SessionViewRepository;
    use crate::compaction::CompactionConfig;
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
            CompactionConfig::default(),
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
            CompactionConfig::default(),
        );
        SessionRunner::new(services)
    }

    fn runner_with_coding_loop_tools(
        store: Arc<SessionStore>,
        bus: Arc<RecordingEventBus>,
    ) -> SessionRunner {
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "apply_patch".to_string(),
            Arc::new(ApplyPatchTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "run_command".to_string(),
            Arc::new(RunCommandTool),
            Arc::new(AllowAllPolicy),
        );

        let services = RuntimeServices::new(
            Arc::new(CodingLoopProvider::default()),
            store,
            bus,
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        SessionRunner::new(services)
    }

    fn runner_with_slow_coding_loop_tools(
        store: Arc<SessionStore>,
        bus: Arc<RecordingEventBus>,
    ) -> SessionRunner {
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "apply_patch".to_string(),
            Arc::new(ApplyPatchTool),
            Arc::new(AllowAllPolicy),
        );
        registry.register(
            "run_command".to_string(),
            Arc::new(RunCommandTool),
            Arc::new(AllowAllPolicy),
        );

        let services = RuntimeServices::new(
            Arc::new(CodingLoopProvider {
                delay: Duration::from_millis(50),
            }),
            store,
            bus,
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        SessionRunner::new(services)
    }

    fn runner_with_compaction(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<RecordingEventBus>,
        config: CompactionConfig,
    ) -> SessionRunner {
        let services = RuntimeServices::new(
            provider,
            store,
            bus,
            Arc::new(MvpToolRegistry::new()),
            Arc::new(MvpSkillRegistry::new()),
            config,
        );
        SessionRunner::new(services)
    }

    /// A provider that detects summarization prompts and returns a fixed summary,
    /// otherwise echoes the last developer message like `EchoProvider`.
    struct TestCompactionProvider;

    #[async_trait]
    impl ModelProvider for TestCompactionProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let is_summary_request = messages.first().is_some_and(|message| {
                message.role == MessageRole::System
                    && llm_message_text(message).contains("Summarize")
            });

            if is_summary_request {
                let message_id = uuid::Uuid::new_v4().to_string();
                let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        message_id: message_id.clone(),
                        delta: "summary of prior conversation".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: None,
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }

            let last = messages
                .iter()
                .rev()
                .filter(|message| message.role == MessageRole::Developer)
                .map(|message| llm_message_text(message))
                .next()
                .unwrap_or_default();
            let content = format!("Echo: {}", last);
            let message_id = uuid::Uuid::new_v4().to_string();
            let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: content.into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: None,
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn compaction_triggered_when_budget_exceeded() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        let mut last_id: Option<String> = None;
        for i in 0..4 {
            let developer_text =
                format!("This is a long developer message number {} with text.", i);
            let developer_id = store
                .append_message(
                    "s1",
                    None,
                    last_id.as_deref(),
                    MessageRole::Developer,
                    MessageBody::text(&developer_text),
                    None,
                )
                .await
                .expect("append developer message");

            let assistant_text = format!(
                "This is an assistant response number {} with many chars.",
                i
            );
            let assistant_id = store
                .append_message(
                    "s1",
                    None,
                    Some(&developer_id),
                    MessageRole::Assistant,
                    MessageBody::text(&assistant_text),
                    None,
                )
                .await
                .expect("append assistant message");

            last_id = Some(assistant_id);
        }

        let runner = runner_with_compaction(
            Arc::new(TestCompactionProvider),
            store.clone(),
            bus.clone(),
            CompactionConfig {
                context_budget: 20,
                threshold_percent: 90,
            },
        );

        let run_id = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Hello again".into(),
            })
            .await
            .expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;

        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::CompactionStarted { run_id: rid, session_id, .. }
                if rid == &run_id && session_id == "s1"
            )),
            "should emit CompactionStarted"
        );

        let completed_summary = events.iter().find_map(|event| match &event.kind {
            RuntimeEventKind::CompactionCompleted {
                run_id: rid,
                session_id,
                summary,
                ..
            } if rid == &run_id && session_id == "s1" => Some(summary.clone()),
            _ => None,
        });
        assert!(
            completed_summary.is_some(),
            "should emit CompactionCompleted"
        );
        assert!(
            !completed_summary.unwrap().is_empty(),
            "summary should be non-empty"
        );

        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &run_id
            )),
            "run should finish successfully"
        );

        let entries = store.read_entries("s1").await.expect("read entries");
        let compaction_entry = entries.iter().find_map(|entry| match entry {
            byte_protocol::SessionEntry::CompactionEntry(ce) => Some(ce),
            _ => None,
        });
        assert!(
            compaction_entry.is_some(),
            "should persist a compaction entry"
        );

        let active_path = crate::session::active_path::build_active_path(&entries);
        assert!(
            active_path
                .iter()
                .any(|message| message.role == MessageRole::Summary),
            "active path should contain a Summary message"
        );
    }

    /// Wraps [`CodingLoopProvider`] so the test can inspect the messages sent
    /// to the provider on each Model Turn.
    struct RecordingCodingLoopProvider {
        inner: CodingLoopProvider,
        calls: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    }

    impl RecordingCodingLoopProvider {
        fn new() -> (Self, Arc<Mutex<Vec<Vec<LlmMessage>>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    inner: CodingLoopProvider::default(),
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    #[async_trait]
    impl ModelProvider for RecordingCodingLoopProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            self.calls.lock().await.push(messages.clone());
            self.inner.send_message(messages, tools).await
        }
    }

    /// A provider that returns two `read_file` tool calls in a single assistant
    /// message, then returns final text once both results are present.
    struct TwoReadFileProvider;

    #[async_trait]
    impl ModelProvider for TwoReadFileProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let has_read_file = tools.iter().any(|tool| tool.name == "read_file");
            let tool_count = messages
                .iter()
                .filter(|message| message.role == MessageRole::Tool)
                .count();

            if has_read_file && tool_count == 0 {
                let calls = vec![
                    ToolCall {
                        id: "call-a".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.txt"}),
                    },
                    ToolCall {
                        id: "call-b".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "b.txt"}),
                    },
                ];
                let message_id = uuid::Uuid::new_v4().to_string();
                let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(calls),
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }

            let content = if tool_count == 2 {
                "Done: both files read."
            } else {
                "Echo"
            };
            let message_id = uuid::Uuid::new_v4().to_string();
            let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: content.into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: None,
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    /// A provider that returns a `read_file` tool call on the first Model Turn
    /// and then fails, so tests can verify that the runner releases the
    /// active-run state after a loop error.
    struct FailingAfterFirstTurnProvider;

    #[async_trait]
    impl ModelProvider for FailingAfterFirstTurnProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let has_read_file = tools.iter().any(|tool| tool.name == "read_file");
            let tool_count = messages
                .iter()
                .filter(|message| message.role == MessageRole::Tool)
                .count();

            if has_read_file && tool_count == 0 {
                let message_id = "msg-1".to_string();
                let call = ToolCall {
                    id: "read-call".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "main.rs"}),
                };
                let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(vec![call]),
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }

            Err(ProviderError::Request(
                "provider failure after first turn".into(),
            ))
        }
    }

    #[test]
    fn delta_buffer_flushes_when_threshold_reached() {
        let mut buffer = DeltaBuffer::new(super::DELTA_BUFFER_THRESHOLD);
        assert_eq!(buffer.push("hello "), None);
        assert_eq!(buffer.push("world"), Some("hello world".to_owned()));
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn delta_buffer_flush_returns_remaining_content() {
        let mut buffer = DeltaBuffer::new(super::DELTA_BUFFER_THRESHOLD);
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
            CompactionConfig::default(),
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
            _tools: Vec<ToolDefinition>,
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
            CompactionConfig::default(),
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
    async fn cancel_run_does_not_persist_partial_assistant_message() {
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

        // Wait until at least one delta has been emitted so the cancellation
        // observes an in-flight assistant message.
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
        runner.wait_until_idle().await;

        let view = load_view(store, "s1").await;
        assert_eq!(
            view.messages.len(),
            1,
            "only the developer message should be persisted; no partial assistant message"
        );
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "hello");
    }

    #[tokio::test]
    async fn cancel_run_allows_next_run() {
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
        runner.wait_until_idle().await;

        let second_id = runner
            .send_message(params)
            .await
            .expect("second send accepted");
        assert_ne!(first_id, second_id);
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &second_id
            )),
            "second run should finish successfully"
        );
    }

    #[tokio::test]
    async fn cancel_run_emits_single_terminal_cancelled_outcome() {
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
        runner.wait_until_idle().await;

        let mut events = accumulated;
        events.append(&mut bus.take_events().await);
        let cancelled_events: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id
                )
            })
            .collect();
        let finished_cancelled: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunFinished {
                        run_id: rid,
                        status: RunStatus::Cancelled,
                        ..
                    } if rid == &run_id
                )
            })
            .collect();

        assert_eq!(
            cancelled_events.len(),
            1,
            "should emit exactly one RunCancelled event"
        );
        assert_eq!(
            finished_cancelled.len(),
            1,
            "should emit exactly one RunFinished(Cancelled) event"
        );
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
            1,
            "only the developer message should be persisted; partial assistant messages are dropped on cancellation"
        );
        assert_eq!(view.messages[0].role, MessageRole::Developer);
        assert_eq!(message_text(&view.messages[0]), "hello");
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
        // guaranteed to produce a partial assistant message before cancellation.
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
            3,
            "should persist two developer messages and one final assistant from the second run"
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
            CompactionConfig::default(),
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
    async fn same_session_busy_during_multi_tool_loop() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(
            workspace.join("main.rs"),
            "fn main() { println!(\"old\"); }",
        )
        .await
        .expect("write main.rs");

        let runner = runner_with_slow_coding_loop_tools(store.clone(), bus.clone());

        let _first = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Update and verify".into(),
            })
            .await
            .expect("first send accepted");

        // Give the first run time to start before sending the second message.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let second = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Another request".into(),
            })
            .await;
        assert!(
            matches!(second, Err(RunnerError::Busy)),
            "second message should be rejected while the first multi-turn run is active"
        );

        runner.wait_until_idle().await;

        let view = load_view(store, "s1").await;
        assert!(
            !view.messages.iter().any(|message| {
                message.role == MessageRole::Developer && message_text(message) == "Another request"
            }),
            "rejected second message should not be appended to history"
        );
    }

    #[tokio::test]
    async fn cross_session_runs_remain_independent_during_loop() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session s1");
        store
            .new_session("s2", workspace.to_str().unwrap())
            .await
            .expect("create session s2");
        tokio::fs::write(
            workspace.join("main.rs"),
            "fn main() { println!(\"old\"); }",
        )
        .await
        .expect("write main.rs");

        let runner1 = runner_with_coding_loop_tools(store.clone(), bus.clone());
        let runner2 = runner_with_coding_loop_tools(store.clone(), bus.clone());

        let first = runner1
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Update and verify".into(),
            })
            .await
            .expect("s1 run accepted");
        let second = runner2
            .send_message(SendMessageParams {
                session_id: "s2".into(),
                message: "Update and verify".into(),
            })
            .await
            .expect("s2 run accepted");
        assert_ne!(first, second);

        runner1.wait_until_idle().await;
        runner2.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &first
            )),
            "s1 run should finish"
        );
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Succeeded,
                    ..
                } if rid == &second
            )),
            "s2 run should finish"
        );
    }

    #[tokio::test]
    async fn active_run_released_after_loop_error() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(workspace.join("main.rs"), "fn main() {}")
            .await
            .expect("write main.rs");

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(FailingAfterFirstTurnProvider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let first = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Read main.rs".into(),
            })
            .await
            .expect("first send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    status: RunStatus::Failed,
                    ..
                } if rid == &first
            )),
            "first run should fail after the first tool turn"
        );

        let second = runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "Second request".into(),
            })
            .await
            .expect("second send accepted after error release");
        assert_ne!(first, second);
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(
                &event.kind,
                RuntimeEventKind::RunFinished {
                    run_id: rid,
                    ..
                } if rid == &second
            )),
            "second run should reach a terminal outcome"
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
            CompactionConfig::default(),
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

    #[tokio::test]
    async fn message_completed_body_has_text_then_tool_calls() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");
        tokio::fs::write(workspace.join("main.rs"), "fn main() {}")
            .await
            .expect("write main.rs");

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(TextAndToolCallProvider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Read the file".into(),
        };
        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        let completed = events
            .iter()
            .find(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::MessageCompleted { run_id: rid, .. } if rid == &run_id
                )
            })
            .expect("should emit MessageCompleted");

        let body = match &completed.kind {
            RuntimeEventKind::MessageCompleted { body, .. } => body.as_ref(),
            _ => panic!("expected MessageCompleted event"),
        }
        .expect("MessageCompleted should carry body");
        assert!(
            body.0.len() >= 2,
            "body should contain at least text and one tool call"
        );
        assert!(
            matches!(&body.0[0], MessageBlock::Text { text } if text == "I will read the file."),
            "first block should be the assistant text"
        );
        assert!(
            matches!(&body.0[1],
                MessageBlock::ToolCall(call) if call.name == "read_file"
            ),
            "second block should be the read_file tool call"
        );

        let view = load_view(store, "s1").await;
        let assistant = view
            .messages
            .iter()
            .find(|message| message.role == MessageRole::Assistant)
            .expect("assistant message persisted");
        assert_eq!(
            assistant.body, *body,
            "persisted body should match event body"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn cancel_during_cancellable_tool_reaches_cancelled_outcome() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        let mut registry = MvpToolRegistry::new();
        registry.register(
            "cancellable_wait".to_string(),
            Arc::new(CancellableWaitTool),
            Arc::new(AllowAllPolicy),
        );
        let services = RuntimeServices::new(
            Arc::new(CancellableToolProvider),
            store.clone(),
            bus.clone(),
            Arc::new(registry),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Run cancellable tool".into(),
        };
        let run_id = runner.send_message(params).await.expect("send accepted");

        // Wait until the tool has started before cancelling.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let mut accumulated = Vec::new();
        let mut saw_tool_started = false;
        while !saw_tool_started {
            tokio::time::sleep(Duration::from_millis(5)).await;
            accumulated.append(&mut bus.take_events().await);
            saw_tool_started = accumulated.iter().any(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::ToolStarted { run_id: rid, .. } if rid == &run_id
                )
            });
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout waiting for tool_started"
            );
        }

        runner.cancel_run().await.expect("cancel succeeds");
        runner.wait_until_idle().await;

        let mut events = accumulated;
        events.append(&mut bus.take_events().await);

        let cancelled_events: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id
                )
            })
            .collect();
        let finished_cancelled: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunFinished {
                        run_id: rid,
                        status: RunStatus::Cancelled,
                        ..
                    } if rid == &run_id
                )
            })
            .collect();

        assert_eq!(
            cancelled_events.len(),
            1,
            "should emit exactly one RunCancelled event"
        );
        assert_eq!(
            finished_cancelled.len(),
            1,
            "should emit exactly one RunFinished(Cancelled) event"
        );

        let view = load_view(store, "s1").await;
        let roles: Vec<_> = view.messages.iter().map(|message| message.role).collect();
        assert_eq!(
            roles,
            vec![
                MessageRole::Developer,
                MessageRole::Assistant,
                MessageRole::Tool
            ],
            "history should contain developer, completed assistant, and tool result"
        );
        let assistant = view
            .messages
            .iter()
            .find(|message| message.role == MessageRole::Assistant)
            .expect("assistant message persisted");
        assert!(
            assistant.body.0.iter().any(|block| {
                matches!(block, MessageBlock::ToolCall(call) if call.name == "cancellable_wait")
            }),
            "assistant message should contain the completed tool call"
        );
        assert!(
            !view.messages.iter().any(|message| {
                message.role == MessageRole::Assistant
                    && message.body.0.iter().any(|block| {
                        matches!(block, MessageBlock::Text { text } if text.contains("partial"))
                    })
            }),
            "no partial assistant text should be persisted"
        );
    }

    #[tokio::test]
    async fn unknown_tool_represented_as_error_in_next_turn() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().to_path_buf();
        store
            .new_session("s1", workspace.to_str().unwrap())
            .await
            .expect("create session with workspace");

        let services = RuntimeServices::new(
            Arc::new(UnknownToolThenCheckProvider),
            store.clone(),
            bus.clone(),
            Arc::new(MvpToolRegistry::new()),
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        );
        let runner = SessionRunner::new(services);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "Call unknown tool".into(),
        };
        let run_id = runner.send_message(params).await.expect("send accepted");
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunFinished {
                        run_id: rid,
                        status: RunStatus::Succeeded,
                        ..
                    } if rid == &run_id
                )
            }),
            "run should finish successfully after observing tool error"
        );

        let view = load_view(store, "s1").await;
        let tool_messages: Vec<&Message> = view
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .collect();
        assert_eq!(
            tool_messages.len(),
            1,
            "one tool result should be persisted"
        );
        let tool_text = message_text(tool_messages[0]);
        assert!(
            tool_text.to_ascii_lowercase().contains("unknown tool")
                || tool_text.to_ascii_lowercase().contains("error"),
            "tool result should represent an error: {tool_text}"
        );

        let assistant_messages: Vec<&Message> = view
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .collect();
        assert_eq!(
            assistant_messages.len(),
            2,
            "should have assistant tool-call message and final assistant"
        );
        assert!(
            assistant_messages.last().unwrap().body.0.iter().any(|block| {
                matches!(block, MessageBlock::Text { text } if text.contains("observed tool error"))
            }),
            "final assistant should confirm it observed the tool error"
        );
    }

    #[tokio::test]
    async fn invalid_response_fails_run_with_single_terminal_outcome() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let runner = runner_without_tools(
            Arc::new(InvalidResponseProvider),
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
        let run_finished: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::RunFinished { run_id: rid, .. } if rid == &run_id
                )
            })
            .collect();
        assert_eq!(
            run_finished.len(),
            1,
            "should emit exactly one RunFinished event"
        );
        let is_failed_with_invalid_response = if let RuntimeEventKind::RunFinished {
            status: RunStatus::Failed,
            error: Some(ref msg),
            ..
        } = run_finished[0].kind
        {
            msg.contains("malformed response")
        } else {
            false
        };
        assert!(
            is_failed_with_invalid_response,
            "run should fail with invalid response error, got {:?}",
            run_finished[0].kind
        );

        let view = load_view(store, "s1").await;
        assert_eq!(
            view.messages.len(),
            1,
            "only the developer message should be persisted"
        );
        assert_eq!(view.messages[0].role, MessageRole::Developer);
    }

    /// A provider that emits some assistant text and then requests a tool call.
    struct TextAndToolCallProvider;

    #[async_trait]
    impl ModelProvider for TextAndToolCallProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let has_tool_result = messages
                .iter()
                .any(|message| message.role == MessageRole::Tool);
            if has_tool_result {
                let message_id = "msg-final".to_string();
                let events = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        message_id: message_id.clone(),
                        delta: "File read successfully.".into(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: None,
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }

            let message_id = "msg-tool".to_string();
            let call = ToolCall {
                id: "call-read".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "main.rs"}),
            };
            let events = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: "I will read the file.".into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: Some(vec![call]),
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    /// A provider that requests a custom cancellable tool.
    struct CancellableToolProvider;

    #[async_trait]
    impl ModelProvider for CancellableToolProvider {
        async fn send_message(
            &self,
            _messages: Vec<LlmMessage>,
            tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            assert!(
                tools.iter().any(|tool| tool.name == "cancellable_wait"),
                "test should register cancellable_wait"
            );
            let message_id = "msg-cancel".to_string();
            let call = ToolCall {
                id: "call-cancel".into(),
                name: "cancellable_wait".into(),
                arguments: serde_json::json!({}),
            };
            let events = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: Some(vec![call]),
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    /// A provider that requests an unknown tool, then expects the error result in the next turn.
    struct UnknownToolThenCheckProvider;

    #[async_trait]
    impl ModelProvider for UnknownToolThenCheckProvider {
        async fn send_message(
            &self,
            messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let has_tool_result = messages
                .iter()
                .any(|message| message.role == MessageRole::Tool);
            if !has_tool_result {
                let message_id = "msg-1".to_string();
                let call = ToolCall {
                    id: "unknown-call".into(),
                    name: "unknown_tool".into(),
                    arguments: serde_json::json!({}),
                };
                let events = vec![
                    Ok(ProviderEvent::MessageStarted {
                        message_id: message_id.clone(),
                    }),
                    Ok(ProviderEvent::MessageCompleted {
                        message_id,
                        tool_calls: Some(vec![call]),
                    }),
                ];
                return Ok(Box::pin(stream::iter(events)));
            }

            let last_tool = messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Tool);
            let content = if let Some(tool) = last_tool {
                let text = llm_message_text(tool).to_ascii_lowercase();
                if text.contains("unknown tool") || text.contains("error") {
                    "observed tool error"
                } else {
                    "no tool error observed"
                }
            } else {
                "no tool result"
            };
            let message_id = "msg-2".to_string();
            let events = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: content.into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: None,
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    /// A provider that immediately fails with an invalid response error.
    struct InvalidResponseProvider;

    #[async_trait]
    impl ModelProvider for InvalidResponseProvider {
        async fn send_message(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            Err(ProviderError::InvalidResponse("malformed response".into()))
        }
    }

    /// A tool that waits on the cancellation token and returns an error when cancelled.
    struct CancellableWaitTool;

    #[async_trait]
    impl Tool for CancellableWaitTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "cancellable_wait".into(),
                description: "Wait until cancelled or timeout".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            }
        }

        async fn invoke(
            &self,
            _call: &ToolCall,
            _ctx: &SessionContext,
            cancel: &CancellationToken,
        ) -> Result<ToolOutputStream, ToolError> {
            let (tx, rx) = mpsc::unbounded();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                let _ = tx.unbounded_send(Ok(ToolStreamEvent::Chunk {
                    chunk: "started".into(),
                }));
                tokio::select! {
                    () = cancel.cancelled() => {
                        let _ = tx.unbounded_send(Ok(ToolStreamEvent::Done {
                            result: ToolOutputResult::error("cancelled by user"),
                        }));
                    }
                    () = tokio::time::sleep(Duration::from_secs(30)) => {
                        let _ = tx.unbounded_send(Ok(ToolStreamEvent::Done {
                            result: ToolOutputResult::success("completed without cancellation"),
                        }));
                    }
                }
            });
            Ok(Box::pin(rx))
        }
    }

    fn llm_message_text(message: &LlmMessage) -> String {
        message
            .body
            .0
            .iter()
            .filter_map(|block| {
                if let MessageBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect()
    }
}
