//! Simple example demonstrating how to publish messages to a Sluice topic.
//!
//! This example shows:
//! - Connecting to a Sluice server
//! - Publishing messages to a topic
//! - Handling publish responses
//!
//! # Usage
//!
//! Start a Sluice server in another terminal:
//! ```bash
//! cargo run -p sluice-server
//! ```
//!
//! Then run this example:
//! ```bash
//! cargo run --example simple_publish
//! ```

use sluice_client::{ConnectConfig, SluiceClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Publish a simple message
    let topic = "example-topic";
    let message = b"Hello, Sluice!".to_vec();

    println!("\nPublishing message to topic '{}'...", topic);
    let response = client.publish(topic, message).await?;

    println!("Message published successfully!");
    println!("  Message ID: {}", response.message_id);
    println!("  Sequence: {}", response.sequence);

    // Publish multiple messages
    println!("\nPublishing batch of 5 messages...");
    for i in 1..=5 {
        let message = format!("Message #{}", i).into_bytes();
        let response = client.publish(topic, message).await?;
        println!("  [{}] Published message ID: {}", i, response.message_id);
    }

    println!("\nAll messages published successfully!");
    println!("\nYou can now subscribe to '{}' using:", topic);
    println!("  cargo run -p sluicectl -- subscribe {}", topic);
    println!("  cargo run --example subscribe_and_ack");

    Ok(())
}
