use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{ActivatedSkill, ToolCall, ToolDefinition};
use byte_session::SessionStore;
use byte_skills::SkillRegistry;
use byte_tools::{Tool, ToolError, ToolOutputStream, ToolPolicy, ToolRegistry, ToolStreamEvent};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// A tool that activates an agent skill by name for the current session.
///
/// The tool looks up the skill through the shared [`SkillRegistry`] and appends
/// the result to the per-session `active_skills` list maintained by the
/// [`SessionRunner`](crate::runner::SessionRunner).
pub struct ActivateSkillTool {
    /// Registry used to resolve and load skill definitions by name.
    skill_registry: Arc<dyn SkillRegistry>,
    /// Per-session list of skills that have been activated.
    active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
    /// Persistent session store used to record skill activations.
    store: Arc<SessionStore>,
}

impl std::fmt::Debug for ActivateSkillTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivateSkillTool").finish_non_exhaustive()
    }
}

impl ActivateSkillTool {
    /// Create a new activate-skill tool bound to the given registry, active
    /// skills list, and session store.
    #[must_use]
    pub fn new(
        skill_registry: Arc<dyn SkillRegistry>,
        active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
        store: Arc<SessionStore>,
    ) -> Self {
        Self {
            skill_registry,
            active_skills,
            store,
        }
    }
}

#[async_trait]
impl Tool for ActivateSkillTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "activate_skill".to_owned(),
            description:
                "Activate an agent skill by name and include its content in the session context."
                    .to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to activate"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &byte_protocol::SessionContext,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutputStream, ToolError> {
        let name = call
            .arguments
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `name` argument"))?;

        let definition = self
            .skill_registry
            .activate(Some(ctx.workspace_root.as_path()), name)
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;

        if let Some(session_id) = &ctx.session_id {
            self.store
                .append_skill_activation(session_id, &definition.name, &definition.content)
                .await
                .map_err(|error| ToolError::new(error.to_string()))?;
        }

        let mut active_skills = self.active_skills.lock().await;
        let already_active = active_skills
            .iter()
            .any(|skill| skill.name == definition.name);
        if let Some(existing) = active_skills
            .iter_mut()
            .find(|skill| skill.name == definition.name)
        {
            // Refresh the persisted snapshot of an already-active skill
            // instead of creating a duplicate entry.
            existing.content.clone_from(&definition.content);
        } else {
            active_skills.push(ActivatedSkill {
                name: definition.name.clone(),
                content: definition.content.clone(),
            });
        }
        drop(active_skills);

        // First activation returns the full structured skill definition so
        // the model receives the instructions through this tool result.
        // Repeated activation only confirms the state; the content is
        // already present in the conversation (see ADR 0021).
        let output = if already_active {
            format!(
                "Skill `{}` is already active; its instructions are provided in the conversation.",
                definition.name
            )
        } else {
            serde_json::json!({
                "name": definition.name,
                "description": definition.description,
                "content": definition.content,
            })
            .to_string()
        };
        Ok(byte_tools::single_event_stream(Ok(ToolStreamEvent::done(
            output,
        ))))
    }
}

/// A tool registry wrapper that adds the per-session `activate_skill` tool on
/// top of a base registry.
///
/// The base registry is shared across sessions and contains the concrete file
/// system and command tools. This wrapper lets each
/// [`SessionRunner`](crate::runner::SessionRunner) inject its own session-scoped
/// `activate_skill` implementation without mutating the shared base registry.
pub struct SessionToolRegistry {
    /// Underlying registry shared across sessions.
    base: Arc<dyn ToolRegistry>,
    /// Session-scoped `activate_skill` tool instance.
    activate_skill: Arc<dyn Tool>,
    /// Policy applied to the `activate_skill` tool.
    activate_policy: Arc<dyn ToolPolicy>,
}

impl std::fmt::Debug for SessionToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionToolRegistry")
            .field("base", &self.base.names())
            .field("activate_skill", &"activate_skill")
            .finish_non_exhaustive()
    }
}

impl SessionToolRegistry {
    /// Create a wrapper around `base` that also exposes `activate_skill`.
    #[must_use]
    pub fn new(
        base: Arc<dyn ToolRegistry>,
        activate_skill: Arc<dyn Tool>,
        activate_policy: Arc<dyn ToolPolicy>,
    ) -> Self {
        Self {
            base,
            activate_skill,
            activate_policy,
        }
    }
}

#[async_trait]
impl ToolRegistry for SessionToolRegistry {
    /// Dynamic registration after construction is intentionally a no-op.
    ///
    /// The session wrapper is built around an already-populated base registry
    /// that is shared across sessions. Allowing dynamic registration here would
    /// either mutate shared state for all sessions or require per-session
    /// cloning; neither is needed for the MVP.
    fn register(&mut self, _name: String, _tool: Arc<dyn Tool>, _policy: Arc<dyn ToolPolicy>) {
        // Intentionally no-op: the base registry is populated at construction
        // and the wrapper only adds the session-scoped `activate_skill` tool.
    }

    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.base.definitions();
        definitions.push(self.activate_skill.definition());
        definitions
    }

    fn names(&self) -> Vec<String> {
        let mut names = self.base.names();
        names.push("activate_skill".to_owned());
        names
    }

    fn get(&self, name: &str) -> Option<(Arc<dyn Tool>, Arc<dyn ToolPolicy>)> {
        if name == "activate_skill" {
            Some((
                Arc::clone(&self.activate_skill),
                Arc::clone(&self.activate_policy),
            ))
        } else {
            self.base.get(name)
        }
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &byte_protocol::SessionContext,
        cancel: &CancellationToken,
    ) -> Result<ToolOutputStream, ToolError> {
        if call.name == "activate_skill" {
            self.activate_policy.check(call, ctx)?;
            self.activate_skill.invoke(call, ctx, cancel).await
        } else {
            self.base.invoke(call, ctx, cancel).await
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use byte_session::SessionStore;

    use super::*;
    use async_trait::async_trait;
    use byte_protocol::{SessionContext, SkillDefinition, SkillEntry};
    use byte_tools::{AllowAllPolicy, MvpToolRegistry};
    use futures::StreamExt;

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempfile::tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    struct StubSkillRegistry {
        skills: Mutex<HashMap<String, SkillDefinition>>,
    }

    #[async_trait]
    impl SkillRegistry for StubSkillRegistry {
        async fn catalog(
            &self,
            _workspace: Option<&Path>,
        ) -> Result<Vec<SkillEntry>, byte_skills::SkillError> {
            let skills = self.skills.lock().await;
            Ok(skills
                .values()
                .map(|definition| SkillEntry {
                    name: definition.name.clone(),
                    description: definition.description.clone(),
                })
                .collect())
        }

        async fn activate(
            &self,
            _workspace: Option<&Path>,
            name: &str,
        ) -> Result<SkillDefinition, byte_skills::SkillError> {
            let skills = self.skills.lock().await;
            skills
                .get(name)
                .cloned()
                .ok_or_else(|| byte_skills::SkillError::NotFound(name.to_owned()))
        }
    }

    async fn collect_one(mut stream: ToolOutputStream) -> ToolStreamEvent {
        stream
            .next()
            .await
            .expect("stream should have an event")
            .expect("event should be Ok")
    }

    #[tokio::test]
    async fn activate_skill_appends_to_active_skills_and_returns_content() {
        let active_skills: Arc<Mutex<Vec<ActivatedSkill>>> = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(StubSkillRegistry {
            skills: Mutex::new(HashMap::from([(
                "review".to_owned(),
                SkillDefinition {
                    name: "review".to_owned(),
                    description: "Review skill".to_owned(),
                    content: "Review carefully.".to_owned(),
                },
            )])),
        });

        let tool = ActivateSkillTool::new(registry, Arc::clone(&active_skills), temp_store());
        let call = ToolCall {
            id: "call-1".into(),
            name: "activate_skill".into(),
            arguments: serde_json::json!({"name": "review"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: PathBuf::from("/workspace"),
        };

        let stream = tool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        let event = collect_one(stream).await;
        let ToolStreamEvent::Done { result } = event else {
            panic!("expected done event");
        };
        assert!(!result.is_error);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["name"], "review");
        assert_eq!(output["description"], "Review skill");
        assert_eq!(output["content"], "Review carefully.");

        let skills = active_skills.lock().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].content, "Review carefully.");
    }

    #[test]
    fn session_tool_registry_includes_activate_skill_definition() {
        let base = Arc::new(MvpToolRegistry::new());
        let wrapper = SessionToolRegistry::new(
            base,
            Arc::new(ActivateSkillTool::new(
                Arc::new(StubSkillRegistry {
                    skills: Mutex::new(HashMap::new()),
                }),
                Arc::new(Mutex::new(Vec::new())),
                temp_store(),
            )),
            Arc::new(AllowAllPolicy),
        );

        let definitions = wrapper.definitions();
        assert!(
            definitions
                .iter()
                .any(|definition| definition.name == "activate_skill")
        );
        assert!(wrapper.names().contains(&"activate_skill".to_owned()));
    }

    #[tokio::test]
    async fn activate_skill_deduplicates_and_updates_existing_skill() {
        let active_skills: Arc<Mutex<Vec<ActivatedSkill>>> = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(StubSkillRegistry {
            skills: Mutex::new(HashMap::from([(
                "review".to_owned(),
                SkillDefinition {
                    name: "review".to_owned(),
                    description: "Review skill".to_owned(),
                    content: "First version.".to_owned(),
                },
            )])),
        });

        let tool =
            ActivateSkillTool::new(registry.clone(), Arc::clone(&active_skills), temp_store());
        let ctx = SessionContext {
            session_id: None,
            workspace_root: PathBuf::from("/workspace"),
        };

        let first_call = ToolCall {
            id: "call-1".into(),
            name: "activate_skill".into(),
            arguments: serde_json::json!({"name": "review"}),
        };
        let first_stream = tool
            .invoke(&first_call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        let first_event = collect_one(first_stream).await;
        let ToolStreamEvent::Done {
            result: first_result,
        } = first_event
        else {
            panic!("expected done event");
        };
        let first_output: serde_json::Value = serde_json::from_str(&first_result.output).unwrap();
        assert_eq!(first_output["content"], "First version.");

        // Simulate a refreshed skill definition from the registry.
        {
            let mut skills = registry.skills.lock().await;
            skills.get_mut("review").unwrap().content = "Updated version.".to_owned();
        }

        let second_call = ToolCall {
            id: "call-2".into(),
            name: "activate_skill".into(),
            arguments: serde_json::json!({"name": "review"}),
        };
        let second_stream = tool
            .invoke(&second_call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        let second_event = collect_one(second_stream).await;
        // Repeated activation returns a short confirmation instead of the
        // full content, which is already present in the conversation.
        assert!(
            matches!(second_event, ToolStreamEvent::Done { result } if result.output.contains("already active") && !result.is_error)
        );

        let skills = active_skills.lock().await;
        assert_eq!(skills.len(), 1, "same skill should not be added twice");
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].content, "Updated version.");
    }
}
