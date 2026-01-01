# Data Model: Sluice MVP (v0.1)

**Feature**: 001-mvp-core  
**Date**: 2026-01-01  
**Status**: Complete

## Overview

Sluice uses SQLite as its persistence layer with WAL mode for concurrent read/write access. The data model is intentionally minimal to support the MVP scope while enabling future extensibility.

---

## Entity Relationship Diagram

```
┌─────────────┐       ┌─────────────────┐       ┌───────────────────┐
│   topics    │       │    messages     │       │   subscriptions   │
├─────────────┤       ├─────────────────┤       ├───────────────────┤
│ id (PK)     │◄──────│ topic_id (FK)   │       │ id (PK)           │
│ name        │       │ global_seq (PK) │       │ topic_id (FK)     │──►│
│ created_at  │       │ message_id      │       │ consumer_group    │
└─────────────┘       │ payload         │       │ cursor_seq        │
                      │ attributes      │       │ updated_at        │
                      │ created_at      │       └───────────────────┘
                      └─────────────────┘
```

---

## Entities

### 1. Topic

A named stream of messages. Auto-created on first publish.

| Field        | Type    | Constraints               | Description                             |
| ------------ | ------- | ------------------------- | --------------------------------------- |
| `id`         | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal topic identifier               |
| `name`       | TEXT    | NOT NULL, UNIQUE          | User-facing topic name (e.g., "orders") |
| `created_at` | INTEGER | NOT NULL                  | Unix timestamp (ms) of creation         |

**Lifecycle**:

- Created: Implicitly on first `Publish` to a new topic name
- Updated: Never (immutable after creation)
- Deleted: Not supported in MVP (deferred to future version)

**Indexes**:

- Primary key on `id` (implicit)
- Unique index on `name` for fast lookup

### 2. Message

The unit of data flowing through Sluice.

| Field        | Type    | Constraints               | Description                              |
| ------------ | ------- | ------------------------- | ---------------------------------------- |
| `global_seq` | INTEGER | PRIMARY KEY AUTOINCREMENT | Global sequence number (strict ordering) |
| `topic_id`   | INTEGER | NOT NULL, FK → topics.id  | Parent topic                             |
| `message_id` | TEXT    | NOT NULL                  | UUIDv7 (time-sortable), server-generated |
| `payload`    | BLOB    |                           | Opaque message content                   |
| `attributes` | TEXT    |                           | JSON-encoded key-value headers           |
| `created_at` | INTEGER | NOT NULL                  | Unix timestamp (ms)                      |

**Lifecycle**:

- Created: On `Publish` RPC, after fsync confirmation
- Updated: Never (immutable after creation)
- Deleted: Not supported in MVP (retention policy deferred)

**Indexes**:

- Primary key on `global_seq` (B-tree append-only for write performance)
- Composite index on `(topic_id, global_seq)` for subscription seeking

**Notes**:

- `global_seq` is a global monotonic counter, not per-topic. This simplifies cursor tracking.
- `attributes` uses JSON encoding for flexibility; no schema enforcement in MVP.

### 3. Subscription

Tracks consumer group progress through a topic.

| Field            | Type    | Constraints               | Description                               |
| ---------------- | ------- | ------------------------- | ----------------------------------------- |
| `id`             | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal subscription identifier          |
| `topic_id`       | INTEGER | NOT NULL, FK → topics.id  | Target topic                              |
| `consumer_group` | TEXT    | NOT NULL                  | Consumer group name (e.g., "workers")     |
| `cursor_seq`     | INTEGER | NOT NULL, DEFAULT 0       | Last acknowledged global_seq              |
| `updated_at`     | INTEGER |                           | Unix timestamp (ms) of last cursor update |

**Lifecycle**:

- Created: On first `Subscribe` with a new topic/consumer_group pair
- Updated: On `Ack` message (cursor_seq advances)
- Deleted: Not supported in MVP

**Indexes**:

- Unique index on `(topic_id, consumer_group)` for upsert operations

**Notes**:

- `cursor_seq = 0` means "never consumed" — delivery starts from `global_seq > 0`
- Consumer takeover: When new consumer connects to existing group, it inherits `cursor_seq`

---

## Runtime State (Not Persisted)

### Credit Balance

Tracks per-stream flow control. Lives only in memory during active subscription.

| Field            | Type      | Description                             |
| ---------------- | --------- | --------------------------------------- |
| `credits`        | AtomicU32 | Messages consumer is willing to receive |
| `topic_id`       | u32       | Subscribed topic                        |
| `consumer_group` | String    | Consumer group for cursor tracking      |
| `consumer_id`    | String    | Optional identifier for logging         |

**Lifecycle**:

- Created: On `SubscriptionInit` message
- Updated: On `CreditGrant` (additive), on delivery (decrement)
- Destroyed: On stream close/error

---

## SQLite Schema

```sql
-- Pragma configuration (applied on connection open)
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA cache_size = -2000;
PRAGMA temp_store = MEMORY;

-- Topics table
CREATE TABLE IF NOT EXISTS topics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

-- Messages table
CREATE TABLE IF NOT EXISTS messages (
    global_seq INTEGER PRIMARY KEY AUTOINCREMENT,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    message_id TEXT NOT NULL,
    payload BLOB,
    attributes TEXT,
    created_at INTEGER NOT NULL
);

-- Index for subscription seeking: fetch messages for topic after cursor
CREATE INDEX IF NOT EXISTS idx_messages_topic_seq
ON messages(topic_id, global_seq);

-- Subscriptions table
CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    consumer_group TEXT NOT NULL,
    cursor_seq INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER,
    UNIQUE(topic_id, consumer_group)
);
```

---

## State Transitions

### Message State

```
[Not Exists] ─── Publish ───► [Persisted]
                                   │
                                   └─── (immutable) ───► [Persisted]
```

### Subscription Cursor State

```
[cursor_seq = 0] ─── Subscribe(EARLIEST) ───► [Delivering from seq 1]
       │
       └─── Subscribe(LATEST) ───► [Delivering from max_seq + 1]

[cursor_seq = N] ─── Ack(message with seq M) ───► [cursor_seq = M]
       │
       └─── Consumer Disconnect ───► [cursor_seq = N] (unchanged)
```

### Credit Balance State

```
[credits = 0] ─── CreditGrant(N) ───► [credits = N]
       │
       └─── No delivery (blocked)

[credits = N] ─── Deliver message ───► [credits = N-1]
       │
       └─── CreditGrant(M) ───► [credits = N+M]
```

---

## Validation Rules

### Topic

- `name`: Non-empty, max 255 characters, alphanumeric + `-` + `_` + `.`

### Message

- `message_id`: Valid UUIDv7 format
- `payload`: Max 4MB (gRPC default limit)
- `attributes`: Valid JSON object or null

### Subscription

- `consumer_group`: Non-empty, max 255 characters
- `cursor_seq`: Non-negative integer

---

## Query Patterns

### Write Path

1. **Get or Create Topic**

```sql
INSERT INTO topics (name, created_at) VALUES (?, ?)
ON CONFLICT(name) DO UPDATE SET name = name
RETURNING id;
```

2. **Insert Message** (batched in transaction)

```sql
INSERT INTO messages (topic_id, message_id, payload, attributes, created_at)
VALUES (?, ?, ?, ?, ?)
RETURNING global_seq;
```

### Read Path

3. **Fetch Messages for Subscription**

```sql
SELECT global_seq, message_id, payload, attributes, created_at
FROM messages
WHERE topic_id = ? AND global_seq > ?
ORDER BY global_seq ASC
LIMIT ?;
```

4. **Get or Create Subscription**

```sql
INSERT INTO subscriptions (topic_id, consumer_group, cursor_seq, updated_at)
VALUES (?, ?, 0, ?)
ON CONFLICT(topic_id, consumer_group) DO UPDATE SET updated_at = ?
RETURNING id, cursor_seq;
```

5. **Update Cursor**

```sql
UPDATE subscriptions
SET cursor_seq = ?, updated_at = ?
WHERE topic_id = ? AND consumer_group = ? AND cursor_seq < ?;
```

6. **Get Topic Max Sequence** (for LATEST initial position)

```sql
SELECT COALESCE(MAX(global_seq), 0) FROM messages WHERE topic_id = ?;
```

---

## Performance Considerations

1. **INTEGER PRIMARY KEY**: SQLite alias for rowid; appends to B-tree end (optimal for write-heavy)
2. **Composite Index**: `(topic_id, global_seq)` enables efficient range scans for subscriptions
3. **Batch Writes**: All inserts in single transaction amortize fsync cost
4. **Read Pool**: Multiple readers don't block each other in WAL mode
5. **JSON Attributes**: Stored as TEXT; no indexing needed for MVP (extensibility deferred)
