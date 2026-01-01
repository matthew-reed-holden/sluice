# Quickstart: Sluice MVP (v0.1)

**Feature**: 001-mvp-core  
**Date**: 2026-01-01

## Prerequisites

- Rust 1.75+ (latest stable)
- `protoc` (Protocol Buffer compiler)
- Linux or macOS

## Build

```bash
# Clone and build
git clone https://github.com/heisenbergs-uncertainty/sluice.git
cd sluice
cargo build --release

# Binary location
./target/release/sluice --help
```

## Run Server

```bash
# Start with defaults (port 50051, data in ./sluice-data/)
./target/release/sluice serve

# Or with custom settings
./target/release/sluice serve \
  --port 9000 \
  --data-dir /var/lib/sluice \
  --log-level info
```

## Configuration Options

| Flag              | Env Var                       | Default       | Description                                 |
| ----------------- | ----------------------------- | ------------- | ------------------------------------------- |
| `--port`          | `SLUICE_PORT`                 | 50051         | gRPC server port                            |
| `--data-dir`      | `SLUICE_DATA_DIR`             | ./sluice-data | SQLite database directory                   |
| `--log-level`     | `SLUICE_LOG_LEVEL`            | info          | Log level (trace, debug, info, warn, error) |
| `--otel-endpoint` | `OTEL_EXPORTER_OTLP_ENDPOINT` | (none)        | OpenTelemetry collector endpoint            |

## Test with grpcurl

```bash
# Install grpcurl if needed
brew install grpcurl  # macOS
# or go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest

# Publish a message
grpcurl -plaintext -d '{
  "topic": "orders",
  "payload": "eyJvcmRlcl9pZCI6IDEyM30=",
  "attributes": {"source": "test"}
}' localhost:50051 sluice.v1.Sluice/Publish

# Response:
# {
#   "messageId": "01932f4c-5b6d-7000-8000-123456789abc",
#   "sequence": "1",
#   "timestamp": "1767225600000"
# }
```

## Rust Client Example

```rust
use sluice::proto::{
    sluice_client::SluiceClient,
    PublishRequest, SubscribeUpstream, SubscriptionInit,
    CreditGrant, InitialPosition,
};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = SluiceClient::connect("http://localhost:50051").await?;

    // Publish a message
    let response = client.publish(PublishRequest {
        topic: "orders".into(),
        payload: b"hello world".to_vec(),
        attributes: Default::default(),
    }).await?;

    println!("Published: {:?}", response.into_inner());

    // Subscribe to messages
    let (tx, rx) = tokio::sync::mpsc::channel(100);

    // Send init
    tx.send(SubscribeUpstream {
        request: Some(subscribe_upstream::Request::Init(SubscriptionInit {
            topic: "orders".into(),
            consumer_group: "my-group".into(),
            consumer_id: "consumer-1".into(),
            initial_position: InitialPosition::Earliest as i32,
        })),
    }).await?;

    // Grant credits
    tx.send(SubscribeUpstream {
        request: Some(subscribe_upstream::Request::Credit(CreditGrant {
            credits: 10,
        })),
    }).await?;

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut response_stream = client.subscribe(stream).await?.into_inner();

    while let Some(msg) = response_stream.next().await {
        let delivery = msg?.delivery.unwrap();
        println!("Received: {} (seq {})", delivery.message_id, delivery.sequence);

        // Acknowledge
        tx.send(SubscribeUpstream {
            request: Some(subscribe_upstream::Request::Ack(Ack {
                message_id: delivery.message_id,
            })),
        }).await?;
    }

    Ok(())
}
```

## Metrics

When `--otel-endpoint` is configured, the following metrics are exported:

| Metric                            | Type      | Description                             |
| --------------------------------- | --------- | --------------------------------------- |
| `sluice_publish_total`            | Counter   | Total publish attempts                  |
| `sluice_publish_latency_seconds`  | Histogram | Publish latency (request to fsync)      |
| `sluice_messages_delivered_total` | Counter   | Messages delivered to consumers         |
| `sluice_subscription_lag`         | Gauge     | Consumer lag (max_seq - cursor)         |
| `sluice_backpressure_active`      | Gauge     | 1 if consumer has 0 credits and lag > 0 |

## Graceful Shutdown

Send `SIGTERM` or `SIGINT` to gracefully shutdown:

```bash
kill -TERM $(pgrep sluice)
```

The server will:

1. Stop accepting new connections
2. Drain pending writes
3. Close all streams
4. Exit cleanly

## Development

```bash
# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run -- serve

# Check formatting and lints
cargo fmt --check
cargo clippy -- -D warnings
```

## Troubleshooting

### "Database locked" errors

- Ensure only one Sluice instance is running per data directory
- Check for stale lock files in data directory

### Slow publish performance

- Check disk I/O (fsync is the bottleneck)
- Monitor `sluice_storage_batch_size` metric — low values indicate underutilization

### Consumer not receiving messages

- Verify credits were granted via `CreditGrant` message
- Check `sluice_backpressure_active` metric
- Ensure `SubscriptionInit` was sent as first message
