use std::sync::Arc;

use byte_models::ModelProvider;
use byte_session::SessionStore;
use byte_skills::SkillRegistry;
use byte_tools::ToolRegistry;

use crate::event_bus::RuntimeEventBus;

/// Aggregated runtime dependencies used by [`crate::SessionManager`] and
/// [`crate::SessionRunner`].
///
/// The workspace root for a run is stored per-session in the session header
/// (`SessionView.workspace`) rather than here, so that each session resolves
/// tool paths relative to its own workspace.
#[derive(Clone)]
pub struct RuntimeServices {
    /// The model provider used to execute runs.
    pub provider: Arc<dyn ModelProvider>,
    /// Persistent session storage.
    pub store: Arc<SessionStore>,
    /// Bus used to publish runtime events to subscribers.
    pub event_bus: Arc<dyn RuntimeEventBus>,
    /// Registry of tools available during runs.
    pub tool_registry: Arc<dyn ToolRegistry>,
    /// Registry of skills that can be activated at runtime.
    pub skill_registry: Arc<dyn SkillRegistry>,
}

impl std::fmt::Debug for RuntimeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeServices")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl RuntimeServices {
    /// Create a new runtime services container.
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        event_bus: Arc<dyn RuntimeEventBus>,
        tool_registry: Arc<dyn ToolRegistry>,
        skill_registry: Arc<dyn SkillRegistry>,
    ) -> Self {
        Self {
            provider,
            store,
            event_bus,
            tool_registry,
            skill_registry,
        }
    }
}
