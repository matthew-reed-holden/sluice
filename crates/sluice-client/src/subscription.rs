//! Subscription handling for Sluice client.

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::Streaming;

use sluice_proto::sluice::v1::sluice_client::SluiceClient as ProtoClient;
use sluice_proto::sluice::v1::{
    subscribe_downstream, subscribe_upstream, Ack, CreditGrant, InitialPosition, MessageDelivery,
    SubscribeDownstream, SubscribeUpstream, SubscriptionInit, SubscriptionMode,
};

/// Configures how credits are refilled.
#[derive(Debug, Clone)]
pub struct CreditConfig {
    /// Size of the credit window.
    pub window_size: u32,
    /// Threshold (as a fraction of window_size) below which refill is triggered.
    /// E.g., 0.5 means refill when credits fall below 50% of window.
    pub refill_threshold: f32,
    /// How many credits to add when refilling.
    pub refill_amount: RefillAmount,
}

impl Default for CreditConfig {
    fn default() -> Self {
        Self {
            window_size: 100,
            refill_threshold: 0.5,
            refill_amount: RefillAmount::ToWindow,
        }
    }
}

impl CreditConfig {
    /// Create a credit config with custom window size.
    pub fn with_window(window_size: u32) -> Self {
        Self {
            window_size,
            ..Default::default()
        }
    }

    /// Set the refill threshold.
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.refill_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the refill amount strategy.
    pub fn refill(mut self, amount: RefillAmount) -> Self {
        self.refill_amount = amount;
        self
    }
}

/// How many credits to add when refilling.
#[derive(Debug, Clone, Copy)]
pub enum RefillAmount {
    /// Refill to reach the window size.
    ToWindow,
    /// Refill a fixed number of credits.
    Fixed(u32),
    /// Refill a ratio of the window size.
    Ratio(f32),
}

impl RefillAmount {
    /// Calculate the credits to grant based on current remaining credits.
    pub fn calculate(&self, window_size: u32, remaining: u32) -> u32 {
        match self {
            RefillAmount::ToWindow => window_size.saturating_sub(remaining),
            RefillAmount::Fixed(n) => *n,
            RefillAmount::Ratio(r) => (window_size as f32 * r) as u32,
        }
    }
}

/// A handle for controlling an active subscription.
///
/// Manages credit-based flow control and provides methods for receiving
/// messages and sending acknowledgments.
pub struct Subscription {
    /// Sender for upstream messages (credits, acks).
    tx: mpsc::Sender<SubscribeUpstream>,
    /// Receiver for downstream message deliveries.
    rx: Streaming<SubscribeDownstream>,
    /// Credit configuration.
    credit_config: CreditConfig,
    /// Remaining credits before refill is needed.
    remaining_credits: u32,
}

impl Subscription {
    /// Start a new subscription.
    pub(crate) async fn start(
        client: &mut ProtoClient<Channel>,
        topic: String,
        consumer_group: Option<String>,
        consumer_id: Option<String>,
        initial_position: InitialPosition,
        mode: SubscriptionMode,
        credits_window: u32,
    ) -> Result<Self> {
        let credit_config = CreditConfig::with_window(credits_window);
        Self::start_with_config(
            client,
            topic,
            consumer_group,
            consumer_id,
            initial_position,
            mode,
            credit_config,
        )
        .await
    }

    /// Start a new subscription with custom credit configuration.
    pub(crate) async fn start_with_config(
        client: &mut ProtoClient<Channel>,
        topic: String,
        consumer_group: Option<String>,
        consumer_id: Option<String>,
        initial_position: InitialPosition,
        mode: SubscriptionMode,
        credit_config: CreditConfig,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<SubscribeUpstream>(32);

        // Send init message
        let init = SubscribeUpstream {
            request: Some(subscribe_upstream::Request::Init(SubscriptionInit {
                topic,
                consumer_group: consumer_group.unwrap_or_else(|| "default".to_string()),
                consumer_id: consumer_id.unwrap_or_default(),
                initial_position: initial_position.into(),
                offset: 0,
                mode: mode.into(),
            })),
        };
        tx.send(init)
            .await
            .map_err(|_| anyhow!("failed to send subscription init"))?;

        // Send initial credit grant
        let credit = SubscribeUpstream {
            request: Some(subscribe_upstream::Request::Credit(CreditGrant {
                credits: credit_config.window_size,
            })),
        };
        tx.send(credit)
            .await
            .map_err(|_| anyhow!("failed to send initial credits"))?;

        // Start the bidirectional stream
        let stream = ReceiverStream::new(rx);
        let response = client
            .subscribe(stream)
            .await
            .context("subscribe RPC failed")?;

        let remaining_credits = credit_config.window_size;

        Ok(Self {
            tx,
            rx: response.into_inner(),
            credit_config,
            remaining_credits,
        })
    }

    /// Get the next message delivery, returning None if stream ends.
    pub async fn next_message(&mut self) -> Result<Option<MessageDelivery>> {
        match self.rx.next().await {
            Some(Ok(downstream)) => {
                if let Some(subscribe_downstream::Response::Delivery(msg)) = downstream.response {
                    self.consume_credit();
                    Ok(Some(msg))
                } else {
                    // Heartbeat or other message type - continue receiving
                    Ok(None)
                }
            }
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Check if credits should be refilled and send grant if needed.
    /// Returns true if a grant was sent.
    pub async fn maybe_refill_credits(&mut self) -> Result<bool> {
        let threshold =
            (self.credit_config.window_size as f32 * self.credit_config.refill_threshold) as u32;
        if self.remaining_credits < threshold {
            let grant = self.credit_config.refill_amount.calculate(
                self.credit_config.window_size,
                self.remaining_credits,
            );
            if grant > 0 {
                self.send_credit(grant).await?;
                self.remaining_credits += grant;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Decrement remaining credits (called when a message is received).
    fn consume_credit(&mut self) {
        self.remaining_credits = self.remaining_credits.saturating_sub(1);
    }

    /// Send a CreditGrant message.
    pub async fn send_credit(&self, credits: u32) -> Result<()> {
        self.tx
            .send(SubscribeUpstream {
                request: Some(subscribe_upstream::Request::Credit(CreditGrant { credits })),
            })
            .await
            .map_err(|_| anyhow!("subscription channel closed"))
    }

    /// Send an Ack message for a specific message ID.
    pub async fn send_ack(&self, message_id: &str) -> Result<()> {
        self.tx
            .send(SubscribeUpstream {
                request: Some(subscribe_upstream::Request::Ack(Ack {
                    message_id: message_id.to_string(),
                })),
            })
            .await
            .map_err(|_| anyhow!("subscription channel closed"))
    }

    /// Get the configured credits window size.
    pub fn credits_window(&self) -> u32 {
        self.credit_config.window_size
    }

    /// Get the current remaining credits.
    pub fn remaining_credits(&self) -> u32 {
        self.remaining_credits
    }

    /// Get the credit configuration.
    pub fn credit_config(&self) -> &CreditConfig {
        &self.credit_config
    }
}

/// A subscription wrapper that automatically refills credits in the background.
///
/// This provides a higher-level API where credit management is handled
/// automatically, allowing the user to focus on processing messages.
pub struct AutoRefillSubscription {
    inner: Subscription,
    /// Handle to the background task (dropped when subscription ends)
    _refill_task: Option<tokio::task::JoinHandle<()>>,
}

impl AutoRefillSubscription {
    /// Create a new auto-refill subscription from an existing subscription.
    ///
    /// The auto-refill task will check and refill credits after each message.
    pub fn new(subscription: Subscription) -> Self {
        Self {
            inner: subscription,
            _refill_task: None,
        }
    }

    /// Get the next message, automatically refilling credits as needed.
    pub async fn next_message(&mut self) -> Result<Option<MessageDelivery>> {
        let msg = self.inner.next_message().await?;
        if msg.is_some() {
            // Auto-refill after receiving a message
            let _ = self.inner.maybe_refill_credits().await;
        }
        Ok(msg)
    }

    /// Send an acknowledgment for a message.
    pub async fn send_ack(&self, message_id: &str) -> Result<()> {
        self.inner.send_ack(message_id).await
    }

    /// Get the current remaining credits.
    pub fn remaining_credits(&self) -> u32 {
        self.inner.remaining_credits()
    }

    /// Get the configured window size.
    pub fn credits_window(&self) -> u32 {
        self.inner.credits_window()
    }

    /// Convert back to a manual subscription for fine-grained control.
    pub fn into_manual(self) -> Subscription {
        self.inner
    }
}
