//! Application state for lazysluice TUI.

use sluice::client::{MessageDelivery, Topic};

/// Current screen/mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    TopicList,
    Tail,
    Publish,
    Help,
}

/// Connection status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Application state (session-local, in-memory only).
#[derive(Debug, Default)]
pub struct AppState {
    pub screen: Screen,
    pub conn_status: ConnStatus,

    // Topic list
    pub topics: Vec<Topic>,
    pub topic_cursor: usize,

    // Tail view
    pub messages: Vec<MessageDelivery>,
    pub message_cursor: usize,
    pub paused: bool,

    // Publish draft
    pub publish_topic: String,
    pub publish_payload: String,
    pub publish_status: Option<String>,

    // Help overlay toggle (only applies when screen != Help)
    pub show_help: bool,

    // Acked message IDs (in-session only)
    pub acked_ids: std::collections::HashSet<String>,

    // Config from CLI (used in credit management - T020/T044)
    #[allow(dead_code)]
    pub credits_window: u32,
}

impl AppState {
    pub fn new(credits_window: u32) -> Self {
        Self {
            credits_window,
            ..Default::default()
        }
    }

    pub fn selected_topic(&self) -> Option<&Topic> {
        self.topics.get(self.topic_cursor)
    }

    pub fn selected_message(&self) -> Option<&MessageDelivery> {
        self.messages.get(self.message_cursor)
    }

    /// Validate publish input: both topic and payload must be non-empty.
    pub fn can_publish(&self) -> bool {
        !self.publish_topic.trim().is_empty() && !self.publish_payload.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_validation_requires_non_empty_topic_and_payload() {
        let mut state = AppState::new(128);

        // Both empty
        assert!(!state.can_publish());

        // Only topic set
        state.publish_topic = "test".to_string();
        assert!(!state.can_publish());

        // Only payload set
        state.publish_topic.clear();
        state.publish_payload = "hello".to_string();
        assert!(!state.can_publish());

        // Both set
        state.publish_topic = "test".to_string();
        assert!(state.can_publish());

        // Whitespace-only should not count
        state.publish_topic = "   ".to_string();
        assert!(!state.can_publish());
    }

    #[test]
    fn ack_state_tracking() {
        let mut state = AppState::new(128);

        // Initially no acks
        assert!(state.acked_ids.is_empty());

        // Add acked IDs
        state.acked_ids.insert("msg-001".to_string());
        assert!(state.acked_ids.contains("msg-001"));
        assert!(!state.acked_ids.contains("msg-002"));

        // Duplicate insertion is idempotent
        state.acked_ids.insert("msg-001".to_string());
        assert_eq!(state.acked_ids.len(), 1);

        // Multiple IDs can be tracked
        state.acked_ids.insert("msg-002".to_string());
        assert_eq!(state.acked_ids.len(), 2);
    }
}
