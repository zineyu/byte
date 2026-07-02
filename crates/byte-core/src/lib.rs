//! Core runtime crate for the Byte agent.
//!
//! `byte-core` ties together the conversation loop, session lifecycle, prompt
//! construction, skill activation, tool dispatch, and runtime event publishing.
//! It is intentionally agnostic to transport concerns such as WebSocket or stdio.
#![deny(rustdoc::broken_intra_doc_links)]

/// Tool and registry wrappers for activating agent skills at runtime.
pub mod activate_skill;

/// Event bus abstractions used to publish and observe runtime events.
pub mod event_bus;

/// LLM context construction from session state, tools, and active skills.
pub mod llm_context;

/// Session runner that drives the model/provider conversation loop.
pub mod runner;

/// Aggregated runtime dependencies shared across sessions and runs.
pub mod runtime_services;

/// Session lifecycle manager (create, load, delete, dispatch messages).
pub mod session_manager;

pub use runner::{RunId, RunnerError, SessionRunner};
pub use runtime_services::RuntimeServices;
pub use session_manager::SessionManager;

pub use event_bus::{BroadcastEventBus, RecordingEventBus, RuntimeEventBus};
