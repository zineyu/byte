use std::collections::HashMap;
use std::sync::Arc;

use byte_protocol::{
    CancelRunParams, CancelRunResult, DeleteSessionResult, ListSessionsResult, LoadSessionResult,
    NewSessionResult, RuntimeEventKind, SessionChangeAction,
};
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

use crate::runner::{RunnerError, SessionRunner};
use crate::runtime_services::RuntimeServices;

/// Manages the lifecycle of per-session runners and session CRUD.
///
/// A `SessionManager` is a long-lived service that owns the mapping from
/// session id to [`SessionRunner`]. Runners are created lazily the first time
/// a session receives a message. When a session is deleted its runner is
/// removed from the map so that the manager does not leak runners across
/// session lifecycles.
#[derive(Clone)]
pub struct SessionManager {
    services: RuntimeServices,
    runners: Arc<Mutex<HashMap<String, Arc<SessionRunner>>>>,
}

impl SessionManager {
    /// Create a new session manager with the given shared runtime services.
    pub fn new(services: RuntimeServices) -> Self {
        Self {
            services,
            runners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new session file with an optional workspace path.
    ///
    /// Emits `session_changed(Created)` on success.
    #[instrument(skip(self))]
    pub async fn new_session(
        &self,
        session_id: &str,
        workspace: Option<&str>,
    ) -> Result<NewSessionResult, RunnerError> {
        self.services
            .store
            .new_session(session_id, workspace)
            .await?;
        self.emit_session_changed(session_id.to_owned(), SessionChangeAction::Created)
            .await;
        info!(%session_id, "session created");
        Ok(NewSessionResult {
            session_id: session_id.to_owned(),
        })
    }

    /// Load a session view by id.
    ///
    /// Emits `session_changed(Loaded)` on success.
    #[instrument(skip(self))]
    pub async fn load_session(&self, session_id: &str) -> Result<LoadSessionResult, RunnerError> {
        let session = self.services.store.load_session(session_id).await?;
        self.emit_session_changed(session_id.to_owned(), SessionChangeAction::Loaded)
            .await;
        debug!(%session_id, message_count = session.messages.len(), "session loaded");
        Ok(LoadSessionResult { session })
    }

    /// List all sessions ordered by created time descending.
    pub async fn list_sessions(&self) -> Result<ListSessionsResult, RunnerError> {
        let sessions = self.services.store.list_sessions().await?;
        Ok(ListSessionsResult { sessions })
    }

    /// Delete a session file if it exists.
    ///
    /// Returns `RunnerError::Busy` if the session has an active run. The
    /// runner is removed from the internal map on success so that a later
    /// session with the same id gets a fresh runner.
    ///
    /// The runners map lock is held across the active-run check, file deletion
    /// and runner removal so that no other task can observe a deleted file
    /// while the old runner is still reachable from the map.
    #[instrument(skip(self))]
    pub async fn delete_session(
        &self,
        session_id: &str,
    ) -> Result<DeleteSessionResult, RunnerError> {
        let mut runners = self.runners.lock().await;

        if let Some(runner) = runners.get(session_id).cloned() {
            let active = runner.active_run_guard().await;
            if active.is_some() {
                return Err(RunnerError::Busy);
            }
            self.services.store.delete_session(session_id).await?;
            runners.remove(session_id);
        } else {
            drop(runners);
            self.services.store.delete_session(session_id).await?;
        }

        self.emit_session_changed(session_id.to_owned(), SessionChangeAction::Deleted)
            .await;
        info!(%session_id, "session deleted");
        Ok(DeleteSessionResult {
            session_id: session_id.to_owned(),
        })
    }

    /// Start a run for the given session, lazily creating a runner if needed.
    ///
    /// Concurrent runs on the same session return `RunnerError::Busy` from the
    /// underlying runner.
    #[instrument(skip(self, params))]
    pub async fn send_message(
        &self,
        params: byte_protocol::SendMessageParams,
    ) -> Result<crate::runner::RunId, RunnerError> {
        let runner = self.runner_for(&params.session_id).await;
        runner.send_message(params).await
    }

    /// Cancel the active run for a session, if any.
    ///
    /// Returns success immediately when the session has no runner or no active
    /// run.
    #[instrument(skip(self))]
    pub async fn cancel_run(
        &self,
        params: CancelRunParams,
    ) -> Result<CancelRunResult, RunnerError> {
        let runners = self.runners.lock().await;
        if let Some(runner) = runners.get(&params.session_id).cloned() {
            drop(runners);
            runner.cancel_run().await?;
        }
        Ok(CancelRunResult {})
    }

    /// Return true if the session has an active run.
    pub async fn is_running(&self, session_id: &str) -> bool {
        let runners = self.runners.lock().await;
        if let Some(runner) = runners.get(session_id) {
            runner.is_running().await
        } else {
            false
        }
    }

    /// Get or create the runner for a session.
    async fn runner_for(&self, session_id: &str) -> Arc<SessionRunner> {
        let mut runners = self.runners.lock().await;
        runners
            .entry(session_id.to_owned())
            .or_insert_with(|| Arc::new(SessionRunner::new(self.services.clone())))
            .clone()
    }

    async fn emit_session_changed(&self, session_id: String, action: SessionChangeAction) {
        self.services
            .event_bus
            .emit(RuntimeEventKind::SessionChanged { session_id, action })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use byte_models::{EchoProvider, ModelProvider};
    use byte_protocol::{
        CancelRunParams, RunStatus, RuntimeEventKind, SendMessageParams, SessionChangeAction,
    };
    use byte_session::SessionStore;
    use byte_tools::{AllowAllPolicy, MvpToolRegistry, ReadFileTool, ToolRegistry};
    use tempfile::tempdir;

    use crate::event_bus::{RecordingEventBus, RuntimeEventBus};
    use crate::runtime_services::RuntimeServices;

    use super::SessionManager;

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    fn services(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<dyn RuntimeEventBus>,
    ) -> RuntimeServices {
        let mut registry = MvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        RuntimeServices::new(provider, store, bus, Arc::new(registry))
    }

    fn services_without_tools(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<dyn RuntimeEventBus>,
    ) -> RuntimeServices {
        RuntimeServices::new(provider, store, bus, Arc::new(MvpToolRegistry::new()))
    }

    fn manager() -> (SessionManager, Arc<RecordingEventBus>, Arc<SessionStore>) {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&recording_bus) as Arc<dyn RuntimeEventBus>;
        let manager = SessionManager::new(services(provider, Arc::clone(&store), bus));
        (manager, recording_bus, store)
    }
    fn manager_without_tools() -> (SessionManager, Arc<RecordingEventBus>, Arc<SessionStore>) {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&recording_bus) as Arc<dyn RuntimeEventBus>;
        let manager =
            SessionManager::new(services_without_tools(provider, Arc::clone(&store), bus));
        (manager, recording_bus, store)
    }

    #[tokio::test]
    async fn new_session_creates_file_and_emits_created_event() {
        let (manager, bus, store) = manager();
        let result = manager
            .new_session("s1", Some("/tmp/ws"))
            .await
            .expect("new session succeeds");

        assert_eq!(result.session_id, "s1");

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(view.workspace.as_deref(), Some("/tmp/ws"));

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].kind, RuntimeEventKind::SessionChanged { session_id, action } if session_id == "s1" && *action == SessionChangeAction::Created),
            "should emit session_changed(Created)"
        );
    }

    #[tokio::test]
    async fn delete_session_removes_file_and_emits_deleted_event() {
        let (manager, bus, store) = manager();
        manager.new_session("s1", None).await.unwrap();
        bus.take_events().await;

        let result = manager.delete_session("s1").await.expect("delete succeeds");

        assert_eq!(result.session_id, "s1");
        assert!(matches!(
            store.load_session("s1").await.unwrap_err(),
            byte_session::SessionError::NotFound(id) if id == "s1"
        ));

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].kind, RuntimeEventKind::SessionChanged { session_id, action } if session_id == "s1" && *action == SessionChangeAction::Deleted),
            "should emit session_changed(Deleted)"
        );
    }

    #[tokio::test]
    async fn delete_session_with_active_run_returns_busy() {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider {
            delay: Duration::from_millis(50),
            ..Default::default()
        });
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&recording_bus) as Arc<dyn RuntimeEventBus>;
        let manager = SessionManager::new(services(provider, Arc::clone(&store), bus));
        manager.new_session("s1", None).await.unwrap();
        recording_bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };
        manager.send_message(params).await.expect("run accepted");

        let err = manager.delete_session("s1").await.expect_err("busy");
        assert!(matches!(err, crate::runner::RunnerError::Busy));
        assert!(
            store.load_session("s1").await.is_ok(),
            "session file must not be deleted while a run is active"
        );

        // Cleanup active run before exit to avoid leaking tasks.
        let runner = manager.runner_for("s1").await;
        runner.wait_until_idle().await;
    }

    #[tokio::test]
    async fn send_message_lazily_creates_runner_and_rejects_concurrent_runs() {
        let (manager, bus, store) = manager_without_tools();
        bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };
        let run_id = manager
            .send_message(params.clone())
            .await
            .expect("first run accepted");

        let second = manager.send_message(params).await;
        assert!(matches!(second, Err(crate::runner::RunnerError::Busy)));

        let runner = manager.runner_for("s1").await;
        runner.wait_until_idle().await;

        let events = bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::RunStarted { session_id, run_id: rid } if session_id == "s1" && rid == &run_id)),
            "should emit run_started"
        );
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::RunFinished { status: RunStatus::Succeeded, run_id: rid, .. } if rid == &run_id)),
            "should emit successful run_finished"
        );

        let view = store.load_session("s1").await.unwrap();
        assert_eq!(view.messages.len(), 2);
    }

    #[tokio::test]
    async fn runs_on_different_sessions_execute_concurrently() {
        let (manager, _bus, _store) = manager();
        manager.new_session("s1", None).await.unwrap();
        manager.new_session("s2", None).await.unwrap();

        let first = manager
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "a".into(),
            })
            .await;
        let second = manager
            .send_message(SendMessageParams {
                session_id: "s2".into(),
                message: "b".into(),
            })
            .await;

        assert!(first.is_ok(), "s1 run should be accepted");
        assert!(second.is_ok(), "s2 run should be accepted");
        assert_ne!(first.unwrap(), second.unwrap());

        let r1 = manager.runner_for("s1").await;
        let r2 = manager.runner_for("s2").await;
        r1.wait_until_idle().await;
        r2.wait_until_idle().await;
    }

    #[tokio::test]
    async fn load_session_emits_loaded_event() {
        let (manager, bus, _store) = manager();
        manager.new_session("s1", Some("/workspace")).await.unwrap();
        bus.take_events().await;

        let result = manager.load_session("s1").await.expect("load succeeds");
        assert_eq!(result.session.session_id, "s1");

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].kind, RuntimeEventKind::SessionChanged { session_id, action } if session_id == "s1" && *action == SessionChangeAction::Loaded),
            "should emit session_changed(Loaded)"
        );
    }

    #[tokio::test]
    async fn list_sessions_returns_sessions() {
        let (manager, _bus, _store) = manager();
        manager.new_session("s1", Some("/a")).await.unwrap();
        manager.new_session("s2", Some("/b")).await.unwrap();

        let result = manager.list_sessions().await.expect("list succeeds");
        let ids: Vec<_> = result
            .sessions
            .iter()
            .map(|summary| summary.session_id.clone())
            .collect();
        assert!(ids.contains(&"s1".to_owned()));
        assert!(ids.contains(&"s2".to_owned()));
    }

    #[tokio::test]
    async fn cancel_run_forwards_to_runner() {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider {
            chunk_size: 1,
            delay: Duration::from_millis(10),
        });
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&recording_bus) as Arc<dyn RuntimeEventBus>;
        let manager =
            SessionManager::new(services_without_tools(provider, Arc::clone(&store), bus));
        manager.new_session("s1", None).await.unwrap();
        recording_bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };
        let run_id = manager.send_message(params).await.expect("run accepted");
        tokio::time::sleep(Duration::from_millis(20)).await;

        manager
            .cancel_run(CancelRunParams {
                session_id: "s1".into(),
            })
            .await
            .expect("cancel succeeds");

        let events = recording_bus.take_events().await;
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::RunCancelled { run_id: rid } if rid == &run_id)),
            "should emit run_cancelled"
        );
        assert!(
            events.iter().any(|event| matches!(&event.kind, RuntimeEventKind::RunFinished { run_id: rid, status: RunStatus::Cancelled, .. } if rid == &run_id)),
            "should emit run_finished(Cancelled)"
        );

        let view = store.load_session("s1").await.expect("session loads");
        assert_eq!(
            view.messages.len(),
            1,
            "assistant message should not be persisted"
        );
    }

    #[tokio::test]
    async fn cancel_run_without_runner_succeeds() {
        let (manager, bus, _store) = manager();
        manager.new_session("s1", None).await.unwrap();
        bus.take_events().await;

        let result = manager
            .cancel_run(CancelRunParams {
                session_id: "s1".into(),
            })
            .await;
        assert!(
            result.is_ok(),
            "cancel_run should succeed when session has no runner"
        );
    }
}
