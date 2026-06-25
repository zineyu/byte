use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use byte_models::{ModelProvider, ProviderError, ProviderEvent};
use byte_protocol::{
    MessageRole, RunMessage, RunStatus, RuntimeEvent, RuntimeEventKind, SendMessageParams,
};
use byte_session::{SessionError, SessionStore};
use futures::StreamExt;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument};

use crate::event_bus::RuntimeEventBus;

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
    active_run: Arc<Mutex<Option<RunId>>>,
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
        active.replace(run_id.clone());
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
        };

        tokio::spawn(async move {
            executor.run(runner).await;
        });

        Ok(run_id)
    }

    #[cfg(test)]
    #[cfg(test)]
    /// Wait until there is no active run.
    ///
    /// Useful in tests to observe the full event sequence.
    pub async fn wait_until_idle(&self) {
        loop {
            let active = self.active_run.lock().await;
            if active.is_none() {
                return;
            }
            drop(active);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
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

        while let Some(event) = stream.next().await {
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
                        runner
                            .emit(RuntimeEventKind::MessageDelta {
                                run_id: self.run_id.clone(),
                                message_id: id,
                                delta,
                            })
                            .await;
                    }
                }
                Ok(ProviderEvent::MessageCompleted { message_id: id }) => {
                    if message_id.as_ref() == Some(&id) {
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

    use async_trait::async_trait;
    use byte_models::{EchoProvider, ModelProvider, ProviderError, ProviderStream};
    use byte_protocol::{MessageRole, RunMessage, RunStatus, RuntimeEventKind, SendMessageParams};
    use byte_session::SessionStore;
    use tempfile::tempdir;

    use crate::event_bus::RecordingEventBus;

    use super::{RunnerError, SessionRunner};

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
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
}
