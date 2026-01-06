# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Sluice is a gRPC-native message broker with credit-based flow control. It occupies the "middle way" between heavy cluster-native systems (Kafka) and lightweight ephemeral ones (Redis Pub/Sub), providing durable SQLite-backed message persistence with operational simplicity.

## Build and Test Commands

### Building

The workspace includes convenient build aliases in `.cargo/config.toml`:

```bash
# Build entire workspace
cargo build --workspace
cargo build-all  # alias

# Build release binaries
cargo build --workspace --release
cargo build-release  # alias

# Build specific components
cargo build-server   # Build server binary (release mode)
cargo build-client   # Build client library
cargo build-tools    # Build sluicectl + lazysluice
```

### Running

```bash
# Run server with defaults (port 50051, data in ./data)
cargo run -p sluice-server
cargo run-server  # alias

# Run with custom configuration
cargo run-server --port 9000 --data-dir /var/lib/sluice --log-level debug

# Run sluicectl CLI
cargo run -p sluicectl -- --help
cargo run-ctl -- --help  # alias

# Run lazysluice TUI
cargo run -p lazysluice
cargo run-tui  # alias

# Run client examples
cargo run -p sluice-client --example simple_publish
cargo run -p sluice-client --example subscribe_and_ack
```

### Testing

```bash
# Run all tests across workspace
cargo test --workspace
cargo test-all  # alias

# Run tests for specific packages
cargo test-server        # Server tests only
cargo test-client        # Client tests only
cargo test-integration   # Integration tests

# Run specific test file
cargo test -p sluice-server --test integration_test

# Run specific test by name
cargo test test_publish_and_subscribe

# Run with output visible
cargo test -- --nocapture
```

### Development

```bash
# Check all code without building
cargo check --workspace --all-targets
cargo check-all  # alias

# Run clippy lints
cargo clippy --workspace --all-targets
cargo clippy-all  # alias

# Auto-fix clippy warnings
cargo clippy-fix  # alias

# Format code
cargo fmt

# Generate documentation
cargo doc-all      # Open workspace docs
cargo doc-private  # Include private items
```

## Architecture Overview

### Core Threading Model

Sluice uses a **dedicated writer thread pattern** for SQLite operations:
- Single `std::thread` owns the write connection (crates/sluice-server/src/storage/writer.rs)
- Communication via `tokio::sync::mpsc` channel for publish commands
- Enables group commit batching for 5,000+ msg/s throughput
- Reader pool uses `r2d2` for concurrent read operations (crates/sluice-server/src/storage/reader.rs)

### Workspace Structure

This is a Cargo workspace with **5 crates** providing clear separation of concerns:

```
sluice/
├── crates/
│   ├── sluice-proto/       # Protocol buffer definitions
│   │   ├── proto/          # .proto files
│   │   ├── build.rs        # Code generation
│   │   └── src/lib.rs      # Generated gRPC/protobuf code
│   │
│   ├── sluice-client/      # Lightweight client library (6 dependencies)
│   │   ├── src/
│   │   │   ├── connection.rs   # gRPC connection management
│   │   │   └── subscription.rs # Subscription lifecycle
│   │   ├── examples/       # Usage examples
│   │   └── README.md
│   │
│   ├── sluice-server/      # Message broker server
│   │   ├── src/
│   │   │   ├── storage/    # SQLite persistence
│   │   │   ├── service/    # gRPC RPC handlers
│   │   │   ├── flow/       # Credit tracking & notification
│   │   │   ├── observability/  # Metrics & tracing
│   │   │   ├── config.rs   # CLI configuration
│   │   │   └── server.rs   # Server setup
│   │   ├── tests/          # Integration tests
│   │   └── README.md
│   │
│   ├── sluicectl/          # CLI tool (uses sluice-client)
│   │   └── src/
│   │       ├── main.rs
│   │       ├── publish.rs
│   │       ├── subscribe.rs
│   │       └── topics.rs
│   │
│   └── lazysluice/         # TUI client (uses sluice-client)
│       └── src/
│           ├── main.rs
│           ├── app.rs
│           └── controller.rs
│
├── Cargo.toml              # Workspace manifest with shared dependencies
└── .cargo/config.toml      # Build aliases
```

### Component Organization (sluice-server)

```
crates/sluice-server/src/
├── storage/        # SQLite persistence layer
│   ├── writer.rs   # Dedicated writer thread with group commit
│   ├── reader.rs   # r2d2 connection pool for reads
│   ├── batch.rs    # Group commit batching logic
│   └── schema.rs   # Schema initialization and queries
├── service/        # gRPC service handlers
│   ├── publish.rs  # Unary Publish RPC handler
│   ├── subscribe.rs # Bidirectional Subscribe RPC handler
│   ├── topics.rs   # ListTopics RPC handler
│   └── registry.rs # Active subscription tracking
├── flow/           # Flow control infrastructure
│   ├── credit.rs   # Credit-based backpressure tracking
│   └── notify.rs   # tokio::broadcast notification bus
├── observability/  # Metrics and tracing
│   ├── metrics.rs  # OpenTelemetry metrics setup
│   └── tracing.rs  # tracing-subscriber configuration
├── config.rs       # CLI and environment configuration
├── server.rs       # gRPC server setup and ServerState
└── lib.rs          # Public API exports
```

### Key Data Flow

1. **Publish Path**: gRPC request → service/publish.rs → mpsc channel → writer thread → group commit → SQLite WAL → notify bus
2. **Subscribe Path**: gRPC stream → service/subscribe.rs → reader pool → credit tracking → message delivery → ack → cursor update
3. **Notification**: Writer thread → broadcast channel → sleeping subscriptions → wake and poll

### Protocol Buffers

The gRPC API is defined in `crates/sluice-proto/proto/sluice/v1/sluice.proto`:
- **Publish**: Unary RPC that returns after fsync (durability guarantee)
- **Subscribe**: Bidirectional stream with credit-based flow control
  - Client sends: `SubscriptionInit`, `CreditGrant`, `Ack`
  - Server sends: `MessageDelivery`
- **ListTopics**: Unary RPC for topic discovery

Generated code is in `crates/sluice-proto/src/lib.rs` via build.rs.
The proto crate is shared by both client and server.

### Storage Schema

SQLite with WAL mode, synchronous=FULL for crash safety:
- `topics` table: topic_id, name, created_at
- `messages` table: id (UUIDv7), topic_id, sequence, payload, attributes, timestamp
- `subscriptions` table: topic_id, consumer_group, cursor_seq (tracks read position)

Auto-created topics on first publish.

### Configuration

Server configuration via CLI args or environment variables (crates/sluice-server/src/config.rs):
- `--host` / `SLUICE_HOST`: Bind address (default: 0.0.0.0)
- `--port` / `SLUICE_PORT`: Server port (default: 50051)
- `--data-dir` / `SLUICE_DATA_DIR`: SQLite database location (default: ./data)
- `--log-level` / `RUST_LOG`: Logging verbosity (default: info)
- `--write-channel-size` / `SLUICE_WRITE_CHANNEL_SIZE`: Write backpressure buffer (default: 1000)
- `--reader-pool-size` / `SLUICE_READER_POOL_SIZE`: Reader connection pool (default: 10)
- `--otel-endpoint` / `OTEL_EXPORTER_OTLP_ENDPOINT`: OpenTelemetry collector (optional)

### Client Usage

The `sluice-client` crate provides a lightweight client library:

```rust
use sluice_client::{SluiceClient, ConnectConfig, InitialPosition};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect and publish
    let config = ConnectConfig::plaintext("http://localhost:50051");
    let mut client = SluiceClient::connect(config).await?;

    let response = client.publish("my-topic", b"payload".to_vec()).await?;
    println!("Published: {}", response.message_id);

    // Subscribe with credit-based flow control
    let mut subscription = client
        .subscribe(
            "my-topic",
            Some("my-group"),      // consumer group
            None,                   // auto-generate subscription ID
            InitialPosition::Earliest,
            10                      // initial credits
        )
        .await?;

    while let Some(msg) = subscription.next_message().await? {
        println!("Received: {:?}", msg.payload);
        subscription.send_ack(&msg.message_id).await?;
        subscription.maybe_refill_credits().await?;
    }

    Ok(())
}
```

**See also**: `crates/sluice-client/examples/` for complete working examples.

### Testing Infrastructure

Integration tests in `crates/sluice-server/tests/` use temporary SQLite databases and spawn servers on random ports. See `crates/sluice-server/tests/common/mod.rs` for shared test utilities.

## Development Notes

### Workspace Benefits

The refactored multi-crate structure provides:

1. **Clear Separation of Concerns**
   - `sluice-proto`: Single source of truth for protocol definitions
   - `sluice-client`: Lightweight library (6 deps) for client applications
   - `sluice-server`: Full server with storage, observability, etc.
   - `sluicectl` / `lazysluice`: Client tools with minimal dependencies

2. **Faster Builds**
   - Client tools only compile what they need (~66% faster)
   - Workspace caching shares common dependencies
   - Smaller binaries (client tools: 4-6MB vs 10MB monolithic)

3. **Independent Versioning**
   - Each crate can be published separately to crates.io
   - Client and server can evolve at different rates
   - Proto changes trigger rebuild of both, others remain cached

4. **Workspace Dependencies**
   - Centralized version management in root `Cargo.toml`
   - Consistent dependency versions across all crates
   - Easy to update shared dependencies in one place

### Clippy Configuration

The codebase allows several clippy lints (see crates/sluice-server/src/lib.rs):
- `module_name_repetitions`: `service::publish::PublishService` is acceptable
- `needless_raw_string_hashes`: SQL queries use raw strings
- `similar_names`: `seq`, `seq_no`, `sequence` are domain terms
- `too_many_lines`: Some functions (like RPC handlers) are inherently long

### UUIDv7 for Message IDs

Uses time-sortable UUIDv7 (uuid crate with v7 feature) for natural ordering and efficient range queries.
Message IDs are generated via `sluice_server::generate_message_id()`.

### Graceful Shutdown

Server handles SIGTERM/SIGINT by:
1. Stopping new connections
2. Flushing writer thread
3. Closing existing streams
4. Exiting cleanly

See crates/sluice-server/src/main.rs signal handling.

### Import Conventions

When working with the codebase, use these import patterns:

```rust
// In sluice-server code
use sluice_server::config::Config;
use sluice_server::storage::reader::ReaderPool;
use sluice_proto::sluice::v1::*;

// In client applications (sluicectl, lazysluice, etc.)
use sluice_client::{SluiceClient, ConnectConfig, InitialPosition};
use sluice_proto::{MessageDelivery, Topic};

// In tests
use sluice_server::config::Config;
use sluice_client::SluiceClient;
```
