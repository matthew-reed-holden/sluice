//! Application state for lazysluice TUI.

use sluice_client::{InitialPosition, MessageDelivery, Topic, TopicStats};

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
    Metrics,
    ConsumerGroupInput,
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
#[derive(Debug, Default, Clone)]
pub struct AppState {
    pub screen: Screen,
    pub conn_status: ConnStatus,

    // Topic list
    pub topics: Vec<Topic>,
    pub topic_cursor: usize,
    pub visited_topics: std::collections::HashSet<String>, // Track visited topics

    // Tail view
    pub messages: Vec<MessageDelivery>,
    pub message_cursor: usize,
    pub paused: bool,
    pub initial_position: InitialPosition,
    pub current_topic: Option<String>, // Track which topic we're subscribed to

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

    // Metrics tracking (Phase 4)
    pub connection_start: Option<std::time::Instant>,
    pub total_published: u64,
    pub total_consumed: u64,
    pub publish_timestamps: std::collections::VecDeque<std::time::Instant>,
    pub consume_timestamps: std::collections::VecDeque<std::time::Instant>,

    // Consumer group selection (Phase 4)
    pub consumer_group: Option<String>,
    pub consumer_group_input: String,

    // Search/filter (Phase 4)
    pub search_query: String,
    pub search_active: bool,
    pub filtered_messages: Vec<usize>, // Indices into messages vec

    // Phase 5: Topic stats and browse mode
    pub topic_stats: std::collections::HashMap<String, TopicStats>,
    pub browse_mode: bool,
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
            InitialPosition::Offset => InitialPosition::Earliest,
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

    /// Record a publish event for metrics tracking.
    pub fn record_publish(&mut self) {
        self.total_published += 1;
        let now = std::time::Instant::now();
        self.publish_timestamps.push_back(now);

        // Keep only last 60 seconds
        let cutoff = now - std::time::Duration::from_secs(60);
        while self.publish_timestamps.front().is_some_and(|&t| t < cutoff) {
            self.publish_timestamps.pop_front();
        }
    }

    /// Record a consume event for metrics tracking.
    pub fn record_consume(&mut self) {
        self.total_consumed += 1;
        let now = std::time::Instant::now();
        self.consume_timestamps.push_back(now);

        // Keep only last 60 seconds
        let cutoff = now - std::time::Duration::from_secs(60);
        while self.consume_timestamps.front().is_some_and(|&t| t < cutoff) {
            self.consume_timestamps.pop_front();
        }
    }

    /// Calculate publish rate (messages per second) over last 60s.
    pub fn publish_rate(&self) -> f64 {
        let count = self.publish_timestamps.len();
        if count == 0 {
            return 0.0;
        }

        let oldest = self.publish_timestamps.front().unwrap();
        let newest = self.publish_timestamps.back().unwrap();
        let duration = newest.duration_since(*oldest).as_secs_f64();

        if duration > 0.0 {
            count as f64 / duration
        } else {
            0.0
        }
    }

    /// Calculate consume rate (messages per second) over last 60s.
    pub fn consume_rate(&self) -> f64 {
        let count = self.consume_timestamps.len();
        if count == 0 {
            return 0.0;
        }

        let oldest = self.consume_timestamps.front().unwrap();
        let newest = self.consume_timestamps.back().unwrap();
        let duration = newest.duration_since(*oldest).as_secs_f64();

        if duration > 0.0 {
            count as f64 / duration
        } else {
            0.0
        }
    }

    /// Get connection uptime as a formatted string.
    pub fn uptime_string(&self) -> String {
        match self.connection_start {
            Some(start) => {
                let elapsed = start.elapsed();
                let hours = elapsed.as_secs() / 3600;
                let minutes = (elapsed.as_secs() % 3600) / 60;
                let seconds = elapsed.as_secs() % 60;
                format!("{}h {}m {}s", hours, minutes, seconds)
            }
            None => "N/A".to_string(),
        }
    }

    /// Toggle browse mode on/off.
    pub fn toggle_browse_mode(&mut self) {
        self.browse_mode = !self.browse_mode;
    }

    /// Get topic stats for a topic name.
    pub fn get_topic_stats(&self, topic_name: &str) -> Option<&TopicStats> {
        self.topic_stats.get(topic_name)
    }

    /// Apply search filter to messages.
    pub fn apply_search_filter(&mut self) {
        self.filtered_messages.clear();

        if self.search_query.trim().is_empty() {
            // No filter - show all messages
            return;
        }

        let query_lower = self.search_query.to_lowercase();

        for (i, msg) in self.messages.iter().enumerate() {
            // Search in message ID
            if msg.message_id.to_lowercase().contains(&query_lower) {
                self.filtered_messages.push(i);
                continue;
            }

            // Search in payload (if UTF-8)
            if let Ok(payload_str) = std::str::from_utf8(&msg.payload) {
                if payload_str.to_lowercase().contains(&query_lower) {
                    self.filtered_messages.push(i);
                    continue;
                }
            }

            // Search in attributes
            for (key, value) in &msg.attributes {
                if key.to_lowercase().contains(&query_lower)
                    || value.to_lowercase().contains(&query_lower)
                {
                    self.filtered_messages.push(i);
                    break;
                }
            }
        }
    }

    /// Get visible messages (filtered or all).
    pub fn visible_messages(&self) -> Vec<&MessageDelivery> {
        if self.search_active && !self.search_query.trim().is_empty() {
            self.filtered_messages
                .iter()
                .filter_map(|&i| self.messages.get(i))
                .collect()
        } else {
            self.messages.iter().collect()
        }
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
            delivery_count: 0,
        };

        let msg2 = MessageDelivery {
            message_id: "msg-002".to_string(),
            sequence: 2,
            payload: vec![4, 5, 6],
            attributes: Default::default(),
            timestamp: 12346,
            delivery_count: 0,
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

    #[test]
    fn metrics_tracking_publish_events() {
        let mut state = AppState::new(128);

        // Initially zero
        assert_eq!(state.total_published, 0);
        assert_eq!(state.publish_timestamps.len(), 0);

        // Record publish event
        state.record_publish();
        assert_eq!(state.total_published, 1);
        assert_eq!(state.publish_timestamps.len(), 1);

        // Record multiple
        state.record_publish();
        state.record_publish();
        assert_eq!(state.total_published, 3);
        assert_eq!(state.publish_timestamps.len(), 3);
    }

    #[test]
    fn metrics_tracking_consume_events() {
        let mut state = AppState::new(128);

        // Initially zero
        assert_eq!(state.total_consumed, 0);
        assert_eq!(state.consume_timestamps.len(), 0);

        // Record consume event
        state.record_consume();
        assert_eq!(state.total_consumed, 1);
        assert_eq!(state.consume_timestamps.len(), 1);

        // Record multiple
        state.record_consume();
        state.record_consume();
        assert_eq!(state.total_consumed, 3);
        assert_eq!(state.consume_timestamps.len(), 3);
    }

    #[test]
    fn uptime_string_when_not_connected() {
        let state = AppState::new(128);
        assert_eq!(state.uptime_string(), "N/A");
    }

    #[test]
    fn uptime_string_when_connected() {
        let mut state = AppState::new(128);
        state.connection_start = Some(std::time::Instant::now());
        let uptime = state.uptime_string();
        // Should be formatted as "Xh Xm Xs"
        assert!(uptime.contains('h'));
        assert!(uptime.contains('m'));
        assert!(uptime.contains('s'));
    }

    #[test]
    fn search_filter_by_payload() {
        let mut state = AppState::new(128);

        // Add messages
        let msg1 = MessageDelivery {
            message_id: "msg-001".to_string(),
            sequence: 1,
            payload: b"hello world".to_vec(),
            attributes: Default::default(),
            timestamp: 12345,
            delivery_count: 0,
        };

        let msg2 = MessageDelivery {
            message_id: "msg-002".to_string(),
            sequence: 2,
            payload: b"goodbye moon".to_vec(),
            attributes: Default::default(),
            timestamp: 12346,
            delivery_count: 0,
        };

        state.messages.push(msg1);
        state.messages.push(msg2);

        // Search for "hello"
        state.search_query = "hello".to_string();
        state.apply_search_filter();

        assert_eq!(state.filtered_messages.len(), 1);
        assert_eq!(state.filtered_messages[0], 0); // First message

        // Search for "moon"
        state.search_query = "moon".to_string();
        state.apply_search_filter();

        assert_eq!(state.filtered_messages.len(), 1);
        assert_eq!(state.filtered_messages[0], 1); // Second message

        // Search for something that doesn't exist
        state.search_query = "xyz".to_string();
        state.apply_search_filter();

        assert_eq!(state.filtered_messages.len(), 0);
    }

    #[test]
    fn search_filter_by_message_id() {
        let mut state = AppState::new(128);

        let msg = MessageDelivery {
            message_id: "test-uuid-123".to_string(),
            sequence: 1,
            payload: b"payload".to_vec(),
            attributes: Default::default(),
            timestamp: 12345,
            delivery_count: 0,
        };

        state.messages.push(msg);

        // Search by ID
        state.search_query = "uuid".to_string();
        state.apply_search_filter();

        assert_eq!(state.filtered_messages.len(), 1);
        assert_eq!(state.filtered_messages[0], 0);
    }

    #[test]
    fn search_filter_by_attributes() {
        let mut state = AppState::new(128);

        let mut attrs = std::collections::HashMap::new();
        attrs.insert("user_id".to_string(), "12345".to_string());
        attrs.insert("trace_id".to_string(), "abc-def-ghi".to_string());

        let msg = MessageDelivery {
            message_id: "msg-001".to_string(),
            sequence: 1,
            payload: b"data".to_vec(),
            attributes: attrs,
            timestamp: 12345,
            delivery_count: 0,
        };

        state.messages.push(msg);

        // Search by attribute key
        state.search_query = "user_id".to_string();
        state.apply_search_filter();
        assert_eq!(state.filtered_messages.len(), 1);

        // Search by attribute value
        state.search_query = "12345".to_string();
        state.apply_search_filter();
        assert_eq!(state.filtered_messages.len(), 1);

        // Search by trace ID
        state.search_query = "abc-def".to_string();
        state.apply_search_filter();
        assert_eq!(state.filtered_messages.len(), 1);
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut state = AppState::new(128);

        let msg = MessageDelivery {
            message_id: "MSG-001".to_string(),
            sequence: 1,
            payload: b"Hello World".to_vec(),
            attributes: Default::default(),
            timestamp: 12345,
            delivery_count: 0,
        };

        state.messages.push(msg);

        // Lowercase search should find uppercase content
        state.search_query = "msg".to_string();
        state.apply_search_filter();
        assert_eq!(state.filtered_messages.len(), 1);

        state.search_query = "hello".to_string();
        state.apply_search_filter();
        assert_eq!(state.filtered_messages.len(), 1);
    }
}
