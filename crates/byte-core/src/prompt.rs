use byte_protocol::{CompactionSummary, MessageRole, RunMessage, SessionMessage, ToolDefinition};
use std::fmt::Write;

/// Context supplied to `PromptBuilder` for a single run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContext {
    pub user_message: String,
    pub history: Vec<SessionMessage>,
    pub compactions: Vec<CompactionSummary>,
    pub tools: Vec<ToolDefinition>,
}

impl PromptContext {
    /// Create a prompt context with no history, compactions, or tools.
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            history: Vec::new(),
            compactions: Vec::new(),
            tools: Vec::new(),
        }
    }
}

/// Builds the prompt messages for a model run.
#[derive(Debug, Clone, Default)]
pub struct PromptBuilder;

impl PromptBuilder {
    /// Create a new prompt builder.
    pub fn new() -> Self {
        Self
    }

    /// Build the full list of `RunMessage`s for the provider.
    pub fn build(&self, context: PromptContext) -> Vec<RunMessage> {
        let mut messages = Vec::new();

        messages.push(RunMessage {
            role: MessageRole::System,
            content: Self::build_system_prompt(&context.tools),
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
                role: message.role.clone(),
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

    fn build_system_prompt(tools: &[ToolDefinition]) -> String {
        let mut prompt = String::new();

        prompt.push_str(
            "You are Byte Agent, a local coding assistant running on the user's machine.\n\n",
        );

        prompt.push_str("Available tools:\n");
        for tool in tools {
            let _ = writeln!(prompt, "- {}: {}", tool.name, tool.description);
            let _ = writeln!(prompt, "  parameters: {}", tool.parameters);
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
