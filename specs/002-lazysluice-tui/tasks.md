# Tasks: lazysluice TUI client (MVP)

**Input**: Design documents from `/specs/002-lazysluice-tui/`  
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/, quickstart.md

**Notes on Tests**: The Sluice constitution requires TDD and contract tests for gRPC endpoints, so this plan includes explicit test tasks.

## MVP Keymap (Minimal)

These bindings define “vim-like / keyboard-driven” MVP behavior (FR-003) and MUST match the help screen.

- `q`: Quit
- `?`: Toggle help
- `Tab`: Cycle views (Topic List → Tail → Publish)
- `j/k` or `Up/Down`: Move selection (topics/messages)
- `Enter`: Select topic / start tail
- `p`: Jump to Publish view
- `Space`: Pause/resume tail rendering
- `a`: Ack selected message

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Establish workspace + new crate skeleton without changing server behavior.
- [X] T001 Update root Cargo workspace members in Cargo.toml (add `crates/lazysluice` while preserving existing `sluice` package)
- [X] T002 [P] Create lazysluice crate manifest in crates/lazysluice/Cargo.toml (bin + deps: ratatui/crossterm/tokio/tonic/prost/clap/anyhow/tracing)
- [X] T003 [P] Add proto codegen for lazysluice in crates/lazysluice/build.rs (compile from ../../proto/sluice/v1/sluice.proto)
- [X] T004 [P] Add lazysluice entrypoint + CLI args in crates/lazysluice/src/main.rs (`--endpoint`, `--tls-ca <path>` optional, `--tls-domain <dns>` optional, `--credits-window <n>`)
- [X] T005 [P] Create TUI module skeletons in crates/lazysluice/src/{app,ui,controller,events}.rs
- [X] T006 [P] Create gRPC wrapper module skeletons in crates/lazysluice/src/grpc/{mod,client}.rs

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Add additive topic discovery API + server implementation and contract test.

**⚠️ CRITICAL**: No lazysluice story can meet the topic-list requirement until this phase is complete.

- [X] T007 Add `ListTopics` RPC and messages to proto/sluice/v1/sluice.proto (additive only; keep existing Publish/Subscribe unchanged)
- [X] T011 Add gRPC contract test for ListTopics in tests/list_topics_test.rs (expected RED until server implementation exists)
- [X] T008 Wire new server handler module in src/service/mod.rs (declare + route `list_topics`)
- [X] T010 Implement storage query to list topics in src/storage/reader.rs (read `topics` table name + created_at)
- [X] T009 Implement ListTopics service logic in src/service/topics.rs (return lexicographically sorted topics)

**Checkpoint**: Foundation ready — lazysluice can now reliably populate the topic picker.

---

## Phase 3: User Story 1 - Connect and live-tail a topic (Priority: P1) 🎯 MVP

**Goal**: Connect to a Sluice server, load topic list, select topic, and live-tail from LATEST with reconnect.

**Independent Test**: Start `sluice serve`, publish messages with existing tooling, run lazysluice and confirm new messages appear within 250ms.

### Tests for User Story 1

-- [X] T012 [P] [US1] Add unit test for endpoint parsing/defaults in crates/lazysluice/src/main.rs
-- [X] T013 [P] [US1] Add unit test for reconnect/backoff timing policy in crates/lazysluice/src/controller.rs

### Implementation for User Story 1

-- [X] T014 [P] [US1] Implement gRPC connect + optional TLS configuration in crates/lazysluice/src/grpc/client.rs (TLS is enabled when endpoint scheme is `https://`; `--tls-ca` is required for TLS; if endpoint is `http://` then TLS flags are rejected with a clear error)
-- [X] T015 [P] [US1] Implement `list_topics()` call in crates/lazysluice/src/grpc/client.rs
- [X] T016 [US1] Implement crossterm event stream + tick events in crates/lazysluice/src/events.rs
- [X] T017 [US1] Implement app state for connection + topic list in crates/lazysluice/src/app.rs
- [X] T018 [US1] Implement topic list screen rendering in crates/lazysluice/src/ui.rs
- [X] T019 [US1] Implement controller state machine (connect → load topics → select topic) in crates/lazysluice/src/controller.rs
- [X] T045 [US1] Implement MVP keybindings (see “MVP Keymap”) in crates/lazysluice/src/controller.rs
- [X] T020 [US1] Implement Subscribe stream setup (Init + initial CreditGrant window) in crates/lazysluice/src/grpc/client.rs
- [X] T044 [US1] Implement credit refill policy (re-grant `credits_window` when remaining credits drops below `credits_window / 2`, rounding down) in crates/lazysluice/src/grpc/client.rs
- [X] T021 [US1] Render delivered messages in tail view in crates/lazysluice/src/ui.rs
- [X] T040 [US1] Render required metadata (message_id, sequence, timestamp, attributes summary) in crates/lazysluice/src/ui.rs
- [X] T042 [US1] Implement safe payload rendering policy (utf-8 else preview + length + truncation) in crates/lazysluice/src/ui.rs
- [X] T043 [P] [US1] Unit test payload rendering policy (utf8, binary, oversized) in crates/lazysluice/src/ui.rs
- [X] T022 [US1] Implement auto-reconnect with backoff when subscribe stream drops in crates/lazysluice/src/controller.rs

**Checkpoint**: lazysluice can connect, show topics, and tail a topic from LATEST.

---

## Phase 4: User Story 2 - Publish a test message (Priority: P2)

**Goal**: Publish a message to a selected topic from within the TUI.

**Independent Test**: With server running, publish via lazysluice and verify message appears in tail view (lazysluice or a separate subscriber).

### Tests for User Story 2

- [X] T023 [P] [US2] Add unit test for publish input validation (non-empty payload/topic) in crates/lazysluice/src/app.rs

### Implementation for User Story 2

- [X] T024 [P] [US2] Implement `publish()` RPC call wrapper in crates/lazysluice/src/grpc/client.rs
- [X] T025 [US2] Add publish screen state (draft payload, status) in crates/lazysluice/src/app.rs
- [X] T026 [US2] Render publish screen + status messages in crates/lazysluice/src/ui.rs
- [X] T027 [US2] Wire keybindings to switch to publish mode and submit publish in crates/lazysluice/src/controller.rs

---

## Phase 5: User Story 3 - Control subscription behavior (Priority: P3)

**Goal**: Control initial position, consumer group, pause/resume rendering, and manual per-message ack.

**Independent Test**: Tail from EARLIEST and confirm backlog renders; pause/resume stops/starts rendering; ack selected message and show in-session ack marker.

### Tests for User Story 3

- [X] T028 [P] [US3] Add unit test for credits window configuration parsing in crates/lazysluice/src/main.rs
- [X] T029 [P] [US3] Add unit test for message ack state tracking in crates/lazysluice/src/app.rs

### Implementation for User Story 3

- [X] T030 [US3] Add consumer group + initial position selection to subscribe init in crates/lazysluice/src/grpc/client.rs
- [X] T031 [US3] Add pause/resume toggle for tail rendering in crates/lazysluice/src/controller.rs
- [X] T032 [US3] Track selected message + acked IDs in app state in crates/lazysluice/src/app.rs
- [X] T033 [US3] Implement manual Ack RPC send on selected message in crates/lazysluice/src/grpc/client.rs
- [X] T034 [US3] Render ack indicator on messages in tail view in crates/lazysluice/src/ui.rs
- [X] T035 [US3] Add help/shortcuts screen rendering in crates/lazysluice/src/ui.rs

**Checkpoint**: Subscription controls and manual ack work end-to-end.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Tighten UX reliability and ensure quickstart is executable.

- [X] T036 [P] Add tracing spans/logs for connect/list/subscribe/publish actions in crates/lazysluice/src/controller.rs
- [ ] T047 [P] Add OpenTelemetry setup in lazysluice (tracing-opentelemetry + OTLP exporter; honor standard OTEL env vars) in crates/lazysluice/src/main.rs
- [ ] T048 [P] Emit minimal OTEL metrics from lazysluice (message throughput counter, arrival-to-render latency histogram, backpressure/credit events counter) in crates/lazysluice/src/{controller,grpc/client}.rs
- [X] T037 [P] Add graceful shutdown / terminal cleanup handling in crates/lazysluice/src/main.rs
- [X] T038 Ensure `quickstart.md` commands match built binaries in specs/002-lazysluice-tui/quickstart.md
- [X] T039 Run full test suite + clippy + fmt checklist in README.md (documented commands only) in README.md
- [ ] T046 Manual perf validation for SC-001/SC-002/SC-003 (record results in PR description): time-to-first-frame <1s, connect+tail <30s, tail render shows local publishes within ~250ms
- [ ] T049 Manual usability validation for SC-004 (hallway test protocol; record N, success count, and top failure mode(s) in PR description)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)** → **Foundational (Phase 2)** → **US1 (Phase 3)** → **US2 (Phase 4)** → **US3 (Phase 5)** → **Polish (Phase 6)**

### User Story Dependencies

- **US1** depends on **Phase 1–2**.
- **US2** depends on **US1** for shared connection/topic selection UX.
- **US3** depends on **US1** for the tail view and streaming subscription.

---

## Parallel Opportunities

- Setup: T002–T006 are parallelizable.
- Foundational: Draft T011 immediately after T007; it MUST fail before implementing T008–T010–T009.
- US1: T012–T015 can run in parallel.
- US3: T028–T029 can run in parallel.

---

## Parallel Execution Examples

### US1

```text
Run in parallel (different files):
- T014 (grpc connect/TLS) -> crates/lazysluice/src/grpc/client.rs
- T017 (app state) -> crates/lazysluice/src/app.rs
- T018 (topic list UI) -> crates/lazysluice/src/ui.rs
```

### US3

```text
Run in parallel (different files):
- T032 (ack state tracking) -> crates/lazysluice/src/app.rs
- T033 (Ack RPC send) -> crates/lazysluice/src/grpc/client.rs
- T034 (ack rendering) -> crates/lazysluice/src/ui.rs
```

---

## Implementation Strategy

### MVP scope

- MVP is **Phase 1–3 (Setup + Foundational + US1)**.

### Incremental delivery

- Implement and validate US1 first.
- Add publish (US2) next.
- Add subscription controls + ack UX (US3) last.
