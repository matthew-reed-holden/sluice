//! Application state for lazysluice TUI.

use sluice_client::{InitialPosition, MessageDelivery, Topic};

/// Current screen/mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    TopicList,
    Tail,
    Publish,
    Help,
    CreateTopic,
    MessageDetail,
}

/// Active field in publish screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PublishInputField {
    #[default]
    Topic,
    Payload,
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
    pub visited_topics: std::collections::HashSet<String>,  // Track visited topics

    // Tail view
    pub messages: Vec<MessageDelivery>,
    pub message_cursor: usize,
    pub paused: bool,
    pub initial_position: InitialPosition,
    pub current_topic: Option<String>,  // Track which topic we're subscribed to

    // Publish draft
    pub publish_topic: String,
    pub publish_payload: String,
    pub publish_status: Option<String>,
    pub publish_active_field: PublishInputField,

    // Create topic dialog
    pub create_topic_name: String,
    pub create_topic_status: Option<String>,

    // Message detail view
    pub detail_message: Option<MessageDelivery>,

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
            initial_position: InitialPosition::Earliest,
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

    /// Toggle between Earliest and Latest subscription positions.
    pub fn toggle_initial_position(&mut self) {
        self.initial_position = match self.initial_position {
            InitialPosition::Earliest => InitialPosition::Latest,
            InitialPosition::Latest => InitialPosition::Earliest,
        };
    }

    /// Cycle between Topic and Payload fields in publish screen.
    pub fn cycle_publish_field(&mut self) {
        self.publish_active_field = match self.publish_active_field {
            PublishInputField::Topic => PublishInputField::Payload,
            PublishInputField::Payload => PublishInputField::Topic,
        };
    }

    /// Check if a message with the given ID already exists.
    pub fn has_message(&self, message_id: &str) -> bool {
        self.messages.iter().any(|m| m.message_id == message_id)
    }

    /// Add a message only if it doesn't already exist (prevent duplicates).
    pub fn add_message_if_new(&mut self, msg: MessageDelivery) -> bool {
        if !self.has_message(&msg.message_id) {
            self.messages.push(msg);
            true
        } else {
            false
        }
    }

    /// Validate topic name: must be non-empty and alphanumeric with dashes/underscores.
    pub fn is_valid_topic_name(name: &str) -> bool {
        if name.trim().is_empty() {
            return false;
        }
        // Allow alphanumeric, dash, underscore, and dot
        name.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    }

    /// Check if topic creation is valid.
    pub fn can_create_topic(&self) -> bool {
        Self::is_valid_topic_name(&self.create_topic_name)
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

    #[test]
    fn initial_position_defaults_to_earliest() {
        let state = AppState::new(128);
        assert_eq!(state.initial_position, InitialPosition::Earliest);
    }

    #[test]
    fn can_toggle_initial_position() {
        let mut state = AppState::new(128);

        // Start at Earliest
        assert_eq!(state.initial_position, InitialPosition::Earliest);

        // Toggle to Latest
        state.toggle_initial_position();
        assert_eq!(state.initial_position, InitialPosition::Latest);

        // Toggle back to Earliest
        state.toggle_initial_position();
        assert_eq!(state.initial_position, InitialPosition::Earliest);
    }

    #[test]
    fn publish_active_field_defaults_to_topic() {
        let state = AppState::new(128);
        assert_eq!(state.publish_active_field, PublishInputField::Topic);
    }

    #[test]
    fn can_cycle_publish_fields() {
        let mut state = AppState::new(128);

        // Start at Topic
        assert_eq!(state.publish_active_field, PublishInputField::Topic);

        // Tab to Payload
        state.cycle_publish_field();
        assert_eq!(state.publish_active_field, PublishInputField::Payload);

        // Tab back to Topic
        state.cycle_publish_field();
        assert_eq!(state.publish_active_field, PublishInputField::Topic);
    }

    #[test]
    fn detects_duplicate_messages() {
        let mut state = AppState::new(128);

        let msg1 = MessageDelivery {
            message_id: "msg-001".to_string(),
            sequence: 1,
            payload: vec![1, 2, 3],
            attributes: Default::default(),
            timestamp: 12345,
        };

        let msg2 = MessageDelivery {
            message_id: "msg-002".to_string(),
            sequence: 2,
            payload: vec![4, 5, 6],
            attributes: Default::default(),
            timestamp: 12346,
        };

        // Initially no messages
        assert!(!state.has_message("msg-001"));
        assert_eq!(state.messages.len(), 0);

        // Add first message
        assert!(state.add_message_if_new(msg1.clone()));
        assert_eq!(state.messages.len(), 1);
        assert!(state.has_message("msg-001"));

        // Try to add same message again - should be rejected
        assert!(!state.add_message_if_new(msg1.clone()));
        assert_eq!(state.messages.len(), 1); // Still 1

        // Add different message - should succeed
        assert!(state.add_message_if_new(msg2));
        assert_eq!(state.messages.len(), 2);
        assert!(state.has_message("msg-002"));
    }

    #[test]
    fn validates_topic_names() {
        // Valid names
        assert!(AppState::is_valid_topic_name("my-topic"));
        assert!(AppState::is_valid_topic_name("orders"));
        assert!(AppState::is_valid_topic_name("user_events"));
        assert!(AppState::is_valid_topic_name("app.logs.production"));
        assert!(AppState::is_valid_topic_name("topic123"));

        // Invalid names
        assert!(!AppState::is_valid_topic_name(""));
        assert!(!AppState::is_valid_topic_name("   "));
        assert!(!AppState::is_valid_topic_name("topic with spaces"));
        assert!(!AppState::is_valid_topic_name("topic/slash"));
        assert!(!AppState::is_valid_topic_name("topic@special"));
    }

    #[test]
    fn can_create_topic_validation() {
        let mut state = AppState::new(128);

        // Empty name - invalid
        state.create_topic_name = "".to_string();
        assert!(!state.can_create_topic());

        // Valid name
        state.create_topic_name = "new-topic".to_string();
        assert!(state.can_create_topic());

        // Invalid characters
        state.create_topic_name = "bad topic name".to_string();
        assert!(!state.can_create_topic());
    }
}
