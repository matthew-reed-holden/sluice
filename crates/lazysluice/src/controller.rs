use std::time::Duration;

use crossterm::event::KeyCode;

use crate::app::{AppState, ConnStatus, Screen};
use crate::events::Event;
use crate::grpc::client::{GrpcClient, Subscription};
use crate::proto::sluice::v1::InitialPosition;

/// Controller: owns app state and mutates it in response to events.
pub struct Controller {
    pub state: AppState,
    client: Option<GrpcClient>,
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
        match GrpcClient::connect(
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
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load topics");
                    self.state.conn_status = ConnStatus::Error(format!("list_topics: {e}"));
                }
            }
        }
    }

    /// Start a subscription to the given topic.
    #[tracing::instrument(skip(self), fields(%topic))]
    async fn start_subscription(&mut self, topic: String) {
        tracing::info!("Starting subscription");
        if let Some(ref mut c) = self.client {
            match c
                .subscribe(
                    topic,
                    None, // consumer_group: default
                    None, // consumer_id
                    InitialPosition::Latest,
                    self.state.credits_window,
                )
                .await
            {
                Ok(sub) => {
                    tracing::info!("Subscription started");
                    self.subscription = Some(sub);
                    self.state.messages.clear();
                    self.state.message_cursor = 0;
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
                    self.state.messages.push(msg);
                    // Auto-scroll to latest if cursor is at end
                    if self.state.message_cursor + 1 >= self.state.messages.len().saturating_sub(1)
                    {
                        self.state.message_cursor = self.state.messages.len().saturating_sub(1);
                    }
                    return true;
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

                // Save topic for potential subscription restart
                let topic = self.state.publish_topic.clone();

                // Attempt to reconnect
                self.connect().await;

                // If connected and we had a topic, restart subscription
                if matches!(self.state.conn_status, ConnStatus::Connected) && !topic.is_empty() {
                    self.start_subscription(topic).await;
                }
            }
        }
    }

    async fn handle_key(&mut self, code: KeyCode) -> bool {
        // Handle publish screen input separately
        if self.state.screen == Screen::Publish {
            return self.handle_publish_key(code).await;
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
                };
            }
            KeyCode::Char('p') => {
                self.state.screen = Screen::Publish;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor_down();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor_up();
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
            _ => {}
        }
        true
    }

    async fn handle_publish_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc | KeyCode::Tab => {
                self.state.screen = Screen::TopicList;
            }
            KeyCode::Char(c) => {
                // Add character to payload
                self.state.publish_payload.push(c);
                self.state.publish_status = None;
            }
            KeyCode::Backspace => {
                self.state.publish_payload.pop();
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
            match c.publish(topic, payload).await {
                Ok(resp) => {
                    tracing::info!(message_id = %resp.message_id, seq = resp.sequence, "Message published");
                    self.state.publish_status = Some(format!(
                        "Published: {} (seq {})",
                        resp.message_id, resp.sequence
                    ));
                    self.state.publish_payload.clear();
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

    /// Send an Ack for a message ID via the subscription.
    #[tracing::instrument(skip(self), fields(%message_id))]
    async fn send_ack(&mut self, message_id: String) {
        tracing::debug!("Sending ack");
        if let Some(ref sub) = self.subscription {
            // Ack errors are non-fatal for MVP - just log
            if let Err(e) = sub.send_ack(message_id).await {
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
