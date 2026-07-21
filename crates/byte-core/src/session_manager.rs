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
    /// A message starting with `/skill:<name>` explicitly activates the named
    /// skill before the run starts: the activation is persisted to the session
    /// and injected into the model context through the message stream (see
    /// ADR 0021), while the original message text is sent unchanged. An
    /// unknown skill name aborts the request with an error and no run is
    /// started.
    ///
    /// # Errors
    ///
    /// Returns an error if the runner cannot be created, the run cannot be
    /// started, or a `/skill:` command names an unknown skill.
    #[instrument(skip(self, params))]
    pub async fn send_message(
        &self,
        params: byte_protocol::SendMessageParams,
    ) -> Result<crate::runner::RunId, RunnerError> {
        if let Some(skill_name) = parse_skill_command(&params.message) {
            self.activate_skill_for_session(&params.session_id, skill_name)
                .await?;
        }
        let runner = self.pool.get_or_create(&params.session_id).await;
        runner.send_message(params).await
    }

    /// Activate a skill for a session outside the model-driven
    /// `activate_skill` tool flow.
    ///
    /// The activation snapshot is persisted before the in-memory handle is
    /// updated, mirroring the write-through order of
    /// [`crate::activate_skill::ActivateSkillTool`].
    async fn activate_skill_for_session(
        &self,
        session_id: &str,
        skill_name: &str,
    ) -> Result<(), RunnerError> {
        let view = self.services.view_repo.load_session(session_id).await?;
        let definition = self
            .services
            .skill_registry
            .activate(Some(std::path::Path::new(&view.workspace)), skill_name)
            .await?;

        self.services
            .store
            .append_skill_activation(session_id, &definition.name, &definition.content)
            .await?;

        self.pool
            .record_skill_activation(
                session_id,
                byte_protocol::ActivatedSkill {
                    name: definition.name,
                    content: definition.content,
                },
            )
            .await;
        info!(%session_id, skill = skill_name, "skill activated via command");
        Ok(())
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

/// Parse a `/skill:<name>` command at the start of a message.
///
/// Returns the skill name when the message (after leading whitespace) begins
/// with `/skill:` followed by a non-empty name. The name runs until the next
/// whitespace character; any remaining text is left untouched and stays part
/// of the user message sent to the model.
fn parse_skill_command(message: &str) -> Option<&str> {
    let trimmed = message.trim_start();
    let rest = trimmed.strip_prefix("/skill:")?;
    let name = rest.split_whitespace().next()?;
    if name.is_empty() { None } else { Some(name) }
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

    #[test]
    fn parse_skill_command_matches_line_start_only() {
        assert_eq!(super::parse_skill_command("/skill:review"), Some("review"));
        assert_eq!(
            super::parse_skill_command("/skill:review check this code"),
            Some("review")
        );
        assert_eq!(
            super::parse_skill_command("  /skill:review\nmore text"),
            Some("review")
        );
        assert_eq!(super::parse_skill_command("/skill:"), None);
        assert_eq!(super::parse_skill_command("/skill"), None);
        assert_eq!(super::parse_skill_command("hello /skill:review"), None);
        assert_eq!(super::parse_skill_command("/other:review"), None);
    }

    /// Create a workspace directory containing one `review` skill plus a
    /// skill registry whose home directory is empty.
    fn skill_workspace() -> (tempfile::TempDir, Arc<dyn SkillRegistry>) {
        let workspace = tempdir().expect("temp workspace");
        let skill_dir = workspace.path().join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: Review code\n---\n# Review\n\nReview carefully.\n",
        )
        .expect("write skill file");

        let home = tempdir().expect("temp home");
        let registry: Arc<dyn SkillRegistry> =
            Arc::new(MvpSkillRegistry::with_home_dir(home.path()));
        (workspace, registry)
    }

    #[tokio::test]
    async fn send_message_with_skill_command_activates_skill_and_runs() {
        let (workspace, registry) = skill_workspace();
        let workspace_path = workspace.path().to_str().unwrap().to_owned();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let mut services = services_without_tools(provider, Arc::clone(&store), bus);
        services.skill_registry = registry;
        let manager = SessionManager::new(services);
        manager.new_session("s1", &workspace_path).await.unwrap();
        recording_bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "/skill:review please check src".into(),
        };
        manager
            .send_message(params)
            .await
            .expect("run accepted after skill activation");

        let runner = manager.pool.get_or_create("s1").await;
        runner.wait_until_idle().await;

        let entries = store.read_entries("s1").await.expect("read entries");
        let activations: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                byte_protocol::SessionEntry::SkillActivated(skill) => Some(skill),
                _ => None,
            })
            .collect();
        assert_eq!(activations.len(), 1, "activation must be persisted");
        assert_eq!(activations[0].name, "review");
        assert!(activations[0].content.contains("Review carefully."));

        // The run still executed: the original user message and an assistant
        // response are persisted.
        let messages: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                byte_protocol::SessionEntry::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.role == byte_protocol::MessageRole::Assistant),
            "run should produce an assistant message"
        );

        // The reconstructed LLM context contains the synthetic skill message.
        let active_path = crate::session::active_path::build_active_path(&entries);
        assert!(
            active_path.iter().any(|message| {
                message.body.0.iter().any(|block| matches!(
                    block,
                    byte_protocol::MessageBlock::Text { text } if text.contains("Review carefully.")
                ))
            }),
            "active path should inject the activated skill content"
        );
    }

    #[tokio::test]
    async fn send_message_with_unknown_skill_errors_without_starting_run() {
        let (workspace, registry) = skill_workspace();
        let workspace_path = workspace.path().to_str().unwrap().to_owned();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let mut services = services_without_tools(provider, Arc::clone(&store), bus);
        services.skill_registry = registry;
        let manager = SessionManager::new(services);
        manager.new_session("s1", &workspace_path).await.unwrap();
        recording_bus.take_events().await;

        let params = SendMessageParams {
            session_id: "s1".into(),
            message: "/skill:missing do something".into(),
        };
        let error = manager
            .send_message(params)
            .await
            .expect_err("unknown skill must fail");
        assert!(matches!(
            error,
            crate::runner::RunnerError::SkillRegistry(byte_skills::SkillError::NotFound(name)) if name == "missing"
        ));

        let events = recording_bus.take_events().await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(&event.kind, RuntimeEventKind::RunStarted { .. })),
            "no run should start when skill activation fails"
        );
        let entries = store.read_entries("s1").await.expect("read entries");
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, byte_protocol::SessionEntry::SkillActivated(_))),
            "failed activation must not be persisted"
        );
    }

    #[tokio::test]
    async fn repeated_skill_command_deduplicates_in_memory_activation() {
        let (workspace, registry) = skill_workspace();
        let workspace_path = workspace.path().to_str().unwrap().to_owned();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::default());
        let store = temp_store();
        let recording_bus = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = recording_bus.clone();
        let mut services = services_without_tools(provider, Arc::clone(&store), bus);
        services.skill_registry = registry;
        let manager = SessionManager::new(services);
        manager.new_session("s1", &workspace_path).await.unwrap();

        for text in ["/skill:review first", "/skill:review second"] {
            manager
                .send_message(SendMessageParams {
                    session_id: "s1".into(),
                    message: text.into(),
                })
                .await
                .expect("run accepted");
            let runner = manager.pool.get_or_create("s1").await;
            runner.wait_until_idle().await;
        }

        // Both activations are persisted as snapshots; the reconstructed
        // context contains the skill content exactly once.
        let entries = store.read_entries("s1").await.expect("read entries");
        let active_path = crate::session::active_path::build_active_path(&entries);
        let occurrences = active_path
            .iter()
            .filter(|message| {
                message.body.0.iter().any(|block| matches!(
                    block,
                    byte_protocol::MessageBlock::Text { text } if text.contains("Skill `review` has been activated")
                ))
            })
            .count();
        assert_eq!(occurrences, 1, "repeated activation must be deduplicated");
    }
}
