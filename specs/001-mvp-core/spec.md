# Feature Specification: Sluice MVP (v0.1) — Core Message Broker

**Feature Branch**: `001-mvp-core`  
**Created**: 2026-01-01  
**Status**: Draft  
**Input**: User description: "Sluice: Comprehensive Architectural Specification and MVP Implementation Plan"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Publish a Message with Durability Guarantee (Priority: P1)

A producer application needs to send a message to a topic and receive confirmation that the message has been durably persisted. The producer must know with certainty that once acknowledged, the message will survive process crashes and power failures.

**Why this priority**: This is the foundational write path. Without reliable publish, no other functionality is meaningful. This enables the "At-Least-Once" delivery guarantee that differentiates Sluice from ephemeral brokers.

**Independent Test**: Can be fully tested by publishing a message via gRPC and verifying the response contains a valid message_id, sequence number, and timestamp. Delivers immediate value as an append-only durable log.

**Acceptance Scenarios**:

1. **Given** a running Sluice server, **When** a producer sends a `PublishRequest` with topic "orders" and payload "order-123", **Then** the server returns a `PublishResponse` with a valid UUIDv7 `message_id`, monotonic `sequence` number, and `timestamp`.

2. **Given** a Sluice server, **When** a producer publishes to a topic that does not exist, **Then** the topic is auto-created and the message is persisted successfully.

3. **Given** a Sluice server under load, **When** the internal write channel is full, **Then** the `Publish` RPC blocks/slows down (backpressure) rather than dropping messages or returning errors.

4. **Given** a message is published and acknowledged, **When** the Sluice process is killed with SIGKILL and restarted, **Then** the message is still present and retrievable.

---

### User Story 2 - Subscribe and Consume Messages with Flow Control (Priority: P1)

A consumer application needs to subscribe to a topic and receive messages at a rate it can handle. The consumer controls the flow using credit grants, preventing the broker from overwhelming slow consumers.

**Why this priority**: This is the foundational read path and implements the core backpressure mechanism that differentiates Sluice. Without consumption, publish has no value.

**Independent Test**: Can be fully tested by subscribing to a topic, granting credits, receiving messages, and verifying that no messages are pushed without credits.

**Acceptance Scenarios**:

1. **Given** a running Sluice server with messages in topic "orders", **When** a consumer opens a `Subscribe` stream with `SubscriptionInit` for topic "orders" and sends `CreditGrant(10)`, **Then** the server sends up to 10 `MessageDelivery` messages.

2. **Given** a consumer with 0 credits remaining, **When** new messages are published to the subscribed topic, **Then** the server does NOT push any messages until the consumer sends additional `CreditGrant`.

3. **Given** a consumer subscribed with `InitialPosition::EARLIEST`, **When** the stream is established, **Then** messages are delivered starting from the oldest available message in the topic.

4. **Given** a consumer subscribed with `InitialPosition::LATEST`, **When** the stream is established, **Then** only messages published after subscription are delivered.

---

### User Story 3 - Acknowledge Messages and Advance Cursor (Priority: P1)

A consumer needs to acknowledge processed messages so that the broker tracks consumption progress. If the consumer disconnects and reconnects with the same consumer group, it should resume from the last acknowledged position.

**Why this priority**: Without acknowledgment and cursor tracking, consumers cannot resume after crashes, breaking At-Least-Once delivery semantics.

**Independent Test**: Can be fully tested by consuming messages, sending ACKs, disconnecting, reconnecting with the same consumer group, and verifying messages resume from the correct position.

**Acceptance Scenarios**:

1. **Given** a consumer that received message with `sequence=5`, **When** the consumer sends `Ack(message_id)`, **Then** the server updates the cursor for that consumer group to `sequence=5`.

2. **Given** a consumer group "workers" with cursor at `sequence=5`, **When** a new consumer joins the same group, **Then** message delivery starts from `sequence=6`.

3. **Given** a consumer that crashes without ACKing, **When** the consumer reconnects with the same consumer group, **Then** unacknowledged messages are redelivered.

---

### User Story 4 - Operate a Single-Binary Broker (Priority: P2)

An operator needs to run Sluice as a single binary with minimal configuration, similar to SQLite. The broker should start quickly, use reasonable defaults, and be configurable via CLI arguments or environment variables.

**Why this priority**: Operational simplicity is a core differentiator. This enables the "SQLite-grade" deployment experience but depends on the core messaging paths (P1) being functional first.

**Independent Test**: Can be tested by running the Sluice binary with `--help`, verifying configuration options, and starting the server with default settings.

**Acceptance Scenarios**:

1. **Given** the Sluice binary, **When** an operator runs `sluice serve`, **Then** the server starts with sensible defaults (data directory, port, logging).

2. **Given** command-line arguments for port and data directory, **When** the operator starts Sluice, **Then** those settings are respected.

3. **Given** a running Sluice server, **When** the operator sends SIGTERM, **Then** the server performs graceful shutdown, flushing pending writes.

---

### User Story 5 - Monitor Broker Health and Performance (Priority: P2)

An operator needs visibility into broker performance, including publish latency, message throughput, consumer lag, and backpressure state. Metrics should be exportable via OpenTelemetry.

**Why this priority**: Observability is non-negotiable for production readiness, but the metrics are meaningless without the core messaging functionality (P1) in place.

**Independent Test**: Can be tested by publishing/consuming messages and querying the metrics endpoint for expected metric values.

**Acceptance Scenarios**:

1. **Given** a running Sluice server, **When** messages are published, **Then** the `sluice_publish_total` counter increments and `sluice_publish_latency_seconds` histogram is populated.

2. **Given** a consumer with 0 credits and pending messages, **When** metrics are queried, **Then** `sluice_backpressure_active` gauge shows value `1` for that subscription.

3. **Given** a consumer group lagging behind, **When** metrics are queried, **Then** `sluice_subscription_lag` gauge accurately reflects the delta between latest sequence and cursor.

---

### Edge Cases

- What happens when a consumer sends `CreditGrant(0)`? → Treated as no-op, credits remain unchanged.
- What happens when a consumer ACKs an already-ACKed message? → Idempotent, no error, cursor unchanged.
- What happens when a consumer ACKs a message_id that doesn't exist? → Log warning, no cursor change.
- How does the system handle payload sizes near gRPC limits (~4MB default)? → Reject with RESOURCE_EXHAUSTED status.
- What happens when disk is full? → Publish returns UNAVAILABLE status with descriptive error.
- What happens when a consumer disconnects mid-stream? → Server cleans up subscription handle, cursor remains at last ACK.
- What happens when a new consumer joins an already-active consumer group? → Seamless takeover: prior connection terminated, new consumer inherits cursor and starts fresh with 0 credits.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST expose a gRPC service defined in `proto/sluice/v1/sluice.proto` as the only external interface.
- **FR-002**: System MUST implement unary `Publish` RPC that persists messages with `fsync` before returning acknowledgment.
- **FR-003**: System MUST implement bidirectional `Subscribe` RPC supporting `SubscriptionInit`, `CreditGrant`, and `Ack` upstream messages.
- **FR-004**: System MUST enforce Credit-Based Flow Control — no message delivery without explicit credit.
- **FR-005**: System MUST auto-create topics on first publish if they don't exist.
- **FR-006**: System MUST generate UUIDv7 (time-sortable) message IDs server-side.
- **FR-007**: System MUST maintain monotonic sequence numbers per topic.
- **FR-008**: System MUST track consumer group cursors persistently across restarts.
- **FR-009**: System MUST support `LATEST` and `EARLIEST` initial positions for subscriptions.
- **FR-010**: System MUST implement Group Commit (batching) to achieve 5,000+ msg/s throughput.
- **FR-011**: System MUST use SQLite in WAL mode with `synchronous=FULL` for durability.
- **FR-012**: System MUST implement a Dedicated Writer thread to avoid blocking the async runtime.
- **FR-013**: System MUST use a Notification Bus to wake sleeping subscriptions when new data arrives.
- **FR-014**: System MUST emit OpenTelemetry-compatible metrics for key operations.
- **FR-015**: System MUST propagate W3C Trace Context via message attributes.
- **FR-016**: System MUST be deployable as a single statically-linked binary.
- **FR-017**: System MUST support graceful shutdown with pending write flush.

### Key Entities

- **Message**: The unit of data flowing through Sluice. Contains: message_id (UUIDv7), sequence (per-topic monotonic), topic_id, payload (opaque bytes), attributes (key-value headers), timestamp (Unix epoch ms).

- **Topic**: A named stream of messages. Auto-created on first publish. Contains: topic_id, name, creation timestamp.

- **Subscription**: A consumer's view into a topic. Contains: topic_id, consumer_group, cursor_seq (last ACKed sequence), consumer metadata.

- **Credit Balance**: Runtime state (not persisted) tracking how many messages a subscription stream can receive. Updated by `CreditGrant`, decremented on delivery.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Publish path completes in <10ms p99 latency under sustained 5,000 msg/s load.
- **SC-002**: System sustains 5,000+ messages per second throughput with group commit enabled.
- **SC-003**: 100% of acknowledged messages survive SIGKILL and power failure (durability test).
- **SC-004**: Server memory usage remains flat (<100MB) regardless of slow consumer backlog depth.
- **SC-005**: Slow consumer (1 msg/s) does not impact fast producer throughput (5,000 msg/s).
- **SC-006**: Subscription resumes from correct position after consumer crash and reconnect.
- **SC-007**: Single binary starts in <1 second on commodity hardware.
- **SC-008**: All core metrics (publish latency, throughput, lag, backpressure) are observable via OTEL exporter.

## Assumptions

- **A-001**: Single-node deployment only for MVP; no distributed consensus or replication.
- **A-002**: Topics are not deleted in MVP; retention policy deferred to future version.
- **A-003**: One consumer per consumer group at a time in MVP; competing consumers deferred. When a new consumer connects with the same `consumer_group`, it performs a **seamless takeover**: inherits the cursor position and the prior connection is terminated immediately.
- **A-004**: Authentication and authorization deferred to future version (v0.4).
- **A-005**: Maximum payload size follows gRPC default (~4MB); no chunking support.
- **A-006**: Target platform is Linux server; macOS for development only.

## Clarifications

### Session 2026-01-01

- Q: When a consumer reconnects with the same `consumer_group` but a different `consumer_id`, what should happen to in-flight credits and the subscription stream? → A: Seamless takeover: new consumer inherits cursor position; any prior connection for this group is terminated immediately.
