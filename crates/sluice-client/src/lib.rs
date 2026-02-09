//! Sluice client library for connecting to message brokers.
//!
//! This library provides a reusable gRPC client for connecting to Sluice servers,
//! enabling applications to publish messages, subscribe to topics, and list topics.
//!
//! # Example
//!
//! ```no_run
//! use sluice_client::{SluiceClient, ConnectConfig, InitialPosition};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to server
//!     let config = ConnectConfig::plaintext("http://localhost:50051");
//!     let mut client = SluiceClient::connect(config).await?;
//!
//!     // Publish a message
//!     let response = client.publish("my-topic", b"Hello, World!".to_vec()).await?;
//!     println!("Published message {}", response.message_id);
//!
//!     // Subscribe to messages
//!     let mut subscription = client
//!         .subscribe("my-topic", Some("my-group"), None, InitialPosition::Earliest, 10)
//!         .await?;
//!
//!     while let Some(msg) = subscription.next_message().await? {
//!         println!("Received: {:?}", msg.payload);
//!         subscription.send_ack(&msg.message_id).await?;
//!         subscription.maybe_refill_credits().await?;
//!     }
//!
//!     Ok(())
//! }
//! ```

mod connection;
mod subscription;

pub use connection::{ConnectConfig, RetryConfig, SluiceClient};
pub use subscription::{AutoRefillSubscription, CreditConfig, RefillAmount, Subscription};

// Re-export proto types that clients commonly use
pub use sluice_proto::{
    ConsumerGroupStats, InitialPosition, MessageDelivery, PublishResponse, SubscriptionMode,
    Topic, TopicStats,
};
