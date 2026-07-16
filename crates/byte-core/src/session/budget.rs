//! Token estimation helpers for the active path.
//!
//! These estimators are intentionally simple: they use a deterministic
//! character-per-token heuristic so that tests can reason about them without
//! depending on a specific model tokenizer.

use byte_protocol::{LlmMessage, MessageBlock};

/// Approximate number of characters that represent one token for the MVP
/// heuristic.
pub const DEFAULT_CHARS_PER_TOKEN: usize = 4;

/// Estimate the number of tokens required to send `messages` to a model.
///
/// Uses a deterministic MVP heuristic of approximately four characters per
/// token. This is meant to be replaced by a model-specific tokenizer when
/// needed.
#[must_use]
pub fn estimate_tokens(messages: &[LlmMessage]) -> usize {
    messages.iter().map(estimate_message).sum()
}

/// Calculate the token threshold for a given context budget and percentage.
///
/// A return value of `n` means that `estimate_tokens` returning `n` or more
/// tokens should trigger the configured action (for example, compaction at the
/// 90% threshold).
#[must_use]
pub const fn budget_threshold(budget: usize, threshold_percent: u32) -> usize {
    if threshold_percent == 0 {
        return 0;
    }
    budget
        .saturating_mul(threshold_percent as usize)
        .saturating_div(100)
}

/// Return `true` if `tokens` is at or above `threshold_percent` of `budget`.
#[must_use]
pub const fn is_above_threshold(tokens: usize, budget: usize, threshold_percent: u32) -> bool {
    if budget == 0 {
        return false;
    }
    tokens >= budget_threshold(budget, threshold_percent)
}

/// Estimate the number of tokens in a single message using the MVP heuristic.
fn estimate_message(message: &LlmMessage) -> usize {
    let chars: usize = message
        .body
        .0
        .iter()
        .map(|block| match block {
            MessageBlock::Text { text } => text.len(),
            MessageBlock::ToolCall(tool_call) => {
                tool_call.name.len() + tool_call.arguments.to_string().len()
            }
        })
        .sum();
    chars.div_ceil(DEFAULT_CHARS_PER_TOKEN)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use byte_protocol::MessageRole;

    #[test]
    fn estimate_tokens_uses_chars_per_token() {
        let messages = vec![
            LlmMessage::text(MessageRole::Developer, "a".repeat(40)),
            LlmMessage::text(MessageRole::Assistant, "b".repeat(20)),
        ];

        let tokens = estimate_tokens(&messages);
        assert_eq!(tokens, 15); // 40/4 + 20/4 rounded up
    }

    #[test]
    fn budget_threshold_at_90_percent() {
        assert_eq!(budget_threshold(1000, 90), 900);
    }

    #[test]
    fn is_above_threshold_detects_budget_crossing() {
        assert!(is_above_threshold(900, 1000, 90));
        assert!(!is_above_threshold(899, 1000, 90));
    }
}
