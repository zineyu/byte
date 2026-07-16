//! Pool for managing per-session [`SessionRunner`] lifetimes.
//!
//! A `RunnerPool` centralizes creation, reuse, and teardown of runners so that
//! [`SessionManager`] does not own a raw `HashMap` of runners. Each session's
//! active skill state is kept independently of any runner, allowing runners to
//! be evicted and recreated without losing session configuration.

use std::collections::HashMap;
use std::sync::Arc;

use byte_protocol::{ActivatedSkill, SessionEntry};
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::runner::SessionRunner;
use crate::runtime_services::RuntimeServices;

/// Result of closing a runner from the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseResult {
    /// The session had no cached runner.
    Absent,
    /// The runner was busy and remains cached.
    Busy,
    /// The runner was idle and was removed.
    Closed,
}

/// Type alias for the active skill list of a single session.
type ActiveSkills = Arc<Mutex<Vec<ActivatedSkill>>>;

/// Type alias for the map from session id to its active skill list.
type SessionSkills = HashMap<String, ActiveSkills>;

/// Manages the lifecycle of [`SessionRunner`] instances.
#[derive(Clone, Debug)]
pub struct RunnerPool {
    /// Shared runtime services used to construct runners.
    services: RuntimeServices,
    /// Map from session id to the currently cached runner.
    runners: Arc<Mutex<HashMap<String, Arc<SessionRunner>>>>,
    /// Per-session active skill state, independent of any cached runner.
    session_skills: Arc<Mutex<SessionSkills>>,
}

impl RunnerPool {
    /// Create a new runner pool with the given shared runtime services.
    #[must_use]
    pub fn new(services: RuntimeServices) -> Self {
        Self {
            services,
            runners: Arc::new(Mutex::new(HashMap::new())),
            session_skills: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create the runner for `session_id`.
    ///
    /// Concurrent calls for the same session will never create more than one
    /// runner.
    pub async fn get_or_create(&self, session_id: &str) -> Arc<SessionRunner> {
        {
            let runners = self.runners.lock().await;
            if let Some(runner) = runners.get(session_id).cloned() {
                return runner;
            }
        }

        let active_skills = self.active_skills_for(session_id).await;

        let mut runners = self.runners.lock().await;
        runners
            .entry(session_id.to_owned())
            .or_insert_with(|| {
                Arc::new(SessionRunner::with_active_skills(
                    self.services.clone(),
                    active_skills,
                ))
            })
            .clone()
    }

    /// Close the cached runner for `session_id` if it is idle.
    ///
    /// Returns [`CloseResult::Busy`] when the runner has an active run. In that
    /// case the runner is left in the pool.
    pub async fn close(&self, session_id: &str) -> CloseResult {
        let mut runners = self.runners.lock().await;
        let Some(runner) = runners.get(session_id) else {
            return CloseResult::Absent;
        };
        if runner.is_running().await {
            return CloseResult::Busy;
        }
        let _removed = runners.remove(session_id);
        drop(runners);

        let mut session_skills = self.session_skills.lock().await;
        let _ = session_skills.remove(session_id);
        info!(%session_id, "runner closed");
        CloseResult::Closed
    }

    /// Return or create the active-skills handle for `session_id`.
    async fn active_skills_for(&self, session_id: &str) -> ActiveSkills {
        {
            let session_skills = self.session_skills.lock().await;
            if let Some(skills) = session_skills.get(session_id).cloned() {
                return skills;
            }
        }

        let skills: ActiveSkills = Arc::new(Mutex::new(self.load_active_skills(session_id).await));

        let mut session_skills = self.session_skills.lock().await;
        session_skills
            .entry(session_id.to_owned())
            .or_insert(skills)
            .clone()
    }

    /// Load activated skills for `session_id` from the persistent session store.
    ///
    /// When a skill has been activated multiple times, the latest activation
    /// wins so that the recovered state matches the in-memory deduplication
    /// performed by [`ActivateSkillTool`](crate::activate_skill::ActivateSkillTool).
    async fn load_active_skills(&self, session_id: &str) -> Vec<ActivatedSkill> {
        let entries = match self.services.store.read_entries(session_id).await {
            Ok(entries) => entries,
            Err(error) => {
                debug!(%session_id, %error, "failed to read session entries for active skills; starting with empty skill set");
                return Vec::new();
            }
        };

        let mut skills: Vec<ActivatedSkill> = Vec::new();
        for entry in entries {
            if let SessionEntry::SkillActivated(skill) = entry {
                if let Some(existing) = skills.iter_mut().find(|s| s.name == skill.name) {
                    existing.content.clone_from(&skill.content);
                } else {
                    skills.push(skill);
                }
            }
        }
        skills
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;

    use byte_models::{EchoProvider, ModelProvider};
    use byte_protocol::SendMessageParams;
    use byte_session::SessionStore;
    use byte_skills::MvpSkillRegistry;
    use byte_tools::{
        AllowAllPolicy, MvpToolRegistry as ByteToolsMvpToolRegistry, ReadFileTool, ToolRegistry,
    };
    use tempfile::tempdir;

    use crate::compaction::CompactionConfig;
    use crate::event_bus::{RecordingEventBus, RuntimeEventBus};
    use crate::runtime_services::RuntimeServices;

    use super::{CloseResult, RunnerPool};

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    fn temp_workspace() -> String {
        let dir = std::env::temp_dir().join("byte-test-workspace");
        std::fs::create_dir_all(&dir).expect("create temp workspace");
        dir.to_str()
            .expect("workspace path is valid UTF-8")
            .to_owned()
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
            Arc::new(MvpSkillRegistry::new()),
            CompactionConfig::default(),
        )
    }

    #[tokio::test]
    async fn get_or_create_reuses_runner() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let pool = RunnerPool::new(services(
            Arc::new(EchoProvider::default()),
            Arc::clone(&store),
            bus,
        ));
        store.new_session("s1", &temp_workspace()).await.unwrap();

        let first = pool.get_or_create("s1").await;
        let second = pool.get_or_create("s1").await;
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn close_removes_idle_runner() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let pool = RunnerPool::new(services(
            Arc::new(EchoProvider::default()),
            Arc::clone(&store),
            bus,
        ));
        store.new_session("s1", &temp_workspace()).await.unwrap();

        let runner = pool.get_or_create("s1").await;
        runner.wait_until_idle().await;

        assert_eq!(pool.close("s1").await, CloseResult::Closed);
        let runner2 = pool.get_or_create("s1").await;
        assert!(!Arc::ptr_eq(&runner, &runner2));
    }

    #[tokio::test]
    async fn close_returns_busy_for_active_runner() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let provider = Arc::new(EchoProvider {
            delay: std::time::Duration::from_millis(50),
            ..Default::default()
        });
        let pool = RunnerPool::new(services(provider, Arc::clone(&store), bus));
        store.new_session("s1", &temp_workspace()).await.unwrap();

        let runner = pool.get_or_create("s1").await;
        runner
            .send_message(SendMessageParams {
                session_id: "s1".into(),
                message: "hi".into(),
            })
            .await
            .expect("run accepted");

        assert_eq!(pool.close("s1").await, CloseResult::Busy);

        runner.wait_until_idle().await;
    }

    #[tokio::test]
    async fn close_returns_absent_when_no_runner() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let pool = RunnerPool::new(services(
            Arc::new(EchoProvider::default()),
            Arc::clone(&store),
            bus,
        ));
        store.new_session("s1", &temp_workspace()).await.unwrap();

        assert_eq!(pool.close("s1").await, CloseResult::Absent);
    }

    #[tokio::test]
    async fn load_active_skills_recovers_latest_activation() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let pool = RunnerPool::new(services(
            Arc::new(EchoProvider::default()),
            Arc::clone(&store),
            bus,
        ));
        let workspace = temp_workspace();
        store.new_session("s1", &workspace).await.unwrap();
        store
            .append_skill_activation("s1", "review", "Review carefully.")
            .await
            .unwrap();

        let skills = pool.load_active_skills("s1").await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].content, "Review carefully.");
    }

    #[tokio::test]
    async fn load_active_skills_deduplicates_by_name() {
        let store = temp_store();
        let bus: Arc<dyn RuntimeEventBus> = Arc::new(RecordingEventBus::new());
        let pool = RunnerPool::new(services(
            Arc::new(EchoProvider::default()),
            Arc::clone(&store),
            bus,
        ));
        let workspace = temp_workspace();
        store.new_session("s1", &workspace).await.unwrap();
        store
            .append_skill_activation("s1", "review", "First version.")
            .await
            .unwrap();
        store
            .append_skill_activation("s1", "review", "Updated version.")
            .await
            .unwrap();
        store
            .append_skill_activation("s1", "test", "Test skill.")
            .await
            .unwrap();

        let skills = pool.load_active_skills("s1").await;
        assert_eq!(skills.len(), 2);
        let review = skills
            .iter()
            .find(|s| s.name == "review")
            .expect("review skill present");
        assert_eq!(review.content, "Updated version.");
        let test = skills
            .iter()
            .find(|s| s.name == "test")
            .expect("test skill present");
        assert_eq!(test.content, "Test skill.");
    }
}
