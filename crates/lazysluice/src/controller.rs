use std::time::Duration;

use crossterm::event::KeyCode;

use crate::app::{AppState, ConnStatus, Screen};
use crate::events::Event;
use sluice_client::{InitialPosition, SluiceClient, Subscription};

/// Controller: owns app state and mutates it in response to events.
pub struct Controller {
    pub state: AppState,
    client: Option<SluiceClient>,
    subscription: Option<Subscription>,
    endpoint: String,
    tls_ca: Option<std::path::PathBuf>,
    tls_domain: Option<String>,
    reconnect_attempt: u32,
    last_reconnect: Option<std::time::Instant>,
}

impl Controller {
    pub fn new(
        endpoint: String,
        tls_ca: Option<std::path::PathBuf>,
        tls_domain: Option<String>,
        credits_window: u32,
    ) -> Self {
        Self {
            state: AppState::new(credits_window),
            client: None,
            subscription: None,
            endpoint,
            tls_ca,
            tls_domain,
            reconnect_attempt: 0,
            last_reconnect: None,
        }
    }

    /// Attempt to connect and load topics.
    #[tracing::instrument(skip(self), fields(endpoint = %self.endpoint))]
    pub async fn connect(&mut self) {
        tracing::info!("Connecting to server");
        self.state.conn_status = ConnStatus::Connecting;
        match SluiceClient::connect_with(
            &self.endpoint,
            self.tls_ca.as_deref(),
            self.tls_domain.as_deref(),
        )
        .await
        {
            Ok(c) => {
                tracing::info!("Connected successfully");
                self.client = Some(c);
                self.state.conn_status = ConnStatus::Connected;
                // Initialize connection start time for metrics
                if self.state.connection_start.is_none() {
                    self.state.connection_start = Some(std::time::Instant::now());
                }
                self.reconnect_attempt = 0;
                self.load_topics().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "Connection failed");
                self.state.conn_status = ConnStatus::Error(e.to_string());
                self.reconnect_attempt += 1;
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn load_topics(&mut self) {
        tracing::debug!("Loading topics");
        if let Some(ref mut c) = self.client {
            match c.list_topics().await {
                Ok(topics) => {
                    tracing::info!(count = topics.len(), "Loaded topics");
                    self.state.topics = topics;
                    self.state.topic_cursor = 0;

                    // Fetch topic stats for all topics
                    self.load_topic_stats().await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load topics");
                    self.state.conn_status = ConnStatus::Error(format!("list_topics: {e}"));
                }
            }
        }
    }

    /// Load stats for all topics.
    #[tracing::instrument(skip(self))]
    async fn load_topic_stats(&mut self) {
        tracing::debug!("Loading topic stats");
        if let Some(ref mut c) = self.client {
            match c.get_topic_stats(vec![]).await {
                Ok(stats) => {
                    tracing::info!(count = stats.len(), "Loaded topic stats");
                    self.state.topic_stats.clear();
                    for stat in stats {
                        self.state.topic_stats.insert(stat.topic.clone(), stat);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load topic stats");
                    // Non-fatal - continue without stats
                }
            }
        }
    }

    /// Start a subscription to the given topic.
    #[tracing::instrument(skip(self), fields(%topic, browse_mode = %self.state.browse_mode))]
    async fn start_subscription(&mut self, topic: String) {
        tracing::info!("Starting subscription");
        if let Some(ref mut c) = self.client {
            let result = if self.state.browse_mode {
                // Browse mode: non-destructive viewing
                tracing::info!("Using browse mode");
                c.subscribe_browse(&topic, self.state.credits_window).await
            } else {
                // Consumer group mode: normal subscription
                c.subscribe(
                    &topic,
                    self.state.consumer_group.as_deref(),
                    None, // consumer_id
                    self.state.initial_position,
                    self.state.credits_window,
                )
                .await
            };

            match result {
                Ok(sub) => {
                    tracing::info!("Subscription started");
                    self.subscription = Some(sub);
                    self.state.messages.clear();
                    self.state.message_cursor = 0;
                    self.state.current_topic = Some(topic.clone());  // Track subscribed topic
                    self.state.visited_topics.insert(topic);  // Mark as visited
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to start subscription");
                    self.state.conn_status = ConnStatus::Error(format!("subscribe failed: {e}"));
                }
            }
        }
    }

    /// Poll the subscription for new messages (non-blocking check).
    /// Returns true if a message was received.
    pub async fn poll_subscription(&mut self) -> bool {
        if self.state.paused {
            return false;
        }
        if let Some(ref mut sub) = self.subscription {
            // Check for credit refill
            let _ = sub.maybe_refill_credits().await;

            // Try to get next message with a short timeout
            match tokio::time::timeout(Duration::from_millis(10), sub.next_message()).await {
                Ok(Ok(Some(msg))) => {
                    // Only add if not a duplicate (prevents double-display on reconnect)
                    let is_new = self.state.add_message_if_new(msg);
                    if is_new {
                        // Track metrics
                        self.state.record_consume();
                        // Auto-scroll to latest if cursor is at end
                        if self.state.message_cursor + 1 >= self.state.messages.len().saturating_sub(1)
                        {
                            self.state.message_cursor = self.state.messages.len().saturating_sub(1);
                        }
                    }
                    return is_new;  // Return true if new, false if duplicate
                }
                Ok(Ok(None)) => {
                    // Stream ended
                    self.subscription = None;
                    self.state.conn_status = ConnStatus::Error("subscription ended".to_string());
                }
                Ok(Err(e)) => {
                    self.subscription = None;
                    self.state.conn_status = ConnStatus::Error(format!("stream error: {e}"));
                }
                Err(_) => {
                    // Timeout, no message available
                }
            }
        }
        false
    }

    /// Handle an incoming event; returns false if quit requested.
    pub async fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Quit => return false,
            Event::Tick => {
                // Check for connection errors and trigger auto-reconnect
                self.maybe_reconnect().await;
            }
            Event::Key(key) => {
                if !self.handle_key(key.code).await {
                    return false;
                }
            }
        }
        true
    }

    /// Trigger reconnection if in error state and backoff elapsed.
    async fn maybe_reconnect(&mut self) {
        if let ConnStatus::Error(_) = &self.state.conn_status {
            // Check if enough time has elapsed since last reconnect attempt
            let backoff = reconnect_backoff(self.reconnect_attempt);
            let should_reconnect = match self.last_reconnect {
                Some(last) => last.elapsed() >= backoff,
                None => true,
            };

            if should_reconnect {
                self.last_reconnect = Some(std::time::Instant::now());

                // Save topic for potential subscription restart (use the actual subscribed topic, not publish_topic)
                let topic = self.state.current_topic.clone();

                // Attempt to reconnect
                self.connect().await;

                // If connected and we had a topic, restart subscription
                if matches!(self.state.conn_status, ConnStatus::Connected) {
                    if let Some(topic_name) = topic {
                        self.start_subscription(topic_name).await;
                    }
                }
            }
        }
    }

    async fn handle_key(&mut self, code: KeyCode) -> bool {
        // Handle publish screen input separately
        if self.state.screen == Screen::Publish {
            return self.handle_publish_key(code).await;
        }

        // Handle create topic screen input
        if self.state.screen == Screen::CreateTopic {
            return self.handle_create_topic_key(code).await;
        }

        // Handle message detail screen
        if self.state.screen == Screen::MessageDetail {
            return self.handle_message_detail_key(code);
        }

        // Handle consumer group input screen
        if self.state.screen == Screen::ConsumerGroupInput {
            return self.handle_consumer_group_key(code).await;
        }

        // Handle metrics screen
        if self.state.screen == Screen::Metrics {
            return self.handle_metrics_key(code);
        }

        // Handle search mode in Tail screen
        if self.state.screen == Screen::Tail && self.state.search_active {
            return self.handle_search_key(code);
        }

        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Char('?') => {
                self.state.show_help = !self.state.show_help;
            }
            KeyCode::Tab => {
                self.state.screen = match self.state.screen {
                    Screen::TopicList => Screen::Tail,
                    Screen::Tail => Screen::Publish,
                    Screen::Publish => Screen::TopicList,
                    Screen::Help => Screen::TopicList,
                    Screen::CreateTopic => Screen::TopicList,
                    Screen::MessageDetail => Screen::Tail,
                    Screen::Metrics => Screen::TopicList,
                    Screen::ConsumerGroupInput => Screen::TopicList,
                };
            }
            KeyCode::Char('p') => {
                self.state.screen = Screen::Publish;
            }
            KeyCode::Char('c') => {
                if self.state.screen == Screen::TopicList {
                    self.state.screen = Screen::CreateTopic;
                    self.state.create_topic_name.clear();
                    self.state.create_topic_status = None;
                }
            }
            KeyCode::Char('i') => {
                if self.state.screen == Screen::Tail {
                    if let Some(msg) = self.state.selected_message().cloned() {
                        self.state.detail_message = Some(msg);
                        self.state.screen = Screen::MessageDetail;
                    }
                }
            }
            KeyCode::Char('m') => {
                // Open metrics dashboard
                if matches!(self.state.screen, Screen::TopicList | Screen::Tail) {
                    self.state.screen = Screen::Metrics;
                }
            }
            KeyCode::Char('g') => {
                // Open consumer group selection
                if self.state.screen == Screen::TopicList {
                    self.state.screen = Screen::ConsumerGroupInput;
                    self.state.consumer_group_input.clear();
                }
            }
            KeyCode::Char('/') => {
                // Activate search mode
                if self.state.screen == Screen::Tail {
                    self.state.search_active = true;
                    self.state.search_query.clear();
                }
            }
            KeyCode::Char('b') => {
                // Toggle browse mode
                if matches!(self.state.screen, Screen::TopicList | Screen::Tail) {
                    self.state.toggle_browse_mode();
                    // If we have an active topic, restart subscription with new mode
                    if let Some(topic) = self.state.current_topic.clone() {
                        self.start_subscription(topic).await;
                    }
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor_down();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor_up();
            }
            KeyCode::PageDown => {
                self.page_down();
            }
            KeyCode::PageUp => {
                self.page_up();
            }
            KeyCode::Home => {
                self.jump_to_top();
            }
            KeyCode::End => {
                self.jump_to_bottom();
            }
            KeyCode::Enter => {
                if self.state.screen == Screen::TopicList {
                    if let Some(t) = self.state.selected_topic().cloned() {
                        self.state.publish_topic = t.name.clone();
                        self.state.screen = Screen::Tail;
                        self.start_subscription(t.name).await;
                    }
                }
            }
            KeyCode::Char(' ') => {
                if self.state.screen == Screen::Tail {
                    self.state.paused = !self.state.paused;
                }
            }
            KeyCode::Char('a') => {
                if self.state.screen == Screen::Tail {
                    if let Some(m) = self.state.selected_message() {
                        let msg_id = m.message_id.clone();
                        self.state.acked_ids.insert(msg_id.clone());
                        // Send Ack RPC
                        self.send_ack(msg_id).await;
                    }
                }
            }
            KeyCode::Char('e') => {
                if self.state.screen == Screen::Tail {
                    self.state.initial_position = InitialPosition::Earliest;
                    // Restart subscription with new position if we have a topic
                    if let Some(topic) = self.state.selected_topic().cloned() {
                        self.start_subscription(topic.name).await;
                    }
                }
            }
            KeyCode::Char('l') => {
                if self.state.screen == Screen::Tail {
                    self.state.initial_position = InitialPosition::Latest;
                    // Restart subscription with new position if we have a topic
                    if let Some(topic) = self.state.selected_topic().cloned() {
                        self.start_subscription(topic.name).await;
                    }
                }
            }
            _ => {}
        }
        true
    }

    async fn handle_publish_key(&mut self, code: KeyCode) -> bool {
        use crate::app::PublishInputField;

        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.state.screen = Screen::TopicList;
            }
            KeyCode::Tab => {
                // Cycle between Topic and Payload fields
                self.state.cycle_publish_field();
            }
            KeyCode::Char(c) => {
                // Add character to active field
                match self.state.publish_active_field {
                    PublishInputField::Topic => {
                        self.state.publish_topic.push(c);
                    }
                    PublishInputField::Payload => {
                        self.state.publish_payload.push(c);
                    }
                }
                self.state.publish_status = None;
            }
            KeyCode::Backspace => {
                // Delete from active field
                match self.state.publish_active_field {
                    PublishInputField::Topic => {
                        self.state.publish_topic.pop();
                    }
                    PublishInputField::Payload => {
                        self.state.publish_payload.pop();
                    }
                }
                self.state.publish_status = None;
            }
            KeyCode::Enter => {
                self.submit_publish().await;
            }
            _ => {}
        }
        true
    }

    async fn submit_publish(&mut self) {
        if !self.state.can_publish() {
            self.state.publish_status = Some("Error: topic and payload required".to_string());
            return;
        }

        if let Some(ref mut c) = self.client {
            let topic = self.state.publish_topic.clone();
            let payload = self.state.publish_payload.clone().into_bytes();
            tracing::debug!(topic = %topic, payload_len = payload.len(), "Publishing message");
            match c.publish(&topic, payload).await {
                Ok(resp) => {
                    tracing::info!(message_id = %resp.message_id, seq = resp.sequence, "Message published");
                    self.state.publish_status = Some(format!(
                        "Published: {} (seq {})",
                        resp.message_id, resp.sequence
                    ));
                    self.state.publish_payload.clear();
                    // Track metrics
                    self.state.record_publish();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Publish failed");
                    self.state.publish_status = Some(format!("Error: {e}"));
                }
            }
        } else {
            self.state.publish_status = Some("Error: not connected".to_string());
        }
    }

    async fn handle_create_topic_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.state.screen = Screen::TopicList;
            }
            KeyCode::Char(c) => {
                self.state.create_topic_name.push(c);
                self.state.create_topic_status = None;
            }
            KeyCode::Backspace => {
                self.state.create_topic_name.pop();
                self.state.create_topic_status = None;
            }
            KeyCode::Enter => {
                self.submit_create_topic().await;
            }
            _ => {}
        }
        true
    }

    async fn submit_create_topic(&mut self) {
        if !self.state.can_create_topic() {
            self.state.create_topic_status =
                Some("Error: topic name must be alphanumeric with -_.".to_string());
            return;
        }

        if let Some(ref mut c) = self.client {
            let topic = self.state.create_topic_name.clone();
            tracing::debug!(topic = %topic, "Creating topic");
            // Create topic by publishing an empty message
            match c.publish(&topic, vec![]).await {
                Ok(_) => {
                    tracing::info!(topic = %topic, "Topic created");
                    self.state.create_topic_status = Some(format!("Created: {}", topic));
                    // Reload topics to show the new one
                    self.load_topics().await;
                    // Return to topic list after brief delay would be nice, but for now stay in dialog
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Topic creation failed");
                    self.state.create_topic_status = Some(format!("Error: {e}"));
                }
            }
        } else {
            self.state.create_topic_status = Some("Error: not connected".to_string());
        }
    }

    fn handle_message_detail_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.state.screen = Screen::Tail;
                self.state.detail_message = None;
            }
            _ => {}
        }
        true
    }

    async fn handle_consumer_group_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.state.screen = Screen::TopicList;
                self.state.consumer_group_input.clear();
            }
            KeyCode::Enter => {
                // Set consumer group (empty = None = default)
                if self.state.consumer_group_input.trim().is_empty() {
                    self.state.consumer_group = None;
                } else {
                    self.state.consumer_group = Some(self.state.consumer_group_input.clone());
                }
                self.state.screen = Screen::TopicList;
            }
            KeyCode::Char(c) => {
                self.state.consumer_group_input.push(c);
            }
            KeyCode::Backspace => {
                self.state.consumer_group_input.pop();
            }
            _ => {}
        }
        true
    }

    fn handle_metrics_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.state.screen = Screen::TopicList;
            }
            _ => {}
        }
        true
    }

    fn handle_search_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                // Exit search mode
                self.state.search_active = false;
                self.state.search_query.clear();
                self.state.filtered_messages.clear();
            }
            KeyCode::Enter => {
                // Apply filter
                self.state.apply_search_filter();
                // Exit search input mode but keep filter active
                self.state.search_active = false;
            }
            KeyCode::Char(c) => {
                self.state.search_query.push(c);
                // Live filter as user types
                self.state.apply_search_filter();
            }
            KeyCode::Backspace => {
                self.state.search_query.pop();
                // Update filter
                self.state.apply_search_filter();
            }
            _ => {}
        }
        true
    }

    /// Send an Ack for a message ID via the subscription.
    #[tracing::instrument(skip(self), fields(%message_id))]
    async fn send_ack(&mut self, message_id: String) {
        tracing::debug!("Sending ack");
        if let Some(ref sub) = self.subscription {
            // Ack errors are non-fatal for MVP - just log
            if let Err(e) = sub.send_ack(&message_id).await {
                tracing::warn!("Failed to send ack: {e}");
            }
        }
    }

    fn move_cursor_down(&mut self) {
        match self.state.screen {
            Screen::TopicList => {
                if self.state.topic_cursor + 1 < self.state.topics.len() {
                    self.state.topic_cursor += 1;
                }
            }
            Screen::Tail => {
                if self.state.message_cursor + 1 < self.state.messages.len() {
                    self.state.message_cursor += 1;
                }
            }
            _ => {}
        }
    }

    fn move_cursor_up(&mut self) {
        match self.state.screen {
            Screen::TopicList => {
                self.state.topic_cursor = self.state.topic_cursor.saturating_sub(1);
            }
            Screen::Tail => {
                self.state.message_cursor = self.state.message_cursor.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn page_down(&mut self) {
        const PAGE_SIZE: usize = 10;
        match self.state.screen {
            Screen::TopicList => {
                let max = self.state.topics.len().saturating_sub(1);
                self.state.topic_cursor = (self.state.topic_cursor + PAGE_SIZE).min(max);
            }
            Screen::Tail => {
                let max = self.state.messages.len().saturating_sub(1);
                self.state.message_cursor = (self.state.message_cursor + PAGE_SIZE).min(max);
            }
            _ => {}
        }
    }

    fn page_up(&mut self) {
        const PAGE_SIZE: usize = 10;
        match self.state.screen {
            Screen::TopicList => {
                self.state.topic_cursor = self.state.topic_cursor.saturating_sub(PAGE_SIZE);
            }
            Screen::Tail => {
                self.state.message_cursor = self.state.message_cursor.saturating_sub(PAGE_SIZE);
            }
            _ => {}
        }
    }

    fn jump_to_top(&mut self) {
        match self.state.screen {
            Screen::TopicList => {
                self.state.topic_cursor = 0;
            }
            Screen::Tail => {
                self.state.message_cursor = 0;
            }
            _ => {}
        }
    }

    fn jump_to_bottom(&mut self) {
        match self.state.screen {
            Screen::TopicList => {
                self.state.topic_cursor = self.state.topics.len().saturating_sub(1);
            }
            Screen::Tail => {
                self.state.message_cursor = self.state.messages.len().saturating_sub(1);
            }
            _ => {}
        }
    }

    /// Returns the backoff duration for the current reconnect attempt.
    #[allow(dead_code)]
    pub fn current_backoff(&self) -> Duration {
        reconnect_backoff(self.reconnect_attempt)
    }
}

#[allow(dead_code)]
pub(crate) fn reconnect_backoff(attempt: u32) -> Duration {
    // Exponential backoff starting at 100ms, capped at 5s.
    let base_ms: u64 = 100;
    let cap_ms: u64 = 5_000;
    let exp = attempt.min(16);
    let factor = 1u64.checked_shl(exp).unwrap_or(u64::MAX);
    let delay_ms = base_ms.saturating_mul(factor);
    Duration::from_millis(delay_ms.min(cap_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases_and_caps() {
        let d0 = reconnect_backoff(0);
        let d1 = reconnect_backoff(1);
        let d2 = reconnect_backoff(2);
        assert!(d0 < d1);
        assert!(d1 < d2);

        let capped = reconnect_backoff(999);
        assert_eq!(capped, Duration::from_millis(5_000));
    }
}
