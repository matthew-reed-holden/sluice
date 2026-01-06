# Sluice

**A gRPC-native message broker with credit-based flow control.**

Sluice occupies the "middle way" between heavy cluster-native systems (Kafka) and lightweight ephemeral ones (Redis Pub/Sub). It provides durable message persistence with operational simplicity—a single binary backed by SQLite.

## Features

- **Durable Persistence**: SQLite with WAL mode and fsync for crash-safe message storage
- **Credit-Based Flow Control**: Application-level backpressure via gRPC bidirectional streaming
- **Single Binary**: No external dependencies, no cluster coordination
- **OpenTelemetry**: Built-in metrics and trace propagation
- **5,000+ msg/s**: Group commit batching for high throughput

## Project Structure

This is a Cargo workspace containing:

- **sluice-proto** - Protocol buffer definitions shared between client and server
- **sluice-client** - Lightweight client library (only 6 dependencies)
- **sluice-server** - Message broker server with SQLite persistence
- **sluicectl** - Command-line tool for publishing and subscribing
- **lazysluice** - Terminal UI for monitoring topics and messages

See [CLAUDE.md](CLAUDE.md) for detailed architecture documentation.

## Quick Start

### Build

```bash
# Build entire workspace
cargo build --workspace --release

# Or use the convenient alias
cargo build-release

# Build just the server
cargo build-server
```

### Run Server

```bash
# Start with defaults (port 50051, data in ./data)
cargo run-server

# Or run the binary directly
./target/release/sluice

# With custom configuration
cargo run-server -- --port 9000 --data-dir /var/lib/sluice --log-level debug
```

### Use CLI Tools

```bash
# Publish messages
cargo run-ctl -- publish my-topic "Hello, World!"

# Subscribe to messages
cargo run-ctl -- subscribe my-topic

# List topics
cargo run-ctl -- list-topics

# Launch TUI
cargo run-tui
```

### CLI Options

| Option            | Environment Variable          | Default   | Description                             |
| ----------------- | ----------------------------- | --------- | --------------------------------------- |
| `--host`          | `SLUICE_HOST`                 | `0.0.0.0` | Host address to bind                    |
| `--port`          | `SLUICE_PORT`                 | `50051`   | Port to listen on                       |
| `--data-dir`      | `SLUICE_DATA_DIR`             | `./data`  | Data directory for SQLite               |
| `--log-level`     | `RUST_LOG`                    | `info`    | Log level (trace/debug/info/warn/error) |
| `--otel-endpoint` | `OTEL_EXPORTER_OTLP_ENDPOINT` | (none)    | OTLP collector for metrics              |

### Graceful Shutdown

Send `SIGTERM` or `SIGINT` (Ctrl+C) to gracefully shutdown. Sluice will:

1. Stop accepting new connections
2. Flush pending writes to disk
3. Close existing connections
4. Exit cleanly

## API

Sluice uses gRPC with Protocol Buffers. See [crates/sluice-proto/proto/sluice/v1/sluice.proto](crates/sluice-proto/proto/sluice/v1/sluice.proto) for the full service definition.

### Publish

```protobuf
rpc Publish(PublishRequest) returns (PublishResponse);
```

Publishes a message to a topic. Topics are auto-created on first publish.

### Subscribe

```protobuf
rpc Subscribe(stream SubscribeUpstream) returns (stream SubscribeDownstream);
```

Bidirectional streaming for message consumption with credit-based flow control:

1. Send `SubscriptionInit` with topic, consumer_group, and initial_position
2. Send `CreditGrant` to allow message delivery
3. Receive `MessageDelivery` as messages become available
4. Send `Ack` to acknowledge processed messages

## Client Library

The `sluice-client` crate provides a high-level Rust client:

```rust
use sluice_client::{SluiceClient, ConnectConfig, InitialPosition};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ConnectConfig::plaintext("http://localhost:50051");
    let mut client = SluiceClient::connect(config).await?;

    // Publish
    client.publish("my-topic", b"Hello!".to_vec()).await?;

    // Subscribe
    let mut sub = client
        .subscribe("my-topic", Some("group"), None, InitialPosition::Earliest, 10)
        .await?;

    while let Some(msg) = sub.next_message().await? {
        println!("Received: {:?}", msg.payload);
        sub.send_ack(&msg.message_id).await?;
        sub.maybe_refill_credits().await?;
    }

    Ok(())
}
```

See [crates/sluice-client/README.md](crates/sluice-client/README.md) for complete documentation and [examples](crates/sluice-client/examples/).

## Metrics

When `--otel-endpoint` is configured, the following metrics are exported:

| Metric                           | Type      | Description                             |
| -------------------------------- | --------- | --------------------------------------- |
| `sluice_publish_total`           | Counter   | Total publish operations                |
| `sluice_publish_latency_seconds` | Histogram | Publish latency (request to fsync)      |
| `sluice_subscription_lag`        | Gauge     | Consumer lag (max_seq - cursor)         |
| `sluice_backpressure_active`     | Gauge     | 1 if consumer has 0 credits and lag > 0 |

## Architecture

```
┌─────────────┐     gRPC      ┌─────────────┐
│  Producer   │──────────────▶│   Sluice    │
└─────────────┘               │             │
                              │ ┌─────────┐ │
┌─────────────┐     gRPC      │ │ Writer  │ │──▶ SQLite (WAL)
│  Consumer   │◀─────────────▶│ │ Thread  │ │
└─────────────┘  Bidirectional│ └─────────┘ │
                 Streaming    │ ┌─────────┐ │
                              │ │ Reader  │ │
                              │ │  Pool   │ │
                              │ └─────────┘ │
                              └─────────────┘
```

- **Writer Thread**: Single dedicated thread for all writes, enabling group commit
- **Reader Pool**: r2d2 connection pool for concurrent reads
- **Notification Bus**: tokio::sync::broadcast for new message alerts

## Design Philosophy

The landscape of distributed messaging is bifurcated. On one end lie heavy, cluster-native systems like Apache Kafka, designed for massive throughput but demanding significant operational overhead. On the other end are lightweight, ephemeral systems like NATS Core or Redis Pub/Sub, which offer simplicity but sacrifice durability.

Sluice targets the architectural gap for a broker that offers the operational simplicity of a single binary while providing "good enough" durability and performance for the 90% of use cases that don't require petabyte-scale ingestion.

The defining characteristic of Sluice is its approach to **Application-Level Backpressure**. By mandating explicit flow control via gRPC bidirectional streaming, Sluice shifts the burden of queue management from the broker's memory to the producer's capability, ensuring system stability under varying load conditions.

## License

MIT
