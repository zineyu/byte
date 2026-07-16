use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use byte_models::{ModelProvider, ProviderError, ProviderEvent};
use byte_protocol::{
    CompactionEntry, CompactionRange, LlmMessage, Message, MessageRole, SessionEntry,
};
use byte_session::SessionStore;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

/// Errors that can occur during compaction.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    /// The context budget is exceeded but there is no prior history to compact.
    #[error("context budget exceeded with no prior history")]
    NoPriorHistory,
    /// A single message already exceeds the context budget.
    #[error("message exceeds context budget")]
    SingleMessageExceedsBudget,
    /// The provider call for summarization failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The compaction entry could not be persisted.
    #[error(transparent)]
    SessionStore(#[from] byte_session::SessionError),
    /// The summary did not reduce the active path below the budget.
    #[error("compaction did not reduce context below budget")]
    IneffectiveCompaction,
    /// Compaction was cancelled by the user while the summarization call was in progress.
    #[error("compaction was cancelled")]
    Cancelled,
}

/// Configuration for compaction decisions.
#[derive(Debug, Clone, Copy)]
pub struct CompactionConfig {
    /// Context budget in tokens.
    pub context_budget: usize,
    /// Threshold percentage (0–100) at which compaction triggers.
    pub threshold_percent: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            context_budget: 8192,
            threshold_percent: 90,
        }
    }
}

/// Service that decides when to compact and generates summarization entries.
#[derive(Clone)]
pub struct CompactionService {
    /// Model provider used to generate compaction summaries.
    provider: Arc<dyn ModelProvider>,
    /// Persistent session store for appending compaction entries.
    store: Arc<SessionStore>,
    /// Configuration controlling compaction decisions.
    config: CompactionConfig,
}

impl std::fmt::Debug for CompactionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactionService")
            .field("provider", &"Arc<dyn ModelProvider>")
            .field("store", &"Arc<SessionStore>")
            .field("config", &self.config)
            .finish()
    }
}

/// Returns a future that resolves when the optional cancellation token is
/// cancelled. When no token is provided, the future never resolves.
fn cancellation_future(
    cancel_token: Option<&CancellationToken>,
) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    match cancel_token {
        Some(token) => Box::pin(token.cancelled()),
        None => Box::pin(std::future::pending()),
    }
}

impl CompactionService {
    /// Create a new compaction service.
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        store: Arc<SessionStore>,
        config: CompactionConfig,
    ) -> Self {
        Self {
            provider,
            store,
            config,
        }
    }

    /// Build the active path from raw session entries.
    #[must_use]
    pub fn build_active_path(entries: &[SessionEntry]) -> Vec<LlmMessage> {
        crate::session::active_path::build_active_path(entries)
    }

    /// Estimate tokens of the active path.
    #[must_use]
    pub fn estimate_tokens(active_path: &[LlmMessage]) -> usize {
        crate::session::budget::estimate_tokens(active_path)
    }

    /// Return true if the active path is at or above the configured threshold.
    #[must_use]
    pub fn is_compaction_needed(&self, active_path: &[LlmMessage]) -> bool {
        crate::session::budget::is_above_threshold(
            Self::estimate_tokens(active_path),
            self.config.context_budget,
            self.config.threshold_percent,
        )
    }

    /// Select the oldest contiguous block of non-compacted messages from the
    /// full chronological message list.
    #[must_use]
    pub fn select_compaction_range(
        messages: &[Message],
        compacted_ids: &HashSet<String>,
    ) -> Option<CompactionRange> {
        // Skip any leading messages that are already compacted.
        let first_non_compacted = messages
            .iter()
            .position(|message| !compacted_ids.contains(&message.id))?;

        // Take the longest contiguous run starting at the first non-compacted message.
        let mut last = first_non_compacted;
        for (index, message) in messages.iter().enumerate().skip(first_non_compacted) {
            if compacted_ids.contains(&message.id) {
                break;
            }
            last = index;
        }

        Some(CompactionRange {
            first_message_id: messages[first_non_compacted].id.clone(),
            last_message_id: messages[last].id.clone(),
        })
    }

    /// Build a summarization prompt from the messages to be compacted.
    #[must_use]
    pub fn build_summary_prompt(messages: &[Message]) -> Vec<LlmMessage> {
        let mut prompt = vec![LlmMessage::text(
            MessageRole::System,
            "Summarize the following conversation block concisely. Preserve facts, decisions, and context needed for future turns.",
        )];

        for message in messages {
            prompt.push(LlmMessage {
                role: message.role,
                body: message.body.clone(),
                tool_call_id: message.tool_call_id.clone(),
            });
        }

        prompt
    }

    /// Summarize the given messages by calling the model provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider call fails, if the model emits tool calls
    /// instead of a plain summary, if the provider stream is malformed, or if the
    /// summarization is cancelled.
    pub async fn summarize(
        &self,
        messages: &[Message],
        cancel_token: Option<&CancellationToken>,
    ) -> Result<String, CompactionError> {
        let prompt = Self::build_summary_prompt(messages);
        let mut stream = self.provider.send_message(prompt, vec![]).await?;
        let mut summary = String::new();
        let mut current_message_id: Option<String> = None;

        loop {
            tokio::select! {
                biased;
                () = cancellation_future(cancel_token) => {
                    return Err(CompactionError::Cancelled);
                }
                maybe_event = stream.next() => {
                    match maybe_event {
                        Some(event) => {
                            let event = event?;
                            match event {
                                ProviderEvent::MessageStarted { message_id } => {
                                    current_message_id = Some(message_id);
                                }
                                ProviderEvent::TextDelta { message_id, delta } => {
                                    if current_message_id.as_ref() == Some(&message_id) {
                                        summary.push_str(&delta);
                                    }
                                }
                                ProviderEvent::MessageCompleted {
                                    message_id,
                                    tool_calls,
                                } => {
                                    if current_message_id.as_ref() == Some(&message_id) && tool_calls.is_some() {
                                        return Err(CompactionError::Provider(ProviderError::InvalidResponse(
                                            "compaction summary requested tool calls".into(),
                                        )));
                                    }
                                }
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(summary)
    }

    /// Collect the IDs of all messages already covered by compaction entries.
    #[must_use]
    pub fn collect_compacted_ids(entries: &[SessionEntry]) -> HashSet<String> {
        let mut message_ids: Vec<String> = Vec::new();
        for entry in entries {
            if let SessionEntry::Message(message) = entry {
                message_ids.push(message.id.clone());
            }
        }

        let positions: HashMap<String, usize> = message_ids
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index))
            .collect();

        let mut compacted: HashSet<String> = HashSet::new();
        for entry in entries {
            if let SessionEntry::CompactionEntry(compaction) = entry {
                let first = positions.get(&compaction.compacted_range.first_message_id);
                let last = positions.get(&compaction.compacted_range.last_message_id);
                let (first_index, last_index) = match (first, last) {
                    (Some(&a), Some(&b)) => (a.min(b), a.max(b)),
                    _ => continue,
                };
                for id in &message_ids[first_index..=last_index] {
                    let _ = compacted.insert(id.clone());
                }
            }
        }

        compacted
    }

    /// If compaction is needed, select the oldest block, summarize it, persist
    /// a `CompactionEntry`, and return it. If no compaction is needed, return None.
    ///
    /// The caller is responsible for emitting runtime events.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no compactable history, a single selected
    /// message already exceeds the budget, the provider fails, persistence
    /// fails, or the compaction does not reduce the active path below the
    /// configured threshold.
    pub async fn compact_if_needed(
        &self,
        run_id: &str,
        session_id: &str,
        entries: &[SessionEntry],
        cancel_token: Option<&CancellationToken>,
    ) -> Result<Option<CompactionEntry>, CompactionError> {
        let active_path = Self::build_active_path(entries);

        if !self.is_compaction_needed(&active_path) {
            return Ok(None);
        }

        let messages: Vec<Message> = entries
            .iter()
            .filter_map(|entry| match entry {
                SessionEntry::Message(message) => Some(message.clone()),
                _ => None,
            })
            .collect();

        let compacted_ids = Self::collect_compacted_ids(entries);
        let range = Self::select_compaction_range(&messages, &compacted_ids)
            .ok_or(CompactionError::NoPriorHistory)?;

        let first_index = messages
            .iter()
            .position(|message| message.id == range.first_message_id)
            .unwrap_or(0);
        let last_index = messages
            .iter()
            .position(|message| message.id == range.last_message_id)
            .unwrap_or(messages.len() - 1);
        let (first_index, last_index) = (first_index.min(last_index), first_index.max(last_index));
        let selected = &messages[first_index..=last_index];

        if selected.iter().any(|message| {
            crate::session::budget::estimate_tokens(std::slice::from_ref(&LlmMessage {
                role: message.role,
                body: message.body.clone(),
                tool_call_id: message.tool_call_id.clone(),
            })) > self.config.context_budget
        }) {
            return Err(CompactionError::SingleMessageExceedsBudget);
        }

        let summary = self.summarize(selected, cancel_token).await?;

        let compaction_entry = CompactionEntry {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Summary,
            summary,
            compacted_range: range,
            created_at: now_epoch_millis(),
            run_id: run_id.to_owned(),
        };

        let mut updated_entries = entries.to_vec();
        updated_entries.push(SessionEntry::CompactionEntry(compaction_entry.clone()));
        let _ = self
            .store
            .append_compaction_entry(session_id, compaction_entry.clone())
            .await?;

        let new_active_path = Self::build_active_path(&updated_entries);
        if self.is_compaction_needed(&new_active_path) {
            return Err(CompactionError::IneffectiveCompaction);
        }

        Ok(Some(compaction_entry))
    }
}

/// Returns the current time formatted as seconds.milliseconds UTC.
fn now_epoch_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use std::sync::Arc;

    use async_trait::async_trait;
    use byte_models::{ModelProvider, ProviderError, ProviderEvent, ProviderStream};
    use byte_protocol::{
        CompactionEntry, CompactionRange, LlmMessage, Message, MessageBlock, MessageBody,
        MessageRole, SessionEntry, ToolCall,
    };
    use byte_session::SessionStore;
    use futures::stream;
    use std::collections::HashSet;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    use super::{CompactionConfig, CompactionError, CompactionService};

    fn message(id: &str, role: MessageRole, text: &str) -> Message {
        Message {
            id: id.into(),
            parent_id: None,
            role,
            tool_call_id: None,
            body: MessageBody::text(text),
        }
    }

    fn session_message(id: &str, role: MessageRole, text: &str) -> SessionEntry {
        SessionEntry::Message(message(id, role, text))
    }

    fn compaction(first: &str, last: &str, summary: &str) -> SessionEntry {
        SessionEntry::CompactionEntry(CompactionEntry {
            id: "ce".into(),
            role: MessageRole::Summary,
            summary: summary.into(),
            compacted_range: CompactionRange {
                first_message_id: first.into(),
                last_message_id: last.into(),
            },
            created_at: "t".into(),
            run_id: "r1".into(),
        })
    }

    #[test]
    fn select_compaction_range_chooses_oldest_block() {
        let messages = vec![
            message("m1", MessageRole::Developer, "hello"),
            message("m2", MessageRole::Assistant, "hi"),
            message("m3", MessageRole::Developer, "next"),
        ];

        let range = CompactionService::select_compaction_range(&messages, &HashSet::new()).unwrap();
        assert_eq!(range.first_message_id, "m1");
        assert_eq!(range.last_message_id, "m3");
    }

    #[test]
    fn select_compaction_range_with_already_compacted_messages_skips_them() {
        let messages = vec![
            message("m1", MessageRole::Developer, "hello"),
            message("m2", MessageRole::Assistant, "hi"),
            message("m3", MessageRole::Developer, "next"),
        ];
        let mut compacted = HashSet::new();
        compacted.insert("m1".to_string());

        let range = CompactionService::select_compaction_range(&messages, &compacted).unwrap();
        assert_eq!(range.first_message_id, "m2");
        assert_eq!(range.last_message_id, "m3");
    }

    #[test]
    fn build_summary_prompt_includes_system_instruction_and_messages() {
        let messages = vec![
            message("m1", MessageRole::Developer, "hello"),
            message("m2", MessageRole::Assistant, "hi"),
        ];

        let prompt = CompactionService::build_summary_prompt(&messages);
        assert_eq!(prompt.len(), 3);
        assert_eq!(prompt[0].role, MessageRole::System);
        assert!(prompt[0].body.0.iter().any(
            |block| matches!(block, MessageBlock::Text { text } if text.contains("Summarize"))
        ));
        assert_eq!(prompt[1].role, MessageRole::Developer);
        assert_eq!(prompt[2].role, MessageRole::Assistant);
    }

    #[test]
    fn is_compaction_needed_true_when_above_threshold() {
        // A single long message of 40 characters exceeds a budget of 4 tokens
        // when the threshold is 90% because 40/4 = 10 tokens >= 4 * 0.9 = 3.
        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            temp_store(),
            CompactionConfig {
                context_budget: 4,
                threshold_percent: 90,
            },
        );
        let active_path = vec![LlmMessage::text(MessageRole::Developer, "a".repeat(40))];
        assert!(service.is_compaction_needed(&active_path));
    }

    #[test]
    fn is_compaction_needed_false_when_below_threshold() {
        // 8 characters / 4 = 2 tokens, which is below 90% of a budget of 10.
        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            temp_store(),
            CompactionConfig {
                context_budget: 10,
                threshold_percent: 90,
            },
        );
        let active_path = vec![LlmMessage::text(MessageRole::Developer, "a".repeat(8))];
        assert!(!service.is_compaction_needed(&active_path));
    }

    #[tokio::test]
    async fn compact_if_needed_persists_summary_and_returns_entry() {
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        let entries = vec![
            SessionEntry::Session {
                version: 1,
                id: "s1".into(),
                workspace: "/workspace".into(),
                created_at: "t".into(),
            },
            session_message("m1", MessageRole::Developer, "hello"),
            session_message("m2", MessageRole::Assistant, "hi there"),
        ];

        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            store.clone(),
            CompactionConfig {
                context_budget: 4,
                threshold_percent: 90,
            },
        );

        let result = service
            .compact_if_needed("run-1", "s1", &entries, None)
            .await
            .expect("compaction succeeds");

        let entry = result.expect("compaction was needed");
        assert_eq!(entry.role, MessageRole::Summary);
        assert_eq!(entry.summary, "summary");
        assert_eq!(entry.compacted_range.first_message_id, "m1");
        assert_eq!(entry.compacted_range.last_message_id, "m2");
        assert_eq!(entry.run_id, "run-1");

        let persisted = store.read_entries("s1").await.expect("read entries");
        assert!(persisted.iter().any(|e| matches!(e,
            SessionEntry::CompactionEntry(ce) if ce.id == entry.id
        )));
    }

    #[tokio::test]
    async fn compact_if_needed_returns_none_when_below_threshold() {
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        let entries = vec![
            SessionEntry::Session {
                version: 1,
                id: "s1".into(),
                workspace: "/workspace".into(),
                created_at: "t".into(),
            },
            session_message("m1", MessageRole::Developer, "hello"),
        ];

        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            store,
            CompactionConfig {
                context_budget: 100,
                threshold_percent: 90,
            },
        );

        let result = service
            .compact_if_needed("run-1", "s1", &entries, None)
            .await
            .expect("compaction check succeeds");

        assert!(result.is_none());
    }

    fn temp_store() -> Arc<SessionStore> {
        let dir = tempfile::tempdir().expect("temp dir");
        Arc::new(SessionStore::new(dir.path().to_path_buf()).expect("store creates"))
    }

    /// A provider that always returns a fixed summary text.
    struct FixedSummaryProvider;

    #[async_trait]
    impl ModelProvider for FixedSummaryProvider {
        async fn send_message(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let message_id = uuid::Uuid::new_v4().to_string();
            let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::TextDelta {
                    message_id: message_id.clone(),
                    delta: "summary".into(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: None,
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    /// A provider that never emits events, simulating a very slow summarization
    /// call that can be cancelled before producing any output.
    struct SlowSummaryProvider;

    #[async_trait]
    impl ModelProvider for SlowSummaryProvider {
        async fn send_message(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let stream = stream::once(async {
                tokio::time::sleep(Duration::from_mins(1)).await;
                Ok(ProviderEvent::MessageStarted {
                    message_id: "slow".into(),
                })
            });
            Ok(Box::pin(stream))
        }
    }

    #[tokio::test]
    async fn compact_if_needed_cancellation_stops_summarization() {
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        let entries = vec![
            SessionEntry::Session {
                version: 1,
                id: "s1".into(),
                workspace: "/workspace".into(),
                created_at: "t".into(),
            },
            session_message("m1", MessageRole::Developer, &"a".repeat(10)),
            session_message("m2", MessageRole::Assistant, &"b".repeat(10)),
        ];

        let service = CompactionService::new(
            Arc::new(SlowSummaryProvider),
            store.clone(),
            CompactionConfig {
                context_budget: 4,
                threshold_percent: 90,
            },
        );

        let token = CancellationToken::new();
        let token_for_task = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            token_for_task.cancel();
        });

        let result = service
            .compact_if_needed("run-1", "s1", &entries, Some(&token))
            .await;

        assert!(
            matches!(result, Err(CompactionError::Cancelled)),
            "compaction should be cancelled, got {result:?}"
        );

        let persisted = store.read_entries("s1").await.expect("read entries");
        assert!(
            !persisted
                .iter()
                .any(|e| matches!(e, SessionEntry::CompactionEntry(_))),
            "no compaction entry should be persisted"
        );
    }

    #[tokio::test]
    async fn single_message_exceeds_budget_returns_error() {
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        let huge_body = "a".repeat(1000);
        let entries = vec![
            SessionEntry::Session {
                version: 1,
                id: "s1".into(),
                workspace: "/workspace".into(),
                created_at: "t".into(),
            },
            session_message("m1", MessageRole::Developer, &huge_body),
            session_message("m2", MessageRole::Assistant, "short"),
        ];

        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            store.clone(),
            CompactionConfig {
                context_budget: 10,
                threshold_percent: 90,
            },
        );

        let result = service
            .compact_if_needed("run-1", "s1", &entries, None)
            .await;

        assert!(
            matches!(result, Err(CompactionError::SingleMessageExceedsBudget)),
            "should error with single message exceeds budget, got {result:?}"
        );

        let persisted = store.read_entries("s1").await.expect("read entries");
        assert!(
            !persisted
                .iter()
                .any(|e| matches!(e, SessionEntry::CompactionEntry(_))),
            "no compaction entry should be persisted"
        );
    }

    #[tokio::test]
    async fn empty_prior_history_returns_no_prior_history() {
        let store = temp_store();
        store
            .new_session("s1", "/workspace")
            .await
            .expect("create session");

        // All original messages are already compacted, but the resulting summary
        // is so large that the active path still exceeds the budget. With no
        // non-compacted history left to summarize, the service returns
        // NoPriorHistory.
        let entries = vec![
            SessionEntry::Session {
                version: 1,
                id: "s1".into(),
                workspace: "/workspace".into(),
                created_at: "t".into(),
            },
            SessionEntry::Message(message("m1", MessageRole::Developer, "hello")),
            SessionEntry::CompactionEntry(CompactionEntry {
                id: "ce-1".into(),
                role: MessageRole::Summary,
                summary: "a".repeat(1000),
                compacted_range: CompactionRange {
                    first_message_id: "m1".into(),
                    last_message_id: "m1".into(),
                },
                created_at: "t".into(),
                run_id: "r1".into(),
            }),
        ];

        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            store.clone(),
            CompactionConfig {
                context_budget: 10,
                threshold_percent: 90,
            },
        );

        let result = service
            .compact_if_needed("run-1", "s1", &entries, None)
            .await;

        assert!(
            matches!(result, Err(CompactionError::NoPriorHistory)),
            "should error with no prior history, got {result:?}"
        );
    }

    /// A provider that returns tool calls instead of text, which should fail.
    struct ToolCallSummaryProvider;

    #[async_trait]
    impl ModelProvider for ToolCallSummaryProvider {
        async fn send_message(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<byte_protocol::ToolDefinition>,
        ) -> Result<ProviderStream, ProviderError> {
            let message_id = uuid::Uuid::new_v4().to_string();
            let events: Vec<Result<ProviderEvent, ProviderError>> = vec![
                Ok(ProviderEvent::MessageStarted {
                    message_id: message_id.clone(),
                }),
                Ok(ProviderEvent::MessageCompleted {
                    message_id,
                    tool_calls: Some(vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "x"}),
                    }]),
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn summarize_errors_on_tool_calls() {
        let service = CompactionService::new(
            Arc::new(ToolCallSummaryProvider),
            temp_store(),
            CompactionConfig::default(),
        );

        let result = service
            .summarize(&[message("m1", MessageRole::Developer, "hello")], None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn summarize_collects_text_deltas() {
        let service = CompactionService::new(
            Arc::new(FixedSummaryProvider),
            temp_store(),
            CompactionConfig::default(),
        );

        let summary = service
            .summarize(&[message("m1", MessageRole::Developer, "hello")], None)
            .await
            .expect("summary succeeds");
        assert_eq!(summary, "summary");
    }

    #[test]
    fn collect_compacted_ids_marks_range_inclusive() {
        let entries = vec![
            session_message("m1", MessageRole::Developer, "a"),
            session_message("m2", MessageRole::Assistant, "b"),
            session_message("m3", MessageRole::Developer, "c"),
            compaction("m1", "m2", "ab"),
        ];

        let compacted = CompactionService::collect_compacted_ids(&entries);
        assert!(compacted.contains("m1"));
        assert!(compacted.contains("m2"));
        assert!(!compacted.contains("m3"));
    }

    #[test]
    fn collect_compacted_ids_ignores_missing_message_ids() {
        let entries = vec![
            session_message("m1", MessageRole::Developer, "a"),
            SessionEntry::CompactionEntry(CompactionEntry {
                id: "ce".into(),
                role: MessageRole::Summary,
                summary: "?".into(),
                compacted_range: CompactionRange {
                    first_message_id: "missing-first".into(),
                    last_message_id: "missing-last".into(),
                },
                created_at: "t".into(),
                run_id: "r1".into(),
            }),
        ];

        let compacted = CompactionService::collect_compacted_ids(&entries);
        assert!(compacted.is_empty());
    }
}
