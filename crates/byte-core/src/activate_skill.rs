use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{ActivatedSkill, ToolCall, ToolDefinition};
use byte_skills::SkillRegistry;
use byte_tools::{Tool, ToolError, ToolPolicy, ToolRegistry};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// A tool that activates an agent skill by name for the current session.
///
/// The tool looks up the skill through the shared [`SkillRegistry`] and appends
/// the result to the per-session `active_skills` list maintained by the
/// [`SessionRunner`](crate::runner::SessionRunner).
pub struct ActivateSkillTool {
    skill_registry: Arc<dyn SkillRegistry>,
    active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
}

impl ActivateSkillTool {
    /// Create a new activate-skill tool bound to the given registry and active
    /// skills list.
    pub fn new(
        skill_registry: Arc<dyn SkillRegistry>,
        active_skills: Arc<Mutex<Vec<ActivatedSkill>>>,
    ) -> Self {
        Self {
            skill_registry,
            active_skills,
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
    ) -> Result<String, ToolError> {
        let name = call
            .arguments
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("missing `name` argument"))?;

        let definition = self
            .skill_registry
            .activate(ctx.workspace_root.as_deref(), name)
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;

        let mut active_skills = self.active_skills.lock().await;
        if let Some(existing) = active_skills
            .iter_mut()
            .find(|skill| skill.name == definition.name)
        {
            // Refresh the content of an already-active skill instead of
            // creating a duplicate entry in the system prompt.
            existing.content.clone_from(&definition.content);
        } else {
            active_skills.push(ActivatedSkill {
                name: definition.name.clone(),
                content: definition.content.clone(),
            });
        }
        drop(active_skills);

        Ok(definition.content)
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
    base: Arc<dyn ToolRegistry>,
    activate_skill: Arc<dyn Tool>,
    activate_policy: Arc<dyn ToolPolicy>,
}

impl SessionToolRegistry {
    /// Create a wrapper around `base` that also exposes `activate_skill`.
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
    ) -> Result<String, ToolError> {
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
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;
    use async_trait::async_trait;
    use byte_protocol::{SessionContext, SkillDefinition, SkillEntry};
    use byte_tools::{AllowAllPolicy, MvpToolRegistry};

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

        let tool = ActivateSkillTool::new(registry, Arc::clone(&active_skills));
        let call = ToolCall {
            id: "call-1".into(),
            name: "activate_skill".into(),
            arguments: serde_json::json!({"name": "review"}),
        };
        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };

        let result = tool
            .invoke(&call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, "Review carefully.");

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

        let tool = ActivateSkillTool::new(registry.clone(), Arc::clone(&active_skills));
        let ctx = SessionContext {
            session_id: None,
            workspace_root: None,
        };

        let first_call = ToolCall {
            id: "call-1".into(),
            name: "activate_skill".into(),
            arguments: serde_json::json!({"name": "review"}),
        };
        let first_result = tool
            .invoke(&first_call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(first_result, "First version.");

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
        let second_result = tool
            .invoke(&second_call, &ctx, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(second_result, "Updated version.");

        let skills = active_skills.lock().await;
        assert_eq!(skills.len(), 1, "same skill should not be added twice");
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].content, "Updated version.");
    }
}
