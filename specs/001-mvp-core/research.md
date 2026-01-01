# Research: Sluice MVP (v0.1)

**Feature**: 001-mvp-core  
**Date**: 2026-01-01  
**Status**: Complete

## Research Tasks

This document resolves all technical unknowns identified during planning.

---

## 1. SQLite WAL Mode with synchronous=FULL Performance

**Question**: Can SQLite in WAL mode with `synchronous=FULL` achieve 5,000+ msg/s?

**Decision**: Yes, with Group Commit (batching)

**Rationale**:

- Individual `fsync` operations on SSD typically take 1-5ms, limiting naive implementation to ~200-1000 TPS
- Group Commit amortizes single `fsync` across batch of 50-100 messages
- With 5ms fsync and batch size of 100: theoretical 20,000 TPS
- Real-world expectation with overhead: 5,000-10,000 msg/s achievable

**Alternatives Considered**:

- `synchronous=NORMAL`: Faster but does NOT survive power failure in WAL mode (violates durability requirement)
- RocksDB/LMDB: Higher complexity, violates "SQLite-grade" simplicity goal
- Memory-mapped files: Custom durability logic, higher risk

**Implementation Notes**:

```rust
// Required SQLite pragmas
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA cache_size = -2000;  // ~2MB
PRAGMA temp_store = MEMORY;
```

---

## 2. Dedicated Writer Thread Pattern in Tokio

**Question**: How to safely use synchronous rusqlite with async Tokio runtime?

**Decision**: Spawn dedicated `std::thread` for writes; communicate via `tokio::sync::mpsc`

**Rationale**:

- `rusqlite` is synchronous and blocking; calling from Tokio task blocks executor
- `spawn_blocking` creates new thread per call (expensive, no batching)
- Dedicated thread owns single write connection, enables group commit
- `mpsc` channel provides backpressure when writer is saturated

**Alternatives Considered**:

- `tokio::task::spawn_blocking`: No batching opportunity, thread pool churn
- `sqlx` with async SQLite: Experimental, less mature than rusqlite
- Multiple writer threads: SQLite only supports one writer (SQLITE_BUSY)

**Implementation Notes**:

```rust
// Writer thread pattern
let (tx, rx) = tokio::sync::mpsc::channel::<WriteCommand>(1000);
std::thread::spawn(move || {
    let conn = Connection::open("sluice.db").unwrap();
    // Apply pragmas...
    loop {
        let mut batch = Vec::with_capacity(100);
        // Blocking wait for first message
        if let Some(cmd) = rx.blocking_recv() {
            batch.push(cmd);
            // Non-blocking drain up to limit
            while batch.len() < 100 {
                match rx.try_recv() {
                    Ok(cmd) => batch.push(cmd),
                    Err(_) => break,
                }
            }
            // Execute batch in single transaction
            let tx = conn.transaction().unwrap();
            for cmd in &batch { /* insert */ }
            tx.commit().unwrap();  // Single fsync!
            // Notify waiters
            for cmd in batch { cmd.reply.send(Ok(id)); }
        }
    }
});
```

---

## 3. tokio::sync::broadcast for Notification Bus

**Question**: How to efficiently wake sleeping subscriptions when new data arrives?

**Decision**: Use `tokio::sync::broadcast` channel for pub-sub notifications

**Rationale**:

- Subscriptions sleep when no credits or no data; need reactive wake-up
- Polling database burns CPU; update_hook is thread-local and complex
- Broadcast channel provides multi-consumer pub-sub with bounded capacity
- Lightweight notification (topic_id, max_seq) triggers targeted fetch

**Alternatives Considered**:

- Database polling with interval: CPU waste, latency penalty
- SQLite `update_hook`: Thread-affinity issues, complex async bridging
- Condition variables: Lower-level, harder to integrate with Tokio

**Implementation Notes**:

```rust
struct NewDataNotification {
    topic_id: u32,
    max_seq: u64,
}

// In writer thread after commit:
let _ = notify_tx.send(NewDataNotification { topic_id, max_seq });

// In subscription task:
loop {
    tokio::select! {
        Ok(notif) = notify_rx.recv() => {
            if notif.topic_id == self.topic_id && self.credits > 0 {
                self.fetch_and_deliver().await;
            }
        }
        // ... handle upstream messages
    }
}
```

---

## 4. Credit-Based Flow Control Implementation

**Question**: How to implement credit accounting safely in concurrent streaming?

**Decision**: Per-subscription `AtomicU32` for credits; split inbound/outbound handlers

**Rationale**:

- Credits must be updated by inbound handler (CreditGrant) and read by outbound (delivery)
- Atomics avoid mutex contention on hot path
- Split handlers prevent deadlock (inbound never blocks on outbound)

**Alternatives Considered**:

- Mutex-protected counter: Contention under high throughput
- Channel-based credit tokens: Complex, higher overhead
- Single handler with select!: Risk of head-of-line blocking

**Implementation Notes**:

```rust
struct SubscriptionState {
    credits: AtomicU32,
    cursor: AtomicU64,
    topic_id: u32,
}

// Inbound handler
fn handle_credit_grant(&self, grant: CreditGrant) {
    self.credits.fetch_add(grant.credits, Ordering::SeqCst);
}

// Outbound handler
async fn try_deliver(&self, msg: MessageDelivery) -> bool {
    loop {
        let current = self.credits.load(Ordering::SeqCst);
        if current == 0 { return false; }
        if self.credits.compare_exchange(
            current, current - 1,
            Ordering::SeqCst, Ordering::SeqCst
        ).is_ok() {
            return true;
        }
    }
}
```

---

## 5. UUIDv7 Generation in Rust

**Question**: Which crate provides UUIDv7 (time-sortable) generation?

**Decision**: Use `uuid` crate v1.7+ with `v7` feature

**Rationale**:

- `uuid` is the canonical UUID crate for Rust
- Version 7 added in uuid 1.3.0; stable and well-tested
- Time-sortable property enables efficient range queries

**Alternatives Considered**:

- `ulid`: Similar properties but less ecosystem support
- Custom implementation: Unnecessary given mature crate
- UUIDv4: Not time-sortable, poor index locality

**Implementation Notes**:

```toml
[dependencies]
uuid = { version = "1.7", features = ["v7"] }
```

```rust
use uuid::Uuid;
let message_id = Uuid::now_v7().to_string();
```

---

## 6. gRPC Bidirectional Streaming with Tonic

**Question**: How to structure bidirectional stream handler in Tonic?

**Decision**: Return `impl Stream` for responses; receive `Streaming<T>` for requests

**Rationale**:

- Tonic provides `Streaming<T>` wrapper for inbound stream
- Response stream created via `tokio_stream::wrappers::ReceiverStream`
- Handler spawns background task to process inbound; returns response stream immediately

**Alternatives Considered**:

- Manual futures: Complex, error-prone
- Channels without ReceiverStream: Works but more boilerplate

**Implementation Notes**:

```rust
async fn subscribe(
    &self,
    request: Request<Streaming<SubscribeUpstream>>,
) -> Result<Response<Self::SubscribeStream>, Status> {
    let mut inbound = request.into_inner();
    let (tx, rx) = mpsc::channel(100);

    // Spawn task to handle bidirectional communication
    tokio::spawn(async move {
        while let Some(msg) = inbound.message().await? {
            // Handle init/credit/ack
        }
    });

    Ok(Response::new(ReceiverStream::new(rx)))
}
```

---

## 7. Connection Pooling for Readers

**Question**: How to handle concurrent read connections for subscriptions?

**Decision**: Use `r2d2` with `r2d2_sqlite` for read connection pool

**Rationale**:

- Multiple subscriptions need concurrent read access
- SQLite WAL mode allows concurrent readers
- r2d2 is mature, integrates with rusqlite

**Alternatives Considered**:

- Single shared connection: Contention bottleneck
- Connection per subscription: Resource exhaustion risk
- `deadpool-sqlite`: Less mature, async-first design may not help read path

**Implementation Notes**:

```toml
[dependencies]
r2d2 = "0.8"
r2d2_sqlite = "0.24"
```

```rust
let manager = SqliteConnectionManager::file("sluice.db")
    .with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY);
let pool = Pool::builder()
    .max_size(10)
    .build(manager)?;
```

---

## 8. Graceful Shutdown Pattern

**Question**: How to implement graceful shutdown with pending write flush?

**Decision**: Use `tokio::signal` + `CancellationToken` pattern

**Rationale**:

- Catch SIGTERM/SIGINT via `tokio::signal::unix::signal`
- Propagate cancellation via `tokio_util::sync::CancellationToken`
- Writer thread drains remaining channel messages before exit

**Alternatives Considered**:

- `ctrlc` crate: Less Tokio integration
- Global atomic flag: Harder to propagate to all tasks

**Implementation Notes**:

```rust
let token = CancellationToken::new();
let token_clone = token.clone();

tokio::spawn(async move {
    signal::ctrl_c().await.unwrap();
    token_clone.cancel();
});

// In main loop
tokio::select! {
    _ = token.cancelled() => {
        // Graceful shutdown: drain writer channel, close connections
    }
    // ... normal operation
}
```

---

## Summary

All technical unknowns have been resolved. Key decisions:

| Area           | Decision                                       |
| -------------- | ---------------------------------------------- |
| Durability     | SQLite WAL + `synchronous=FULL` + Group Commit |
| Concurrency    | Dedicated writer thread + read pool            |
| Flow Control   | AtomicU32 credits + split handlers             |
| Notifications  | tokio::sync::broadcast                         |
| IDs            | UUIDv7 via `uuid` crate                        |
| gRPC Streaming | Tonic + ReceiverStream pattern                 |
| Shutdown       | CancellationToken pattern                      |

**Next**: Proceed to Phase 1 (Data Model & Contracts)
