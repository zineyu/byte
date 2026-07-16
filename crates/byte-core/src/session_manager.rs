use byte_protocol::{
    CancelRunParams, CancelRunResult, DeleteSessionResult, ListSessionsResult, LoadSessionResult,
    NewSessionResult, RuntimeEventKind, SessionChangeAction,
};
use tracing::{debug, info, instrument};

use crate::runner::RunnerError;
use crate::runner_pool::{CloseResult, RunnerPool};
use crate::runtime_services::RuntimeServices;

/// Manages the lifecycle of sessions and delegates runner lifecycle to a
/// [`RunnerPool`].
///
/// A `SessionManager` is a long-lived service that handles session CRUD and
/// routes run requests to runners managed by the pool. Runners are created
/// lazily by the pool and can be closed when the session is deleted or when
/// the pool decides to reclaim idle memory.
#[derive(Clone)]
pub struct SessionManager {
    /// Aggregated runtime services.
    services: RuntimeServices,
    /// Pool that owns the mapping from session id to cached runner.
    pool: RunnerPool,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager").finish_non_exhaustive()
    }
}

impl SessionManager {
    /// Create a new session manager with the given shared runtime services.
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        Self {
            pool: RunnerPool::new(services.clone()),
            services,
        }
    }

    /// Create a new session file bound to `workspace`.
    ///
    /// The workspace path must exist and be a directory. Emits
    /// `session_changed(Created)` on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the workspace is invalid or the session cannot be
    /// created.
    #[instrument(skip(self))]
    pub async fn new_session(
        &self,
        session_id: &str,
        workspace: &str,
    ) -> Result<NewSessionResult, RunnerError> {
        let path = std::path::Path::new(workspace);
        if !path.is_dir() {
            return Err(RunnerError::SessionStore(
                byte_session::SessionError::InvalidDirectory(format!(
                    "workspace is not a directory: {workspace}"
                )),
            ));
        }

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
    ///
    /// # Errors
    ///
    /// Returns an error if the session cannot be loaded.
    #[instrument(skip(self))]
    pub async fn load_session(&self, session_id: &str) -> Result<LoadSessionResult, RunnerError> {
        let session = self.services.view_repo.load_session(session_id).await?;
        self.emit_session_changed(session_id.to_owned(), SessionChangeAction::Loaded)
            .await;
        debug!(%session_id, message_count = session.messages.len(), "session loaded");
        Ok(LoadSessionResult { session })
    }

    /// List all sessions ordered by created time descending.
    ///
    /// # Errors
    ///
    /// Returns an error if the session list cannot be read.
    pub async fn list_sessions(&self) -> Result<ListSessionsResult, RunnerError> {
        let sessions = self.services.store.list_sessions().await?;
        Ok(ListSessionsResult { sessions })
    }

    /// Delete a session file if it exists.
    ///
    /// Returns `RunnerError::Busy` if the session has an active run. The
    /// cached runner is closed before the session file is deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is busy or cannot be deleted.
    #[instrument(skip(self))]
    pub async fn delete_session(
        &self,
        session_id: &str,
    ) -> Result<DeleteSessionResult, RunnerError> {
        match self.pool.close(session_id).await {
            CloseResult::Busy => return Err(RunnerError::Busy),
            CloseResult::Absent | CloseResult::Closed => {}
        }

        self.services.store.delete_session(session_id).await?;

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
    ///
    /// # Errors
    ///
    /// Returns an error if the runner cannot be created or the run cannot be
    /// started.
    #[instrument(skip(self, params))]
    pub async fn send_message(
        &self,
        params: byte_protocol::SendMessageParams,
    ) -> Result<crate::runner::RunId, RunnerError> {
        let runner = self.pool.get_or_create(&params.session_id).await;
        runner.send_message(params).await
    }

    /// Cancel the active run for a session, if any.
    ///
    /// Returns success immediately when the session has no runner or no active
    /// run.
    ///
    /// # Errors
    ///
    /// Returns an error if the active run cannot be cancelled.
    #[instrument(skip(self))]
    pub async fn cancel_run(
        &self,
        params: CancelRunParams,
    ) -> Result<CancelRunResult, RunnerError> {
        let runner = self.pool.get_or_create(&params.session_id).await;
        let _ = runner.cancel_run().await?;
        Ok(CancelRunResult {})
    }

    /// Return true if the session has an active run.
    pub async fn is_running(&self, session_id: &str) -> bool {
        self.pool.get_or_create(session_id).await.is_running().await
    }

    /// Emit a session-changed runtime event.
    async fn emit_session_changed(&self, session_id: String, action: SessionChangeAction) {
        self.services
            .event_bus
            .emit(RuntimeEventKind::SessionChanged { session_id, action })
            .await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;
    use std::time::Duration;

    use byte_models::{EchoProvider, ModelProvider};
    use byte_protocol::{
        CancelRunParams, RunStatus, RuntimeEventKind, SendMessageParams, SessionChangeAction,
    };
    use byte_session::SessionStore;
    use byte_skills::{MvpSkillRegistry, SkillRegistry};
    use byte_tools::{
        AllowAllPolicy, MvpToolRegistry as ByteToolsMvpToolRegistry, ReadFileTool, ToolRegistry,
    };
    use tempfile::tempdir;

    use crate::SessionViewRepository;
    use crate::compaction::CompactionConfig;
    use crate::event_bus::{RecordingEventBus, RuntimeEventBus};
    use crate::runtime_services::RuntimeServices;

    use super::SessionManager;

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    fn temp_workspace() -> String {
        // Use a persistent temp directory so the path still exists after this
        // helper returns. Tests only need a valid directory for workspace
        // validation; per-test isolation is provided by the session store.
        let dir = std::env::temp_dir().join("byte-test-workspace");
        std::fs::create_dir_all(&dir).expect("create temp workspace");
        dir.to_str()
            .expect("workspace path is valid UTF-8")
            .to_owned()
    }

    fn empty_skill_registry() -> Arc<dyn SkillRegistry> {
        Arc::new(MvpSkillRegistry::new())
    }

    fn services(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<dyn RuntimeEventBus>,
    ) -> RuntimeServices {
        let mut registry = ByteToolsMvpToolRegistry::new();
        registry.register(
            "read_file".to_string(),
            Arc::new(ReadFileTool),
            Arc::new(AllowAllPolicy),
        );
        RuntimeServices::new(
            provider,
            store,
            bus,
            Arc::new(registry),
            empty_skill_registry(),
            CompactionConfig::default(),
        )
    }

    fn services_without_tools(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        bus: Arc<dyn RuntimeEventBus>,
    ) -> RuntimeServices {
        RuntimeServices::new(
            provider,
            store,
            bus,
            Arc::new(ByteToolsMvpToolRegistry::new()),
            empty_skill_registry(),
            CompactionConfig::default(),
        )
    }

    fn manager() -> (
        SessionManager,
        Arc<RecordingEventBus>,
        Arc<SessionStore>,
        Arc<SessionViewRepository>,
        String,
    ) {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let view_repo = Arc::new(SessionViewRepository::new(Arc::clone(&store)));
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let manager = SessionManager::new(services(provider, Arc::clone(&store), bus));
        let workspace = temp_workspace();
        (manager, recording_bus, store, view_repo, workspace)
    }
    fn manager_without_tools() -> (
        SessionManager,
        Arc<RecordingEventBus>,
        Arc<SessionStore>,
        Arc<SessionViewRepository>,
        String,
    ) {
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let view_repo = Arc::new(SessionViewRepository::new(Arc::clone(&store)));
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let manager =
            SessionManager::new(services_without_tools(provider, Arc::clone(&store), bus));
        let workspace = temp_workspace();
        (manager, recording_bus, store, view_repo, workspace)
    }

    #[tokio::test]
    async fn new_session_creates_file_and_emits_created_event() {
        let (manager, bus, _store, view_repo, workspace) = manager();
        let result = manager
            .new_session("s1", &workspace)
            .await
            .expect("new session succeeds");

        assert_eq!(result.session_id, "s1");

        let view = view_repo.load_session("s1").await.expect("session loads");
        assert_eq!(view.workspace, workspace);

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].kind, RuntimeEventKind::SessionChanged { session_id, action } if session_id == "s1" && *action == SessionChangeAction::Created),
            "should emit session_changed(Created)"
        );
    }

    #[tokio::test]
    async fn delete_session_removes_file_and_emits_deleted_event() {
        let (manager, bus, _store, view_repo, workspace) = manager();
        manager.new_session("s1", &workspace).await.unwrap();
        bus.take_events().await;

        let result = manager.delete_session("s1").await.expect("delete succeeds");

        assert_eq!(result.session_id, "s1");
        assert!(matches!(
            view_repo.load_session("s1").await.unwrap_err(),
            crate::SessionViewError::Store(byte_session::SessionError::NotFound(id)) if id == "s1"
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
        let workspace = temp_workspace();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider {
            delay: Duration::from_millis(50),
            ..Default::default()
        });
        let store = temp_store();
        let view_repo = Arc::new(SessionViewRepository::new(Arc::clone(&store)));
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let manager = SessionManager::new(services(provider, Arc::clone(&store), bus));
        manager.new_session("s1", &workspace).await.unwrap();
        recording_bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "hi".into(),
        };
        manager.send_message(params).await.expect("run accepted");

        let err = manager.delete_session("s1").await.expect_err("busy");
        assert!(matches!(err, crate::runner::RunnerError::Busy));
        assert!(
            view_repo.load_session("s1").await.is_ok(),
            "session file must not be deleted while a run is active"
        );

        // Cleanup active run before exit to avoid leaking tasks.
        let runner = manager.pool.get_or_create("s1").await;
        runner.wait_until_idle().await;
    }

    #[tokio::test]
    async fn send_message_lazily_creates_runner_and_rejects_concurrent_runs() {
        let (manager, bus, _store, view_repo, workspace) = manager_without_tools();
        manager.new_session("s1", &workspace).await.unwrap();
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

        let runner = manager.pool.get_or_create("s1").await;
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

        let view = view_repo.load_session("s1").await.unwrap();
        assert_eq!(view.messages.len(), 2);
    }

    #[tokio::test]
    async fn runs_on_different_sessions_execute_concurrently() {
        let (manager, _bus, _store, _view_repo, workspace) = manager();
        manager.new_session("s1", &workspace).await.unwrap();
        manager.new_session("s2", &workspace).await.unwrap();

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

        let r1 = manager.pool.get_or_create("s1").await;
        let r2 = manager.pool.get_or_create("s2").await;
        r1.wait_until_idle().await;
        r2.wait_until_idle().await;
    }

    #[tokio::test]
    async fn load_session_emits_loaded_event() {
        let (manager, bus, _store, _view_repo, workspace) = manager();
        manager.new_session("s1", &workspace).await.unwrap();
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
        let (manager, _bus, _store, _view_repo, workspace) = manager();
        manager.new_session("s1", &workspace).await.unwrap();
        manager.new_session("s2", &workspace).await.unwrap();

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
        let workspace = temp_workspace();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider {
            chunk_size: 1,
            delay: Duration::from_millis(10),
        });
        let store = temp_store();
        let view_repo = Arc::new(SessionViewRepository::new(Arc::clone(&store)));
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let manager =
            SessionManager::new(services_without_tools(provider, Arc::clone(&store), bus));
        manager.new_session("s1", &workspace).await.unwrap();
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

        let view = view_repo.load_session("s1").await.expect("session loads");
        assert_eq!(
            view.messages.len(),
            1,
            "only the developer message should be persisted; partial assistant messages are dropped on cancellation"
        );
        assert_eq!(view.messages[0].role, byte_protocol::MessageRole::Developer);
        assert!(
            view.messages[0].body.0.iter().any(|block| matches!(
                block,
                byte_protocol::MessageBlock::Text { text } if text == "hi"
            )),
            "developer message should be preserved"
        );
    }

    #[tokio::test]
    async fn cancel_run_without_runner_succeeds() {
        let (manager, bus, _store, _view_repo, workspace) = manager();
        manager.new_session("s1", &workspace).await.unwrap();
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
