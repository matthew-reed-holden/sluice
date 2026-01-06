//! Example demonstrating subscription with message acknowledgments.
//!
//! This example shows:
//! - Connecting to a Sluice server
//! - Subscribing to a topic with a consumer group
//! - Processing messages with acknowledgments
//! - Credit-based flow control
//!
//! # Usage
//!
//! First, publish some messages using:
//! ```bash
//! cargo run --example simple_publish
//! ```
//!
//! Then run this subscriber:
//! ```bash
//! cargo run --example subscribe_and_ack
//! ```
//!
//! # Consumer Groups
//!
//! To run multiple consumers that share the workload, run this example
//! in multiple terminals. Messages will be distributed across the group.

use anyhow::Result;
use sluice_client::{ConnectConfig, InitialPosition, SluiceClient};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Connect to Sluice server
    println!("Connecting to Sluice server at http://localhost:50051...");
    let config = ConnectConfig::plaintext("http://localhost:50051");
    let mut client = SluiceClient::connect(config).await?;
    println!("Connected successfully!");

    // Subscribe to topic
    let topic = "example-topic";
    let consumer_group = Some("example-consumer-group");
    let initial_credits = 10;

    println!("\nSubscribing to topic '{}'", topic);
    println!("  Consumer group: {:?}", consumer_group);
    println!("  Initial position: Earliest");
    println!("  Initial credits: {}", initial_credits);

    let mut subscription = client
        .subscribe(
            topic,
            consumer_group,
            None, // Auto-generate subscription ID
            InitialPosition::Earliest,
            initial_credits,
        )
        .await?;

    println!("\nSubscribed successfully! Waiting for messages...");
    println!("Press Ctrl+C to stop.\n");

    // Process messages
    let mut message_count = 0;

    loop {
        // Wait for next message with timeout
        match tokio::time::timeout(Duration::from_secs(30), subscription.next_message()).await {
            Ok(Ok(Some(msg))) => {
                message_count += 1;

                println!("─────────────────────────────────────────");
                println!("Received message #{}:", message_count);
                println!("  Message ID: {}", msg.message_id);
                println!("  Sequence: {}", msg.sequence);

                // Display payload as string if valid UTF-8
                match std::str::from_utf8(&msg.payload) {
                    Ok(text) => println!("  Payload: {}", text),
                    Err(_) => println!("  Payload: {} bytes (binary)", msg.payload.len()),
                }

                // Display attributes if present
                if !msg.attributes.is_empty() {
                    println!("  Attributes:");
                    for (key, value) in &msg.attributes {
                        println!("    {}: {}", key, value);
                    }
                }

                // Acknowledge the message
                subscription.send_ack(&msg.message_id).await?;
                println!("  ✓ Acknowledged");

                // Refill credits if needed (maintains flow control)
                subscription.maybe_refill_credits().await?;
            }
            Ok(Ok(None)) => {
                println!("Subscription closed by server");
                break;
            }
            Ok(Err(e)) => {
                eprintln!("Error receiving message: {}", e);
                break;
            }
            Err(_) => {
                println!("\nNo messages received for 30 seconds.");
                println!("Try publishing messages with:");
                println!("  cargo run --example simple_publish");
                println!("\nStill waiting...");
            }
        }
    }

    println!("\n─────────────────────────────────────────");
    println!("Total messages processed: {}", message_count);

    Ok(())
}
