use byte_protocol::{CompactionSummary, MessageRole, RunMessage, SessionMessage};

/// Definition of a tool available to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: &'static str,
}

/// A skill listed in the skill catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: &'static str,
    pub description: &'static str,
}

/// Context supplied to `PromptBuilder` for a single run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContext {
    pub user_message: String,
    pub history: Vec<SessionMessage>,
    pub compactions: Vec<CompactionSummary>,
}

/// Builds the prompt messages for a model run.
#[derive(Debug, Clone, Default)]
pub struct PromptBuilder;

impl PromptBuilder {
    /// Create a new prompt builder.
    pub fn new() -> Self {
        Self
    }

    /// Return the static list of MVP tool definitions.
    pub fn tools(&self) -> &'static [ToolDefinition] {
        &[
            ToolDefinition {
                name: "read_file",
                description: "Read the contents of a file at the given path.",
                parameters: r#"{"path": "string"}"#,
            },
            ToolDefinition {
                name: "write_file",
                description: "Write content to a file at the given path.",
                parameters: r#"{"path": "string", "content": "string"}"#,
            },
            ToolDefinition {
                name: "apply_patch",
                description: "Apply a line-based patch to a file.",
                parameters: r#"{"path": "string", "patch": "string"}"#,
            },
            ToolDefinition {
                name: "run_command",
                description: "Run a shell command in the workspace.",
                parameters: r#"{"command": "string", "cwd": "string?"}"#,
            },
            ToolDefinition {
                name: "list_directory",
                description: "List the contents of a directory.",
                parameters: r#"{"path": "string"}"#,
            },
            ToolDefinition {
                name: "grep",
                description: "Search file contents for a pattern.",
                parameters: r#"{"pattern": "string", "path": "string"}"#,
            },
            ToolDefinition {
                name: "find_files",
                description: "Find files matching a glob pattern.",
                parameters: r#"{"pattern": "string", "path": "string?"}"#,
            },
            ToolDefinition {
                name: "activate_skill",
                description: "Activate a skill from the skill catalog by name.",
                parameters: r#"{"name": "string"}"#,
            },
        ]
    }

    /// Return the static skill catalog placeholder.
    pub fn skills(&self) -> &'static [SkillEntry] {
        &[
            SkillEntry {
                name: "context-engineering",
                description: "Optimizes agent context setup.",
            },
            SkillEntry {
                name: "code-review-and-quality",
                description: "Conducts multi-axis code review.",
            },
        ]
    }
    /// Build the full list of `RunMessage`s for the provider.
    pub fn build(&self, context: PromptContext) -> Vec<RunMessage> {
        let mut messages = Vec::new();

        messages.push(RunMessage {
            role: MessageRole::System,
            content: self.build_system_prompt(),
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
            });
        }

        // Add persisted history.
        for message in &context.history {
            messages.push(RunMessage {
                role: message.role.clone(),
                content: message.content.clone(),
            });
        }

        // Add current user message.
        messages.push(RunMessage {
            role: MessageRole::Developer,
            content: context.user_message,
        });

        messages
    }

    fn build_system_prompt(&self) -> String {
        let mut prompt = String::new();

        prompt.push_str(
            "You are Byte Agent, a local coding assistant running on the user's machine.\n\n",
        );

        prompt.push_str("Available tools:\n");
        for tool in self.tools() {
            prompt.push_str(&format!("- {}: {}\n", tool.name, tool.description));
            prompt.push_str(&format!("  parameters: {}\n", tool.parameters));
        }

        prompt.push_str("\nAvailable skills:\n");
        for skill in self.skills() {
            prompt.push_str(&format!("- {}: {}\n", skill.name, skill.description));
        }

        prompt.push_str("\nUse the tools when needed. Output tool calls in plain text for now.");

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byte_protocol::SessionMessage;

    #[test]
    fn builder_includes_tools_and_skills() {
        let builder = PromptBuilder::new();
        let context = PromptContext {
            user_message: "hello".into(),
            history: vec![],
            compactions: vec![],
        };
        let messages = builder.build(context);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("read_file"));
        assert!(messages[0].content.contains("activate_skill"));
        assert!(messages[0].content.contains("context-engineering"));
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
            }],
            compactions: vec![CompactionSummary {
                id: "c1".into(),
                parent_id: "m1".into(),
                summary: "old topic".into(),
            }],
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
}
