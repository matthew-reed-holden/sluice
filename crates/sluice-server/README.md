# sluice-server

A gRPC-native message broker with credit-based flow control and SQLite persistence.

## Overview

Sluice Server is a high-performance message broker that provides:
- **At-Least-Once delivery** semantics with durable persistence
- **Credit-based flow control** to prevent overwhelming consumers
- **gRPC-native** communication via tonic/prost
- **SQLite WAL** for crash-safe message storage
- **Group commit batching** achieving 5,000+ msg/s throughput
- **OpenTelemetry** integration for metrics and tracing

## Features

- **Durable Messaging**: SQLite with `synchronous=FULL` ensures messages survive crashes
- **Consumer Groups**: Multiple consumers can share workload with automatic load balancing
- **Message Acknowledgments**: Track delivery and redelivery of messages
- **Flexible Positioning**: Start from earliest, latest, or specific message offset
- **Observability**: Built-in metrics and distributed tracing via OpenTelemetry
- **UUIDv7 Message IDs**: Time-sortable IDs for efficient range queries
- **Backpressure**: Credit-based flow control prevents consumer overload

## Architecture

### Thread Model

```
┌─────────────────────┐
│   gRPC Handlers     │  (tokio async tasks)
│  - Publish RPC      │
│  - Subscribe RPC    │
│  - ListTopics RPC   │
└──────────┬──────────┘
           │
           ├─> Write Channel ──> Dedicated Writer Thread
           │                     - Group commit batching
           │                     - Single SQLite write connection
           │
           └─> Reader Pool (r2d2)
                - Concurrent reads
                - 10 connections default
                - Lock-free with WAL mode
```

### Storage Layer

- **SQLite WAL Mode**: Write-Ahead Logging for concurrent reads during writes
- **Three Tables**:
  - `topics`: Topic metadata and IDs
  - `messages`: Durable message storage with payload and attributes
  - `subscriptions`: Consumer position tracking and acknowledgments

### Flow Control

- **Credit System**: Consumers request messages by sending credits
- **Notification Bus**: `tokio::sync::broadcast` wakes sleeping subscriptions
- **Backpressure**: Server only delivers messages when consumer has credits

## Installation

### Building from Source

```bash
# Build server binary
cargo build -p sluice-server --release

# Or use the workspace alias
cargo build-server
```

### Running Tests

```bash
# Run all server tests
cargo test -p sluice-server

# Or use workspace alias
cargo test-server
```

## Running the Server

### Quick Start

```bash
# Run with defaults (port 50051, ./data directory)
cargo run -p sluice-server

# Or use the alias
cargo run-server
```

### With Configuration

```bash
# Specify port and data directory
cargo run -p sluice-server -- --port 9090 --data-dir /var/sluice/data

# Set log level
RUST_LOG=debug cargo run -p sluice-server

# Enable OpenTelemetry export
cargo run -p sluice-server -- --otel-endpoint http://localhost:4317
```

## Configuration

Configuration can be provided via CLI arguments or environment variables:

| CLI Argument             | Environment Variable        | Default        | Description                          |
|--------------------------|----------------------------|----------------|--------------------------------------|
| `--host`                 | `SLUICE_HOST`              | `0.0.0.0`      | Host address to bind to              |
| `--port`, `-p`           | `SLUICE_PORT`              | `50051`        | Port to listen on                    |
| `--data-dir`, `-d`       | `SLUICE_DATA_DIR`          | `./data`       | Directory for SQLite database        |
| `--log-level`            | `RUST_LOG`                 | `info`         | Log level (trace/debug/info/warn/error) |
| `--write-channel-size`   | `SLUICE_WRITE_CHANNEL_SIZE`| `1000`         | Write channel buffer size            |
| `--reader-pool-size`     | `SLUICE_READER_POOL_SIZE`  | `10`           | Number of reader connections         |
| `--notify-channel-size`  | `SLUICE_NOTIFY_CHANNEL_SIZE`| `1024`        | Notification broadcast buffer        |
| `--otel-endpoint`        | `OTEL_EXPORTER_OTLP_ENDPOINT`| None         | OpenTelemetry collector endpoint     |

### Example Configurations

**Development**:
```bash
cargo run -p sluice-server -- \
  --port 50051 \
  --data-dir ./dev-data \
  --log-level debug
```

**Production**:
```bash
RUST_LOG=info \
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317 \
./target/release/sluice \
  --host 0.0.0.0 \
  --port 50051 \
  --data-dir /var/lib/sluice \
  --reader-pool-size 20 \
  --write-channel-size 2000
```

## Observability

### Metrics

The server exports OpenTelemetry metrics including:
- Message publish rates
- Subscription active counts
- Credit flow metrics
- Storage operation latencies

View metrics by connecting to an OpenTelemetry collector:

```bash
cargo run -p sluice-server -- \
  --otel-endpoint http://localhost:4317
```

### Tracing

Distributed tracing is automatically enabled. Configure trace level via `RUST_LOG`:

```bash
RUST_LOG=sluice_server=trace cargo run -p sluice-server
```

### Logging

Structured logging via `tracing`:

```bash
# JSON output
RUST_LOG=info cargo run -p sluice-server

# Pretty output for development
RUST_LOG=debug cargo run -p sluice-server
```

## Persistence

### Database Schema

```sql
-- Topics table
CREATE TABLE topics (
    topic_id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL
);

-- Messages table
CREATE TABLE messages (
    message_id TEXT PRIMARY KEY,
    topic_id INTEGER NOT NULL,
    sequence_number INTEGER NOT NULL,
    payload BLOB NOT NULL,
    attributes_json TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (topic_id) REFERENCES topics(topic_id)
);

-- Subscriptions table (consumer position tracking)
CREATE TABLE subscriptions (
    subscription_id TEXT PRIMARY KEY,
    topic_id INTEGER NOT NULL,
    consumer_group TEXT,
    last_delivered_seq INTEGER,
    last_acked_seq INTEGER,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (topic_id) REFERENCES topics(topic_id)
);
```

### WAL Mode

Sluice uses SQLite's Write-Ahead Logging mode for:
- **Concurrent access**: Multiple readers during writes
- **Crash safety**: Atomic commits with `synchronous=FULL`
- **Performance**: Group commit batching in dedicated writer thread

### Backup

```bash
# Backup database while server is running
sqlite3 data/sluice.db ".backup data/backup.db"

# Or copy WAL files (server must be stopped)
cp data/sluice.db* /backup/location/
```

## Performance

### Benchmarks

- **Throughput**: 5,000+ msg/s on commodity hardware
- **Latency**: Sub-millisecond p50, <10ms p99 for local clients
- **Concurrency**: Supports hundreds of concurrent subscriptions

### Tuning

For higher throughput:

```bash
cargo run -p sluice-server -- \
  --write-channel-size 5000 \
  --reader-pool-size 20 \
  --notify-channel-size 2048
```

For lower latency:

```bash
cargo run -p sluice-server -- \
  --write-channel-size 100 \
  --reader-pool-size 5
```

## Testing

### Unit Tests

```bash
cargo test -p sluice-server
```

### Integration Tests

```bash
cargo test -p sluice-server --test integration_test
```

### End-to-End Testing

```bash
# Terminal 1: Start server
cargo run-server

# Terminal 2: Run sluicectl commands
cargo run -p sluicectl -- publish test-topic "hello world"
cargo run -p sluicectl -- subscribe test-topic
```

## Library Usage

The server can also be used as a library for embedding:

```rust
use sluice_server::config::Config;
use sluice_server::server::run_server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        host: "127.0.0.1".into(),
        port: 50051,
        data_dir: "./data".into(),
        ..Default::default()
    };

    run_server(config).await?;
    Ok(())
}
```

## Architecture Deep Dive

### Dedicated Writer Thread

A single `std::thread` owns the write connection to enable group commit batching:

```rust
// Batches multiple publishes into single transaction
let mut batch = Vec::new();
while let Ok(req) = rx.recv_timeout(Duration::from_millis(1)) {
    batch.push(req);
    // Drain up to 100 messages or timeout
    for _ in 0..99 {
        if let Ok(req) = rx.try_recv() {
            batch.push(req);
        } else {
            break;
        }
    }
    commit_batch(&batch)?;
}
```

### Notification Bus

The notification bus wakes sleeping subscriptions when new messages arrive:

```rust
// Publish triggers notification
notify_tx.send(TopicEvent::NewMessage { topic_id })?;

// Subscription listens for events
let mut notify_rx = notify_tx.subscribe();
loop {
    tokio::select! {
        event = notify_rx.recv() => {
            if event.topic_id == my_topic_id {
                fetch_new_messages().await?;
            }
        }
    }
}
```

## License

MIT OR Apache-2.0
