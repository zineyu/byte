use byte_protocol::{
    ActivatedSkill, CompactionSummary, MessageRole, RunMessage, SessionMessage, SkillEntry,
    ToolDefinition,
};
use std::fmt::Write;

/// Context supplied to `PromptBuilder` for a single run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContext {
    /// The current user message for this run.
    pub user_message: String,
    /// Prior messages in the session, in chronological order.
    pub history: Vec<SessionMessage>,
    /// Summaries of compacted conversation ranges.
    pub compactions: Vec<CompactionSummary>,
    /// Tool definitions available to the model for this run.
    pub tools: Vec<ToolDefinition>,
    /// Skills that have been activated for the current session.
    pub active_skills: Vec<ActivatedSkill>,
    /// Skills that are installed and can be activated by name.
    pub available_skills: Vec<SkillEntry>,
}

impl PromptContext {
    /// Create a prompt context with no history, compactions, tools, skills, or active skills.
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            history: Vec::new(),
            compactions: Vec::new(),
            tools: Vec::new(),
            active_skills: Vec::new(),
            available_skills: Vec::new(),
        }
    }
}

/// Builds the prompt messages for a model run.
#[derive(Debug, Clone, Copy, Default)]
pub struct PromptBuilder;

impl PromptBuilder {
    /// Create a new prompt builder.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Build the full list of `RunMessage`s for the provider.
    #[must_use]
    pub fn build(&self, context: PromptContext) -> Vec<RunMessage> {
        let mut messages = Vec::new();

        messages.push(RunMessage {
            role: MessageRole::System,
            content: Self::build_system_prompt(
                &context.tools,
                &context.active_skills,
                &context.available_skills,
            ),
            tool_call_id: None,
            tool_calls: None,
        });

        // Add compaction summaries as system reminders so they remain visible
        // without polluting the persisted message history.
        for compaction in &context.compactions {
            messages.push(RunMessage {
                role: MessageRole::System,
                content: format!(
                    "Earlier conversation summary ({}): {}",
                    compaction.id, compaction.summary
                ),
                tool_call_id: None,
                tool_calls: None,
            });
        }

        // Add persisted history.
        for message in &context.history {
            messages.push(RunMessage {
                role: message.role,
                content: message.content.clone(),
                tool_call_id: message.tool_call_id.clone(),
                tool_calls: message.tool_calls.clone(),
            });
        }

        // Add current user message.
        messages.push(RunMessage {
            role: MessageRole::Developer,
            content: context.user_message,
            tool_call_id: None,
            tool_calls: None,
        });

        messages
    }

    /// Build the system prompt from tool definitions and active/available skills.
    pub(crate) fn build_system_prompt(
        tools: &[ToolDefinition],
        active_skills: &[ActivatedSkill],
        available_skills: &[SkillEntry],
    ) -> String {
        let mut prompt = String::new();

        prompt.push_str(
            "You are Byte Agent, a local coding assistant running on the user's machine.\n\n",
        );

        if !active_skills.is_empty() {
            prompt.push_str("Active skills:\n");
            for skill in active_skills {
                let _ = writeln!(prompt, "## {}", skill.name);
                prompt.push_str(&skill.content);
                prompt.push('\n');
            }
            prompt.push('\n');
        }

        prompt.push_str("Available tools:\n");
        for tool in tools {
            let _ = writeln!(prompt, "- {}: {}", tool.name, tool.description);
            let _ = writeln!(prompt, "  parameters: {}", tool.parameters);
        }

        let active_names: std::collections::HashSet<_> =
            active_skills.iter().map(|s| &s.name).collect();
        let inactive_skills: Vec<_> = available_skills
            .iter()
            .filter(|skill| !active_names.contains(&skill.name))
            .collect();

        if !inactive_skills.is_empty() {
            prompt.push_str("\nAvailable skills (activate with the activate_skill tool):\n");
            for skill in inactive_skills {
                let _ = writeln!(prompt, "- {}: {}", skill.name, skill.description);
            }
        }

        prompt.push_str("\nUse the tools when needed.");

        prompt
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use byte_protocol::SessionMessage;

    #[test]
    fn builder_includes_registered_tools() {
        let builder = PromptBuilder::new();
        let context = PromptContext {
            user_message: "hello".into(),
            history: vec![],
            compactions: vec![],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file.".into(),
                parameters: serde_json::json!({"path": "string"}),
            }],
            active_skills: vec![],
            available_skills: vec![],
        };
        let messages = builder.build(context);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("read_file"));
        assert_eq!(messages[1].role, MessageRole::Developer);
        assert_eq!(messages[1].content, "hello");
    }

    #[test]
    fn builder_appends_history_and_compactions() {
        let builder = PromptBuilder::new();
        let context = PromptContext {
            user_message: "current".into(),
            history: vec![SessionMessage {
                id: "m1".into(),
                parent_id: None,
                role: MessageRole::Developer,
                content: "past".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            compactions: vec![CompactionSummary {
                id: "c1".into(),
                parent_id: "m1".into(),
                summary: "old topic".into(),
            }],
            tools: vec![],
            active_skills: vec![],
            available_skills: vec![],
        };
        let messages = builder.build(context);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("old topic"));
        assert_eq!(messages[2].role, MessageRole::Developer);
        assert_eq!(messages[2].content, "past");
        assert_eq!(messages[3].role, MessageRole::Developer);
        assert_eq!(messages[3].content, "current");
    }

    #[test]
    fn builder_includes_available_skills_for_activation() {
        let builder = PromptBuilder::new();
        let context = PromptContext {
            user_message: "hello".into(),
            history: vec![],
            compactions: vec![],
            tools: vec![],
            active_skills: vec![],
            available_skills: vec![
                SkillEntry {
                    name: "rust".into(),
                    description: "Rust best practices.".into(),
                },
                SkillEntry {
                    name: "testing".into(),
                    description: "Testing guidelines.".into(),
                },
            ],
        };
        let messages = builder.build(context);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        let system = &messages[0].content;
        assert!(system.contains("rust: Rust best practices."));
        assert!(system.contains("testing: Testing guidelines."));
        assert!(system.contains("activate_skill"));
    }

    #[test]
    fn builder_lists_only_inactive_available_skills() {
        let builder = PromptBuilder::new();
        let context = PromptContext {
            user_message: "hello".into(),
            history: vec![],
            compactions: vec![],
            tools: vec![],
            active_skills: vec![ActivatedSkill {
                name: "rust".into(),
                content: "Rust content.".into(),
            }],
            available_skills: vec![
                SkillEntry {
                    name: "rust".into(),
                    description: "Rust best practices.".into(),
                },
                SkillEntry {
                    name: "testing".into(),
                    description: "Testing guidelines.".into(),
                },
            ],
        };
        let messages = builder.build(context);

        assert_eq!(messages.len(), 2);
        let system = &messages[0].content;
        assert!(system.contains("Active skills:"));
        assert!(system.contains("## rust"));
        assert!(system.contains("testing: Testing guidelines."));
        assert!(!system.contains("rust: Rust best practices."));
    }
}
