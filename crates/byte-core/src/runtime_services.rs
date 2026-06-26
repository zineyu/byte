use std::sync::Arc;

use byte_models::ModelProvider;
use byte_session::SessionStore;
use byte_tools::ToolRegistry;

use crate::event_bus::RuntimeEventBus;

/// Aggregated runtime dependencies used by [`SessionManager`] and
/// [`SessionRunner`].
///
/// The workspace root for a run is stored per-session in the session header
/// (`SessionView.workspace`) rather than here, so that each session resolves
/// tool paths relative to its own workspace.
#[derive(Clone)]
pub struct RuntimeServices {
    pub provider: Arc<dyn ModelProvider>,
    pub store: Arc<SessionStore>,
    pub event_bus: Arc<dyn RuntimeEventBus>,
    pub tool_registry: Arc<dyn ToolRegistry>,
}

impl RuntimeServices {
    /// Create a new runtime services container.
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        event_bus: Arc<dyn RuntimeEventBus>,
        tool_registry: Arc<dyn ToolRegistry>,
    ) -> Self {
        Self {
            provider,
            store,
            event_bus,
            tool_registry,
        }
    }
}
