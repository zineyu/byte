use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use byte_models::{ModelProvider, ProviderError, ProviderEvent};
use byte_protocol::{
    CancelRunResult, MessageRole, RunMessage, RunStatus, RuntimeEvent, RuntimeEventKind,
    SendMessageParams,
};
use byte_session::{SessionError, SessionStore};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

use crate::event_bus::RuntimeEventBus;

/// Buffers provider text deltas until a size threshold is reached so that
/// cancellation can flush any remaining content as a final `message_delta`
/// before emitting `run_cancelled`.
pub struct DeltaBuffer {
    threshold: usize,
    buffer: String,
}

impl DeltaBuffer {
    /// Create a new buffer with the given character threshold.
    pub fn new(threshold: usize) -> Self {
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

pub type RunId = String;

/// Errors that can occur when starting or executing a run.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("session is busy with another run")]
    Busy,
    #[error(transparent)]
    SessionStore(#[from] SessionError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Owns the conversation loop for a single session.
///
/// A runner instance enforces the "one active run per session" constraint and
/// emits lifecycle runtime events during the run.
#[derive(Clone)]
pub struct SessionRunner {
    provider: Arc<dyn ModelProvider>,
    store: Arc<SessionStore>,
    bus: Arc<dyn RuntimeEventBus>,
    sequence: Arc<AtomicU64>,
    active_run: Arc<Mutex<Option<(RunId, CancellationToken)>>>,
}

impl SessionRunner {
    /// Create a new runner for one session.
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<dyn RuntimeEventBus>,
    ) -> Self {
        Self {
            provider,
            store,
            bus,
            sequence: Arc::new(AtomicU64::new(0)),
            active_run: Arc::new(Mutex::new(None)),
        }
    }

    /// Start a single-turn model run.
    ///
    /// Returns immediately with the run id; the run itself is executed on a
    /// background task.
    pub async fn send_message(&self, params: SendMessageParams) -> Result<RunId, RunnerError> {
        let mut active = self.active_run.lock().await;
        if active.is_some() {
            return Err(RunnerError::Busy);
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        active.replace((run_id.clone(), token.clone()));
        drop(active);

        if let Err(error) = self.store.new_session(&params.session_id, None).await {
            self.clear_active_run().await;
            return Err(RunnerError::SessionStore(error));
        }

        let parent_id = match self.store.load_session(&params.session_id).await {
            Ok(view) => view.messages.last().map(|message| message.id.clone()),
            Err(error) => {
                self.clear_active_run().await;
                return Err(RunnerError::SessionStore(error));
            }
        };

        let developer_message_id = match self
            .store
            .append_message(
                &params.session_id,
                None,
                parent_id.as_deref(),
                MessageRole::Developer,
                &params.message,
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
            cancel_token: token.child_token(),
        };

        tokio::spawn(async move {
            executor.run(runner).await;
        });

        Ok(run_id)
    }

    /// Cancel the active run for this session, if any.
    ///
    /// This is an idempotent no-op when there is no active run. When a run is
    /// active, the cancellation token is triggered and the caller waits until
    /// the run task has cleaned up `active_run` before returning.
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

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn emit(&self, kind: RuntimeEventKind) {
        let event = RuntimeEvent {
            sequence: self.next_sequence(),
            kind,
        };
        self.bus.emit(event).await;
    }

    async fn clear_active_run(&self) {
        *self.active_run.lock().await = None;
    }

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

struct RunExecutor {
    run_id: RunId,
    session_id: String,
    message: String,
    developer_message_id: String,
    cancel_token: CancellationToken,
}

impl RunExecutor {
    #[instrument(skip_all, fields(run_id, session_id))]
    async fn run(self, runner: Arc<SessionRunner>) {
        tracing::Span::current().record("run_id", &self.run_id);
        tracing::Span::current().record("session_id", &self.session_id);
        info!("starting run");

        runner
            .emit(RuntimeEventKind::RunStarted {
                session_id: self.session_id.clone(),
                run_id: self.run_id.clone(),
            })
            .await;

        let messages = vec![RunMessage {
            role: MessageRole::Developer,
            content: self.message,
        }];

        let mut stream = match runner.provider.send_message(messages).await {
            Ok(stream) => stream,
            Err(error) => {
                error!(%error, "provider request failed");
                runner.emit_run_error(&self.run_id, error.to_string()).await;
                runner.clear_active_run().await;
                return;
            }
        };
        let mut message_id: Option<String> = None;
        let mut assistant_content = String::new();
        let mut completed = false;
        let mut delta_buffer = DeltaBuffer::new(8);

        loop {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => {
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
                    runner.emit(RuntimeEventKind::RunFinished {
                        run_id: self.run_id.clone(),
                        status: RunStatus::Cancelled,
                        error: None,
                    }).await;
                    info!("run cancelled");
                    runner.clear_active_run().await;
                    return;
                }
                maybe_event = stream.next() => {
                    match maybe_event {
                        Some(event) => {
                            match event {
                                Ok(ProviderEvent::MessageStarted { message_id: id }) => {
                                    debug!(message_id = %id, "assistant message started");
                                    message_id = Some(id.clone());
                                    assistant_content.clear();
                                    completed = false;
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
                                    if message_id.as_ref() == Some(&id) {
                                        assistant_content.push_str(&delta);
                                        if let Some(flush) = delta_buffer.push(&delta) {
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
                                Ok(ProviderEvent::MessageCompleted { message_id: id }) => {
                                    if message_id.as_ref() == Some(&id) {
                                        if let Some(flush) = delta_buffer.flush() {
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
                                            })
                                            .await;

                                        if let Err(error) = runner
                                            .store
                                            .append_message(
                                                &self.session_id,
                                                Some(&id),
                                                Some(&self.developer_message_id),
                                                MessageRole::Assistant,
                                                &assistant_content,
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
                                            return;
                                        }

                                        completed = true;
                                    }
                                }
                                Err(error) => {
                                    error!(%error, "provider stream error");
                                    runner.emit_run_error(&self.run_id, error.to_string()).await;
                                    runner.clear_active_run().await;
                                    return;
                                }
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        if completed {
            runner
                .emit(RuntimeEventKind::RunFinished {
                    run_id: self.run_id.clone(),
                    status: RunStatus::Succeeded,
                    error: None,
                })
                .await;
            info!("run finished successfully");
        } else {
            runner
                .emit_run_error(
                    &self.run_id,
                    "provider stream ended without completing the assistant message".into(),
                )
                .await;
        }
        runner.clear_active_run().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use byte_models::{EchoProvider, ModelProvider, ProviderError, ProviderStream};
    use byte_protocol::{MessageRole, RunMessage, RunStatus, RuntimeEventKind, SendMessageParams};
    use byte_session::SessionStore;
    use tempfile::tempdir;

    use crate::event_bus::RecordingEventBus;

    use super::{DeltaBuffer, RunnerError, SessionRunner};

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
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
        let provider = Arc::new(EchoProvider::default());
        let store = temp_store();
        let bus = Arc::new(RecordingEventBus::new());
        let runner = SessionRunner::new(provider, store, bus);

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };
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
        let provider = Arc::new(EchoProvider::default());
        let runner = SessionRunner::new(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };

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
            matches!(events.last().unwrap().kind, RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, error: None, run_id: ref rid } if rid == &run_id),
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
        let provider = Arc::new(EchoProvider {
            chunk_size: 3,
            ..Default::default()
        });
        let runner = SessionRunner::new(provider, temp_store(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello world".into(),
        };

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
        ) -> Result<ProviderStream, ProviderError> {
            Err(ProviderError::Request("boom".into()))
        }
    }

    #[tokio::test]
    async fn provider_error_emits_error_and_failed_run() {
        let bus = Arc::new(RecordingEventBus::new());
        let store = temp_store();
        let provider = Arc::new(BoomProvider);
        let runner = SessionRunner::new(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };

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
            matches!(events.last().unwrap().kind, RuntimeEventKind::RunFinished { status: RunStatus::Failed, error: Some(ref msg), run_id: ref rid } if rid == &run_id && msg.contains("boom")),
            "last event should be failed run_finished with boom"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, RuntimeEventKind::MessageStarted { .. })),
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
        let runner = SessionRunner::new(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };

        let run_id = runner.send_message(params).await.expect("send accepted");
        // Give the provider stream time to emit MessageStarted and some deltas.
        tokio::time::sleep(Duration::from_millis(30)).await;

        runner.cancel_run().await.expect("cancel succeeds");

        let events = bus.take_events().await;
        let run_events: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    RuntimeEventKind::RunStarted { .. }
                        | RuntimeEventKind::RunFinished { .. }
                        | RuntimeEventKind::RunCancelled { .. }
                        | RuntimeEventKind::MessageStarted { .. }
                        | RuntimeEventKind::MessageDelta { .. }
                )
            })
            .collect();

        assert!(
            matches!(run_events[0].kind, RuntimeEventKind::RunStarted { run_id: ref rid, .. } if rid == &run_id)
        );
        assert!(
            run_events.iter().any(|event| matches!(event.kind, RuntimeEventKind::MessageStarted { run_id: ref rid, .. } if rid == &run_id)),
            "should emit message_started"
        );

        let last_three: Vec<_> = run_events.iter().rev().take(3).rev().copied().collect();
        assert_eq!(last_three.len(), 3, "should have at least three run events");
        assert!(
            matches!(last_three[0].kind, RuntimeEventKind::MessageDelta { run_id: ref rid, .. } if rid == &run_id),
            "second-to-last event before run_cancelled should be a message_delta flush"
        );
        assert!(
            matches!(last_three[1].kind, RuntimeEventKind::RunCancelled { run_id: ref rid } if rid == &run_id),
            "should emit run_cancelled"
        );
        assert!(
            matches!(last_three[2].kind, RuntimeEventKind::RunFinished { run_id: ref rid, status: RunStatus::Cancelled, error: None } if rid == &run_id),
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
        let runner = SessionRunner::new(Arc::new(EchoProvider::default()), temp_store(), bus);

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
        let runner = SessionRunner::new(provider, store.clone(), bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };

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
                    event.kind,
                    RuntimeEventKind::RunFinished { run_id: ref rid, .. } if rid == &second_id
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
        let runner = SessionRunner::new(provider, store, bus.clone());
        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hello".into(),
        };

        let run_id = runner.send_message(params).await.expect("send accepted");
        tokio::time::sleep(Duration::from_millis(10)).await;

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

        let events = bus.take_events().await;
        let cancelled_events: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind, RuntimeEventKind::RunCancelled { run_id: ref rid } if rid == &run_id))
            .collect();
        assert_eq!(
            cancelled_events.len(),
            1,
            "should emit exactly one run_cancelled event"
        );
    }
}
