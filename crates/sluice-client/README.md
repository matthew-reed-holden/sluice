# sluice-client

A lightweight, async Rust client library for the Sluice message broker.

## Overview

`sluice-client` provides a high-level, type-safe interface for interacting with Sluice servers via gRPC. It handles connection management, credit-based flow control, and provides ergonomic APIs for publishing and subscribing to messages.

## Features

- Async/await API built on `tokio`
- Credit-based flow control for backpressure management
- Support for consumer groups and message acknowledgments
- Configurable initial position (earliest, latest, or specific offset)
- Automatic credit refilling for subscriptions
- TLS support for secure connections
- Minimal dependencies (only 6 core dependencies)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sluice-client = { path = "../sluice-client" }
tokio = { version = "1", features = ["full"] }
```

## Quick Start

```rust
use sluice_client::{SluiceClient, ConnectConfig, InitialPosition};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to server
    let config = ConnectConfig::plaintext("http://localhost:50051");
    let mut client = SluiceClient::connect(config).await?;

    // Publish a message
    let response = client.publish("orders", b"order-123".to_vec()).await?;
    println!("Published message: {}", response.message_id);

    // Subscribe with consumer group
    let mut subscription = client
        .subscribe(
            "orders",           // topic
            Some("processors"), // consumer group
            None,               // subscription ID (auto-generated)
            InitialPosition::Earliest,
            10                  // initial credits
        )
        .await?;

    // Process messages
    while let Some(msg) = subscription.next_message().await? {
        println!("Received: {:?}", msg.payload);

        // Acknowledge message
        subscription.send_ack(&msg.message_id).await?;

        // Refill credits if needed
        subscription.maybe_refill_credits().await?;
    }

    Ok(())
}
```

## Usage Examples

### Publishing Messages

```rust
// Simple publish
let response = client.publish("topic-name", b"message data".to_vec()).await?;

// With attributes (requires constructing PublishRequest manually)
use sluice_proto::sluice::v1::PublishRequest;
let request = PublishRequest {
    topic: "topic-name".to_string(),
    payload: b"message data".to_vec(),
    attributes: vec![
        Attribute { key: "user_id".to_string(), value: "123".to_string() },
    ],
};
let response = client.publish_request(request).await?;
```

### Subscribing to Topics

```rust
// Subscribe from earliest message
let mut sub = client
    .subscribe("topic", Some("group"), None, InitialPosition::Earliest, 10)
    .await?;

// Subscribe from latest (only new messages)
let mut sub = client
    .subscribe("topic", Some("group"), None, InitialPosition::Latest, 10)
    .await?;

// Subscribe from specific offset
let mut sub = client
    .subscribe("topic", Some("group"), None, InitialPosition::Offset(100), 10)
    .await?;
```

### Listing Topics

```rust
let topics = client.list_topics().await?;
for topic in topics {
    println!("Topic: {} (ID: {})", topic.name, topic.topic_id);
}
```

## Connection Configuration

### Plaintext Connection

```rust
let config = ConnectConfig::plaintext("http://localhost:50051");
let client = SluiceClient::connect(config).await?;
```

### TLS Connection

```rust
let config = ConnectConfig::tls("https://sluice.example.com:50051")?;
let client = SluiceClient::connect(config).await?;
```

## Credit-Based Flow Control

Sluice uses credit-based flow control to prevent overwhelming consumers. The client automatically manages credits:

```rust
let mut subscription = client
    .subscribe("topic", Some("group"), None, InitialPosition::Earliest, 10)
    .await?;

while let Some(msg) = subscription.next_message().await? {
    // Process message...

    subscription.send_ack(&msg.message_id).await?;

    // Automatically refills credits when below 50% of initial
    subscription.maybe_refill_credits().await?;
}
```

You can also manually control credits:

```rust
// Send specific number of credits
subscription.send_credits(5).await?;
```

## Error Handling

The client uses `anyhow::Result` for error handling:

```rust
use anyhow::{Context, Result};

async fn publish_with_retry(
    client: &mut SluiceClient,
    topic: &str,
    data: Vec<u8>
) -> Result<String> {
    client
        .publish(topic, data)
        .await
        .context("Failed to publish message")?
        .message_id
        .ok_or_else(|| anyhow::anyhow!("No message ID returned"))
}
```

## API Reference

### `SluiceClient`

- `connect(config: ConnectConfig) -> Result<Self>` - Connect to server
- `publish(topic: &str, payload: Vec<u8>) -> Result<PublishResponse>` - Publish message
- `subscribe(topic: &str, consumer_group: Option<&str>, subscription_id: Option<&str>, initial_position: InitialPosition, initial_credits: i32) -> Result<Subscription>` - Subscribe to topic
- `list_topics() -> Result<Vec<Topic>>` - List all topics

### `Subscription`

- `next_message() -> Result<Option<MessageDelivery>>` - Get next message (blocking)
- `send_ack(message_id: &str) -> Result<()>` - Acknowledge message
- `send_credits(credits: i32) -> Result<()>` - Send credits to server
- `maybe_refill_credits() -> Result<()>` - Refill if below threshold

### `ConnectConfig`

- `plaintext(endpoint: &str) -> Self` - Create plaintext config
- `tls(endpoint: &str) -> Result<Self>` - Create TLS config

## Examples

See the `examples/` directory for complete working examples:

- `simple_publish.rs` - Basic publishing
- `subscribe_and_ack.rs` - Subscription with acknowledgments

Run examples with:

```bash
cargo run --example simple_publish
cargo run --example subscribe_and_ack
```

## License

MIT OR Apache-2.0
