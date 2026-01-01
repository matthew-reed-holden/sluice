# gRPC API Contract: Sluice v1

**Feature**: 001-mvp-core  
**Date**: 2026-01-01  
**Proto File**: [proto/sluice/v1/sluice.proto](../../../proto/sluice/v1/sluice.proto)

## Overview

Sluice exposes a single gRPC service with two RPCs:

1. **Publish** (Unary): Durably persist a message to a topic
2. **Subscribe** (Bidirectional Streaming): Consume messages with credit-based flow control

---

## Service Definition

```protobuf
service Sluice {
  rpc Publish(PublishRequest) returns (PublishResponse);
  rpc Subscribe(stream SubscribeUpstream) returns (stream SubscribeDownstream);
}
```

---

## RPC: Publish

Durably persist a single message to a topic. Returns only after message is fsync'd to WAL.

### Request: `PublishRequest`

| Field        | Type                | Required | Description                                       |
| ------------ | ------------------- | -------- | ------------------------------------------------- |
| `topic`      | string              | Yes      | Target topic name. Auto-created if doesn't exist. |
| `payload`    | bytes               | No       | Opaque message content. Max 4MB.                  |
| `attributes` | map<string, string> | No       | Key-value headers for tracing/metadata.           |

### Response: `PublishResponse`

| Field        | Type   | Description                               |
| ------------ | ------ | ----------------------------------------- |
| `message_id` | string | Server-generated UUIDv7 (time-sortable)   |
| `sequence`   | uint64 | Global monotonic sequence number          |
| `timestamp`  | int64  | Server-side Unix timestamp (milliseconds) |

### Error Codes

| gRPC Status          | Condition                            |
| -------------------- | ------------------------------------ |
| `OK`                 | Message durably persisted            |
| `INVALID_ARGUMENT`   | Empty topic name, invalid characters |
| `RESOURCE_EXHAUSTED` | Payload exceeds 4MB limit            |
| `UNAVAILABLE`        | Disk full, server shutting down      |
| `INTERNAL`           | Storage error                        |

### Example

```
Request:
  topic: "orders"
  payload: <binary data>
  attributes: {"traceparent": "00-abc123..."}

Response:
  message_id: "01932f4c-5b6d-7000-8000-123456789abc"
  sequence: 42
  timestamp: 1767225600000
```

---

## RPC: Subscribe

Bidirectional streaming RPC for consuming messages with credit-based flow control.

### Upstream Messages: `SubscribeUpstream`

Client sends one of three message types:

#### 1. `SubscriptionInit` (Required, First Message)

| Field              | Type            | Required | Description                   |
| ------------------ | --------------- | -------- | ----------------------------- |
| `topic`            | string          | Yes      | Topic to subscribe to         |
| `consumer_group`   | string          | No       | Defaults to "default"         |
| `consumer_id`      | string          | No       | Client identifier for logging |
| `initial_position` | InitialPosition | No       | Where to start consuming      |

**InitialPosition Enum**:
| Value | Description |
|-------|-------------|
| `LATEST` (0) | Start from new messages only |
| `EARLIEST` (1) | Start from oldest available message |

#### 2. `CreditGrant`

| Field     | Type   | Description                                      |
| --------- | ------ | ------------------------------------------------ |
| `credits` | uint32 | Messages client is willing to receive. Additive. |

#### 3. `Ack`

| Field        | Type   | Description                |
| ------------ | ------ | -------------------------- |
| `message_id` | string | Message being acknowledged |

### Downstream Messages: `SubscribeDownstream`

Server sends one message type:

#### `MessageDelivery`

| Field        | Type                | Description                   |
| ------------ | ------------------- | ----------------------------- |
| `message_id` | string              | UUIDv7 message identifier     |
| `sequence`   | uint64              | Global sequence number        |
| `payload`    | bytes               | Message content               |
| `attributes` | map<string, string> | Key-value headers             |
| `timestamp`  | int64               | Unix timestamp (milliseconds) |

### Flow Control Protocol

```
Client                                    Server
  │                                         │
  ├─── SubscriptionInit ───────────────────►│
  │                                         │
  │    (Server initializes subscription,    │
  │     no messages sent yet)               │
  │                                         │
  ├─── CreditGrant(10) ────────────────────►│
  │                                         │
  │◄─── MessageDelivery (msg 1) ────────────┤
  │◄─── MessageDelivery (msg 2) ────────────┤
  │     ... (up to 10 messages) ...         │
  │◄─── MessageDelivery (msg 10) ───────────┤
  │                                         │
  │    (credits exhausted, server blocks)   │
  │                                         │
  ├─── Ack(msg 5) ─────────────────────────►│
  │    (cursor advances to seq 5)           │
  │                                         │
  ├─── CreditGrant(5) ─────────────────────►│
  │                                         │
  │◄─── MessageDelivery (msg 11) ───────────┤
  │     ... (up to 5 more messages) ...     │
  │                                         │
```

### Error Codes

| gRPC Status        | Condition                           |
| ------------------ | ----------------------------------- |
| `OK`               | Stream closed normally              |
| `INVALID_ARGUMENT` | Missing Init, empty topic           |
| `NOT_FOUND`        | Topic doesn't exist (with EARLIEST) |
| `CANCELLED`        | Client cancelled stream             |
| `UNAVAILABLE`      | Server shutting down                |
| `INTERNAL`         | Storage error                       |

### Consumer Group Takeover

When a new consumer connects with the same `consumer_group`:

1. Server terminates prior connection with `ABORTED` status
2. New consumer inherits cursor position
3. New consumer starts with 0 credits (must send CreditGrant)

---

## W3C Trace Context

Sluice propagates distributed tracing via message attributes:

| Attribute Key | Description                                    |
| ------------- | ---------------------------------------------- |
| `traceparent` | W3C Trace Context traceparent header           |
| `tracestate`  | W3C Trace Context tracestate header (optional) |

Example:

```
attributes: {
  "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
}
```

---

## Idempotency

| Operation   | Idempotent? | Notes                         |
| ----------- | ----------- | ----------------------------- |
| Publish     | No          | Each call creates new message |
| Subscribe   | Yes         | Reconnect resumes from cursor |
| CreditGrant | Yes         | Additive, no side effects     |
| Ack         | Yes         | Duplicate acks are no-op      |

---

## Rate Limits

MVP does not implement server-side rate limiting. Backpressure is achieved via:

1. **Producer backpressure**: Bounded write channel blocks when full
2. **Consumer backpressure**: Credit-based flow control

---

## Versioning

API version is embedded in package path: `sluice.v1`

Future versions will use `sluice.v2`, `sluice.v3`, etc. with graceful deprecation policy.
