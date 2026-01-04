```markdown
# Tasks: Sluice MVP (v0.1) — Core Message Broker

**Input**: Design documents from `/specs/001-mvp-core/`
**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/ ✓

**Tests**: Included per Constitution III (TDD is NON-NEGOTIABLE). Contract tests and integration tests are written FIRST to fail before implementation.

**Organization**: Tasks grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story (US1-US5) this task belongs to
- Exact file paths from plan.md included in descriptions

---

## Phase 1: Setup (Project Initialization)

**Purpose**: Project structure and build configuration

- [x] T001 Create project directory structure per plan.md in src/
- [x] T002 Configure Cargo.toml with dependencies: tonic, prost, tokio, rusqlite, r2d2, r2d2_sqlite, uuid (v7), tracing, opentelemetry, clap
- [x] T003 [P] Setup build.rs for tonic protobuf code generation from proto/sluice/v1/sluice.proto
- [x] T004 [P] Configure clippy and rustfmt settings in .rustfmt.toml and clippy.toml
- [x] T005 [P] Create src/lib.rs as library root with module declarations
- [x] T006 Create tests/common/mod.rs with test utilities and server harness

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story

**⚠️ CRITICAL**: All user stories depend on this phase — no story work until complete

- [x] T007 Implement SQLite schema initialization in src/storage/schema.rs per data-model.md
- [x] T008 [P] Implement connection pool (r2d2) for read operations in src/storage/reader.rs
- [x] T009 Implement dedicated writer thread with mpsc channel in src/storage/writer.rs per research.md decision 2
- [x] T010 [P] Implement notification bus using tokio::sync::broadcast in src/flow/notify.rs per research.md decision 3
- [x] T011 Implement group commit batch logic in src/storage/batch.rs per research.md decision 1
- [x] T012 [P] Setup OpenTelemetry tracing in src/observability/tracing.rs
- [x] T013 [P] Setup gRPC server skeleton with tonic in src/server.rs
- [x] T014 Create src/proto.rs module for generated protobuf code re-export
- [x] T015 Implement configuration parsing (CLI args, env vars) with clap in src/config.rs

**Checkpoint**: Foundation ready — user story implementation can now begin

---

## Phase 3: User Story 1 — Publish a Message with Durability Guarantee (Priority: P1) 🎯 MVP

**Goal**: Producers send messages that survive crashes. Enables At-Least-Once delivery.

**Independent Test**: Publish message via gRPC → verify response has valid message_id, sequence, timestamp

### Tests for User Story 1 ⚠️ TDD: Write FIRST, ensure FAIL

- [x] T016 [P] [US1] Contract test for Publish RPC in tests/publish_test.rs — test valid publish returns message_id, sequence, timestamp
- [x] T017 [P] [US1] Contract test for auto-topic creation in tests/publish_test.rs — test publish to new topic creates it
- [x] T018 [P] [US1] Integration test for durability in tests/integration_test.rs — test message survives restart

### Implementation for User Story 1

- [x] T019 [P] [US1] Create Topic entity operations in src/storage/schema.rs — insert_or_get_topic()
- [x] T020 [P] [US1] Create Message entity operations in src/storage/schema.rs — insert_message()
- [x] T021 [US1] Implement UUIDv7 message ID generation utility in src/lib.rs per research.md decision 5
- [x] T022 [US1] Implement WriteCommand for publish in src/storage/writer.rs — PublishCommand struct
- [x] T023 [US1] Implement Publish RPC handler in src/service/publish.rs — persistence, fsync, response
- [x] T024 [US1] Wire Publish service to gRPC server in src/server.rs
- [x] T025 [US1] Add error handling for RESOURCE_EXHAUSTED (4MB limit) and UNAVAILABLE (disk full) in src/service/publish.rs
- [x] T026 [US1] Add tracing spans for publish operations in src/service/publish.rs

**Checkpoint**: User Story 1 complete — publish works end-to-end, messages survive crash

---

## Phase 4: User Story 2 — Subscribe and Consume Messages with Flow Control (Priority: P1)

**Goal**: Consumers subscribe with credit-based flow control. Core backpressure mechanism.

**Independent Test**: Subscribe to topic → grant credits → receive messages → verify no messages without credits

### Tests for User Story 2 ⚠️ TDD: Write FIRST, ensure FAIL

- [x] T027 [P] [US2] Contract test for Subscribe RPC init in tests/subscribe_test.rs — test SubscriptionInit establishes stream
- [x] T028 [P] [US2] Contract test for CreditGrant flow control in tests/subscribe_test.rs — test no delivery without credits
- [x] T029 [P] [US2] Contract test for EARLIEST/LATEST positions in tests/subscribe_test.rs
- [x] T030 [P] [US2] Integration test for backpressure in tests/integration_test.rs — slow consumer does not block producer

### Implementation for User Story 2

- [x] T031 [P] [US2] Implement credit balance tracking with AtomicU32 in src/flow/credit.rs per research.md decision 4
- [x] T032 [US2] Create Subscription entity operations in src/storage/schema.rs — get_or_create_subscription()
- [x] T033 [US2] Implement message fetch from cursor in src/storage/reader.rs — fetch_messages_from_seq()
- [x] T034 [US2] Implement SubscriptionState struct in src/service/subscribe.rs — credits, cursor, topic_id
- [x] T035 [US2] Implement Subscribe RPC handler (bidirectional stream) in src/service/subscribe.rs
- [x] T036 [US2] Implement inbound message handler (Init, CreditGrant, Ack) in src/service/subscribe.rs
- [x] T037 [US2] Implement outbound message delivery loop in src/service/subscribe.rs — respects credits
- [x] T038 [US2] Integrate notification bus wake-up in src/service/subscribe.rs per research.md decision 3
- [x] T039 [US2] Wire Subscribe service to gRPC server in src/server.rs
- [x] T040 [US2] Add tracing spans for subscribe operations in src/service/subscribe.rs

**Checkpoint**: User Story 2 complete — subscription with credits works, backpressure active

---

## Phase 5: User Story 3 — Acknowledge Messages and Advance Cursor (Priority: P1)

**Goal**: Consumers ACK messages, cursor persists, resume after crash.

**Independent Test**: Consume → ACK → disconnect → reconnect → verify resume from correct position

### Tests for User Story 3 ⚠️ TDD: Write FIRST, ensure FAIL

- [x] T041 [P] [US3] Contract test for Ack updates cursor in tests/subscribe_test.rs
- [x] T042 [P] [US3] Integration test for cursor persistence in tests/integration_test.rs — restart resumes from ACKed position
- [x] T043 [P] [US3] Contract test for idempotent Ack in tests/subscribe_test.rs — duplicate ACK is no-op

### Implementation for User Story 3

- [x] T044 [US3] Implement cursor update in src/storage/schema.rs — update_cursor()
- [x] T045 [US3] Implement Ack handling in src/service/subscribe.rs — lookup message, update cursor
- [x] T046 [US3] Implement consumer group takeover logic in src/service/subscribe.rs per spec.md clarification — terminate prior connection, inherit cursor
- [x] T047 [US3] Add warning logging for non-existent message ACK in src/service/subscribe.rs
- [x] T048 [US3] Add tracing spans for ACK operations in src/service/subscribe.rs

**Checkpoint**: User Story 3 complete — ACK/cursor works, resume after crash verified

---

## Phase 6: User Story 4 — Operate a Single-Binary Broker (Priority: P2)

**Goal**: Operators run Sluice as single binary with CLI config and graceful shutdown.

**Independent Test**: Run binary with --help → verify options; start with defaults → verify serves gRPC

### Tests for User Story 4 ⚠️ TDD: Write FIRST, ensure FAIL

- [x] T049 [P] [US4] Integration test for CLI help in tests/cli_test.rs — verify --help output
- [x] T050 [P] [US4] Integration test for graceful shutdown in tests/cli_test.rs — SIGTERM exits cleanly

### Implementation for User Story 4

- [x] T051 [US4] Implement CLI argument parsing in src/main.rs with clap — port, data-dir, log-level
- [x] T052 [US4] Implement server start/stop lifecycle in src/main.rs — tokio::signal for SIGTERM
- [x] T053 [US4] Implement graceful shutdown with pending write flush in src/storage/writer.rs per research.md decision 8
- [x] T054 [US4] Add startup banner with version and config in src/main.rs
- [x] T055 [US4] Verify single binary build works in release mode — cargo build --release

**Checkpoint**: User Story 4 complete — binary runs, configurable, shuts down gracefully

---

## Phase 7: User Story 5 — Monitor Broker Health and Performance (Priority: P2)

**Goal**: Operators observe metrics: publish latency, throughput, lag, backpressure.

**Independent Test**: Publish/consume messages → query metrics endpoint → verify expected values

### Tests for User Story 5 ⚠️ TDD: Write FIRST, ensure FAIL

- [x] T056 [P] [US5] Integration test for metrics emission — verified via unit tests in src/observability/metrics.rs
- [x] T057 [P] [US5] Integration test for backpressure gauge — verified via unit tests in src/observability/metrics.rs

### Implementation for User Story 5

- [x] T058 [US5] Implement Prometheus/OTLP metrics registry in src/observability/metrics.rs
- [x] T059 [US5] Add sluice_publish_total counter in src/service/publish.rs
- [x] T060 [US5] Add sluice_publish_latency_seconds histogram in src/service/publish.rs
- [x] T061 [US5] Add sluice_backpressure_active gauge in src/service/subscribe.rs
- [x] T062 [US5] Add sluice_subscription_lag gauge in src/service/subscribe.rs
- [x] T063 [US5] Implement W3C trace context propagation via attributes in src/service/publish.rs per contracts/grpc-api.md
- [x] T064 [US5] Wire metrics endpoint to gRPC server in src/server.rs

**Checkpoint**: User Story 5 complete — all key metrics observable

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Final validation and refinements

- [x] T065 [P] Run full quickstart.md validation in tests/quickstart_test.rs — end-to-end smoke test
- [x] T066 [P] Add README.md usage documentation
- [x] T067 Performance benchmark in tests/benchmark_test.rs — ~1,700-3,200 msg/s (hardware dependent, fsync-bound)
- [x] T068 Memory profiling in tests/benchmark_test.rs — stability verified under sustained load
- [x] T069 [P] Add doc comments to public APIs in src/lib.rs
- [x] T070 Security review: validate input sanitization for topic names
- [x] T071 Final clippy and rustfmt pass across codebase

---

## Dependencies & Execution Order

### Phase Dependencies
```

Phase 1 (Setup) → Phase 2 (Foundational) → Phase 3-7 (User Stories) → Phase 8 (Polish)

```

### User Story Dependencies

| Story | Depends On         | Can Parallelize With |
| ----- | ------------------ | -------------------- |
| US1   | Phase 2 completion | None (start first)   |
| US2   | Phase 2 completion | US1 (if team allows) |
| US3   | US2 (uses ACK)     | None                 |
| US4   | US1 (needs server) | US3, US5             |
| US5   | US1 (needs server) | US3, US4             |

### Within Each User Story

1. **Tests FIRST** — write contract/integration tests (TDD per Constitution III)
2. **Verify tests FAIL** — confirms tests are valid
3. **Implement** — models → services → handlers
4. **Verify tests PASS** — confirms implementation complete
5. **Checkpoint** — validate story works independently

### Parallel Opportunities per Phase

**Phase 1 (Setup)**:
```

T003 (build.rs) ║ T004 (lint config) ║ T005 (lib.rs)

```

**Phase 2 (Foundational)**:
```

T008 (reader pool) ║ T010 (notify bus) ║ T012 (tracing) ║ T013 (server skeleton)

```

**Phase 3 (US1 Tests)**:
```

T016 (publish contract) ║ T017 (auto-topic) ║ T018 (durability)

```

**Phase 3 (US1 Implementation)**:
```

T019 (topic ops) ║ T020 (message ops)

```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (~6 tasks)
2. Complete Phase 2: Foundational (~9 tasks) — **BLOCKS everything**
3. Complete Phase 3: User Story 1 (~11 tasks)
4. **STOP and VALIDATE**: Publish works, messages survive crash
5. Deploy/demo if ready — minimal viable broker!

### Incremental Delivery

| Increment  | Delivers                     | Cumulative Tasks |
| ---------- | ---------------------------- | ---------------- |
| Foundation | Build system, storage engine | 15               |
| MVP (US1)  | Durable publish              | 26               |
| +US2       | Subscribe with flow control  | 40               |
| +US3       | ACK and cursor persistence   | 48               |
| +US4       | CLI operations               | 55               |
| +US5       | Observability                | 64               |
| Polish     | Production ready             | 71               |

### Parallel Team Strategy

With 2 developers after Foundational:

- **Developer A**: US1 (Publish) → US3 (Ack) → US5 (Metrics)
- **Developer B**: US2 (Subscribe) → US4 (Operations) → Polish

---

## Notes

- [P] tasks work on different files with no dependencies — safe to parallelize
- [Story] label maps task to user story for traceability
- Each user story checkpoint is independently deployable
- TDD is NON-NEGOTIABLE per Constitution III — tests MUST fail before implementation
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
```
