# Sluice Improvement Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring Sluice from MVP to production quality across correctness, API stability, performance, observability, and developer experience.

**Architecture:** Phased approach—fix correctness bugs first, then build CI/CD foundation, stabilize the API (breaking changes are allowed pre-1.0), improve performance/observability, and polish tooling.

**Tech Stack:** Rust, tonic/prost (gRPC), rusqlite (SQLite), tokio, opentelemetry, ratatui, clap, criterion

---

## Phase 0: Critical Bug Fix

### Task 0.1: Fix writer thread commit-before-reply

**Files:**
- Modify: `crates/sluice-server/src/storage/writer.rs:426-466`
- Test: `crates/sluice-server/tests/integration_test.rs`

**Problem:** `flush_batch()` sends `Ok(result)` to callers at line 461 *before* `tx.commit()` at line 465. If commit fails, producers receive false durability confirmation, violating the core "returns only after fsync" promise.

**Fix:** Collect `(PublishCommand.reply, PublishResult)` pairs in a Vec during the loop. After `tx.commit()` succeeds, iterate and send `Ok(result)`. On commit failure, send `Err(WriterError::Database(...))` to all callers.

**Verification:** Run `cargo test-all`. Existing `test_data_survives_server_restart` must pass.

---

## Phase 1: Foundation

### Task 1.1: GitHub Actions CI Pipeline

**Files:**
- Create: `.github/workflows/ci.yml`

**Steps:**
1. Create workflow triggered on push to `main` and PRs
2. Jobs: `fmt` (`cargo fmt --check`), `clippy` (`cargo clippy --workspace --all-targets -- -D warnings`), `test` (`cargo test --workspace`), `audit` (`cargo audit`)
3. Use `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`
4. Cache `~/.cargo` and `target/` directories

### Task 1.2: Delete orphan src/ directory

**Files:**
- Delete: `src/` (25 files — pre-refactor code at workspace root)

This is leftover code from before the workspace refactor. It creates confusion and has no `Cargo.toml` pointing at it.

### Task 1.3: Add cargo-deny configuration

**Files:**
- Create: `deny.toml`
- Modify: `.github/workflows/ci.yml` (add `cargo deny check` step)

Configure license allowlist (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-DFS-2016) and advisory database checking. Deny copyleft licenses.

### Task 1.4: Add LICENSE files

**Files:**
- Create: `LICENSE-MIT`
- Create: `LICENSE-APACHE`

Match the `license = "MIT OR Apache-2.0"` declaration in `Cargo.toml`.

### Task 1.5: Add rust-toolchain.toml

**Files:**
- Create: `rust-toolchain.toml`

Pin stable channel. Non-Nix contributors currently have no pinned toolchain. MSRV in `clippy.toml` is 1.75.0.

### Task 1.6: Add Dockerfile

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

Multi-stage build:
- Stage 1: `rust:1.82-slim-bookworm` builder, install protobuf-compiler, build release binary
- Stage 2: `debian:bookworm-slim` runtime, copy binary, expose 50051 (gRPC) + 9090 (metrics), volume at `/var/lib/sluice`

---

## Phase 2: API Stabilization

### Task 2.1: Replace anyhow with typed errors in sluice-client

**Files:**
- Modify: `crates/sluice-client/src/lib.rs`
- Create: `crates/sluice-client/src/error.rs`
- Modify: `crates/sluice-client/src/connection.rs`
- Modify: `crates/sluice-client/src/subscription.rs`
- Modify: `crates/sluice-client/Cargo.toml` (remove `anyhow` dependency)

Create `SluiceError` enum with `thiserror`:
```rust
#[derive(Debug, Error)]
pub enum SluiceError {
    #[error("connection failed to {endpoint}: {source}")]
    ConnectionFailed { endpoint: String, source: tonic::transport::Error },

    #[error("{method} RPC failed: {source}")]
    RpcFailed { method: &'static str, source: tonic::Status },

    #[error("subscription stream closed")]
    SubscriptionClosed,

    #[error("invalid configuration: {message}")]
    InvalidConfig { message: String },

    #[error("channel closed")]
    ChannelClosed,
}
```

Replace all `anyhow::Result<T>` with `Result<T, SluiceError>` across the client crate. Update examples to use the new error type.

### Task 2.2: Fix next_message() heartbeat handling

**Files:**
- Modify: `crates/sluice-client/src/subscription.rs:176-190`

Change `next_message()` to loop internally when it receives a heartbeat:
```rust
pub async fn next_message(&mut self) -> Result<Option<MessageDelivery>> {
    loop {
        match self.rx.next().await {
            Some(Ok(downstream)) => match downstream.response {
                Some(subscribe_downstream::Response::Delivery(msg)) => {
                    self.consume_credit();
                    return Ok(Some(msg));
                }
                Some(subscribe_downstream::Response::Heartbeat(_)) => continue,
                None => continue,
            },
            Some(Err(e)) => return Err(e.into()),
            None => return Ok(None),
        }
    }
}
```

Add `next_event()` method returning `SubscriptionEvent::Message(MessageDelivery) | SubscriptionEvent::Heartbeat(Heartbeat)` for callers who want heartbeat visibility.

### Task 2.3: Add publish_with_attributes() to client

**Files:**
- Modify: `crates/sluice-client/src/connection.rs:240-260`
- Modify: `crates/sluice-client/README.md` (fix documentation for non-existent `publish_request()`)

Add:
```rust
pub async fn publish_with_attributes(
    &mut self,
    topic: &str,
    payload: Vec<u8>,
    attributes: HashMap<String, String>,
) -> Result<PublishResponse> { ... }
```

Update existing `publish()` to delegate to `publish_with_attributes()` with empty map.

### Task 2.4: Add admin RPCs to proto and server

**Files:**
- Modify: `crates/sluice-proto/proto/sluice/v1/sluice.proto`
- Modify: `crates/sluice-server/src/storage/schema.rs`
- Create: `crates/sluice-server/src/service/admin.rs`
- Modify: `crates/sluice-server/src/service/mod.rs`
- Modify: `crates/sluice-client/src/connection.rs`

Add RPCs:
```protobuf
rpc DeleteTopic(DeleteTopicRequest) returns (DeleteTopicResponse) {}
rpc DeleteConsumerGroup(DeleteConsumerGroupRequest) returns (DeleteConsumerGroupResponse) {}
rpc ResetCursor(ResetCursorRequest) returns (ResetCursorResponse) {}
```

Implement:
- `delete_topic`: DELETE FROM messages WHERE topic_id=?; DELETE FROM subscriptions WHERE topic_id=?; DELETE FROM topics WHERE topic_id=?
- `delete_consumer_group`: DELETE FROM subscriptions WHERE topic_id=? AND consumer_group=?
- `reset_cursor`: UPDATE subscriptions SET cursor_seq=? WHERE topic_id=? AND consumer_group=?

Add integration tests for each operation. Add client methods. Wire into `SluiceService`.

### Task 2.5: Add Nack, BatchAck, and delivery_count to Subscribe protocol

**Files:**
- Modify: `crates/sluice-proto/proto/sluice/v1/sluice.proto`
- Modify: `crates/sluice-server/src/service/subscribe.rs`
- Modify: `crates/sluice-server/src/storage/schema.rs`
- Modify: `crates/sluice-client/src/subscription.rs`

Proto changes:
```protobuf
message SubscribeUpstream {
  oneof request {
    SubscriptionInit init = 1;
    CreditGrant credit = 2;
    Ack ack = 3;
    Nack nack = 4;        // NEW
    BatchAck batch_ack = 5; // NEW
  }
}

message Nack {
  string message_id = 1;
  uint32 redelivery_delay_ms = 2; // 0 = immediate redelivery
}

message BatchAck {
  repeated string message_ids = 1;
}

message MessageDelivery {
  string message_id = 1;
  uint64 sequence = 2;
  bytes payload = 3;
  map<string, string> attributes = 4;
  int64 timestamp = 5;
  uint32 delivery_count = 6; // NEW: starts at 1
}

message SubscribeDownstream {
  oneof response {
    MessageDelivery delivery = 1;
    Heartbeat heartbeat = 2;
    StreamError error = 3; // NEW
  }
}

message StreamError {
  string code = 1;    // e.g. "TOPIC_DELETED", "SLOW_CONSUMER"
  string message = 2;
}
```

Server: Track delivery counts in a new `delivery_attempts` column on messages table (or an in-memory map for MVP). Handle Nack by resetting cursor. Handle BatchAck by advancing cursor to max sequence. Add client methods `send_nack()` and `send_batch_ack()`.

---

## Phase 3: Performance & Observability

### Task 3.1: Fix Prometheus/OTel metrics bridge

**Files:**
- Modify: `crates/sluice-server/src/observability/metrics.rs:116-175`
- Modify: `crates/sluice-server/src/observability/prometheus.rs`
- Modify: `crates/sluice-server/src/service/subscribe.rs` (wire missing metrics)
- Modify: `crates/sluice-server/src/service/batch_publish.rs` (wire `record_batch_publish`)
- Modify: `crates/sluice-server/Cargo.toml` (add `opentelemetry-prometheus` dependency)

Replace disconnected `prometheus::Registry::new()` with `opentelemetry_prometheus::exporter().build()` which creates a meter provider that also populates a Prometheus registry. This makes `/metrics` serve actual data.

Wire unused recording functions:
- `record_batch_publish()` in `batch_publish.rs` handler
- `record_message_delivered()` in `subscribe.rs` delivery loop
- `record_ack()` in `subscribe.rs` ack handler
- `record_credits_granted()` in `subscribe.rs` credit handler
- `record_subscription_active()` on subscription start/end

### Task 3.2: Fix writer thread polling loop

**Files:**
- Modify: `crates/sluice-server/src/storage/writer.rs:300-400` (main loop)

Replace `std::thread::sleep(Duration::from_millis(1))` polling with `recv_timeout(batch_config.max_delay)`. When a message arrives, continue accumulating until batch is full or timeout expires. This eliminates wasted CPU cycles and reduces first-message latency.

### Task 3.3: Relax atomic orderings in CreditBalance

**Files:**
- Modify: `crates/sluice-server/src/flow/credit.rs`

Change all `Ordering::SeqCst` to `Ordering::AcqRel` (for read-modify-write ops) or `Ordering::Relaxed` (for simple loads/stores that don't need ordering). Credit tracking doesn't require total sequential consistency.

### Task 3.4: Add criterion benchmarks

**Files:**
- Create: `crates/sluice-server/benches/publish_bench.rs`
- Create: `crates/sluice-server/benches/subscribe_bench.rs`
- Modify: `crates/sluice-server/Cargo.toml` (add `[[bench]]` sections + criterion dev-dependency)

Benchmark:
- Single publish throughput (varying payload sizes: 64B, 1KB, 64KB)
- Batch publish throughput (batch sizes: 10, 100, 1000)
- End-to-end publish-subscribe latency
- Raw SQLite write performance (to isolate gRPC overhead)

### Task 3.5: Make health/ready endpoints functional

**Files:**
- Modify: `crates/sluice-server/src/observability/prometheus.rs:68-75`
- Modify: `crates/sluice-server/src/server.rs` (pass shared state to Prometheus server)

Health endpoint:
- Ping SQLite database (attempt a trivial SELECT 1)
- Check writer thread is alive (via a liveness flag or channel probe)
- Return 503 if either check fails

Ready endpoint:
- Verify schema is initialized (topics table exists)
- Verify writer thread is accepting commands
- Return 503 if not ready

---

## Phase 4: TUI & CLI Polish

### Task 4.1: Fix lazysluice search filter rendering

**Files:**
- Modify: `crates/lazysluice/src/ui.rs` (`draw_tail()` function)

Change `draw_tail()` to use `state.visible_messages()` instead of `state.messages` directly. The search infrastructure already computes `filtered_indices` — the renderer just needs to use it.

### Task 4.2: Fix lazysluice viewport scrolling and resize

**Files:**
- Modify: `crates/lazysluice/src/app.rs` (add `ListState` tracking)
- Modify: `crates/lazysluice/src/ui.rs` (use `ListState` for rendering)
- Modify: `crates/lazysluice/src/events.rs:44` (handle resize events)

Add `ratatui::widgets::ListState` with proper offset tracking. When the cursor moves beyond visible bounds, adjust the offset. Handle `Event::Resize` to trigger re-render.

### Task 4.3: Fix q key behavior in text input modes

**Files:**
- Modify: `crates/lazysluice/src/controller.rs` (key event handlers for input screens)

In `Screen::Publish`, `Screen::CreateTopic`, `Screen::ConsumerGroupInput`, and active search mode: route `KeyCode::Char('q')` to the text input buffer, not to the quit handler. Only `Esc` should cancel/go back from these screens.

### Task 4.4: Expand sluicectl commands

**Files:**
- Modify: `crates/sluicectl/src/main.rs` (add subcommands)
- Create: `crates/sluicectl/src/stats.rs`
- Modify: `crates/sluicectl/src/subscribe.rs` (add --browse, --offset flags)
- Modify: `crates/sluicectl/src/topics.rs` (add delete subcommand after Task 2.4)
- Modify: `crates/sluicectl/Cargo.toml` (add `chrono` for timestamp formatting)

New commands:
- `topics stats [topic...]` — display topic statistics with message counts, consumer group lag
- `subscribe --browse` — non-destructive subscription using `subscribe_browse()`
- `subscribe --offset N` — start from specific sequence number

Fix timestamp formatting: replace raw epoch display with human-readable format using `chrono::DateTime::from_timestamp_millis()`.

---

## Dependency Graph

```
Phase 0 (bug fix) -> no dependencies
Phase 1 (foundation) -> no dependencies, all tasks independent
Phase 2.1-2.3 -> independent of each other
Phase 2.4 -> depends on Phase 1 CI for test coverage
Phase 2.5 -> depends on 2.4 (admin RPCs inform Nack design)
Phase 3.1 -> independent
Phase 3.2-3.3 -> independent
Phase 3.4 -> should come after 3.2 (benchmark after optimization)
Phase 3.5 -> independent
Phase 4.1-4.3 -> independent of each other
Phase 4.4 -> depends on 2.4 (needs DeleteTopic RPC for `topics delete`)
```

## Verification

After each phase, run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

After Phase 3.4, run benchmarks:
```bash
cargo bench -p sluice-server
```
