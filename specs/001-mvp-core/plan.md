# Implementation Plan: Sluice MVP (v0.1) — Core Message Broker

**Branch**: `001-mvp-core` | **Date**: 2026-01-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-mvp-core/spec.md`

## Summary

Implement a gRPC-native message broker with credit-based flow control, SQLite WAL persistence, and OpenTelemetry observability. The MVP delivers At-Least-Once delivery semantics with 5,000+ msg/s throughput via group commit batching, while maintaining operational simplicity as a single statically-linked binary.

## Technical Context

**Language/Version**: Rust (latest stable, 1.75+)  
**Primary Dependencies**: tonic (gRPC), prost (protobuf), tokio (async runtime), rusqlite (SQLite), tracing + opentelemetry  
**Storage**: SQLite in WAL mode with `synchronous=FULL` for durability  
**Testing**: cargo test with contract tests (tonic mock) and integration tests  
**Target Platform**: Linux server (single-node); macOS for development  
**Project Type**: Single binary server application  
**Performance Goals**: 5,000+ msg/s sustained throughput, <10ms p99 publish latency  
**Constraints**: <100MB memory, messages survive SIGKILL/power failure  
**Scale/Scope**: Single-node, auto-created topics, one consumer per consumer group

## Constitution Check

_GATE: Must pass before Phase 0 research. Re-check after Phase 1 design._

| Principle                                           | Status  | Evidence                                                                                   |
| --------------------------------------------------- | ------- | ------------------------------------------------------------------------------------------ |
| I. gRPC-Native Protocol (NON-NEGOTIABLE)            | ✅ PASS | Proto file is canonical source; tonic/prost for code generation                            |
| II. Application-Level Backpressure (NON-NEGOTIABLE) | ✅ PASS | Credit-based flow control in SubscribeUpstream; FR-004 mandates no delivery without credit |
| III. Test-Driven Development (NON-NEGOTIABLE)       | ✅ PASS | TDD workflow defined; contract + integration tests required                                |
| IV. Operational Simplicity                          | ✅ PASS | Single binary target; SQLite embedded; no external dependencies                            |
| V. Observability via OpenTelemetry (NON-NEGOTIABLE) | ✅ PASS | FR-014, FR-015 mandate OTEL metrics and W3C trace propagation                              |

**Gate Result**: ✅ PASSED — Proceed to Phase 0

## Project Structure

### Documentation (this feature)

```text
specs/001-mvp-core/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output (OpenAPI/protobuf docs)
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
src/
├── main.rs              # CLI entry point (clap)
├── lib.rs               # Library root
├── config.rs            # Configuration (CLI args, env vars)
├── server.rs            # gRPC server setup (tonic)
├── service/
│   ├── mod.rs
│   ├── publish.rs       # Publish RPC handler
│   └── subscribe.rs     # Subscribe RPC handler (bidirectional stream)
├── storage/
│   ├── mod.rs
│   ├── writer.rs        # Dedicated Writer Actor (std::thread)
│   ├── reader.rs        # Read pool (r2d2)
│   ├── schema.rs        # SQLite schema and migrations
│   └── batch.rs         # Group commit logic
├── flow/
│   ├── mod.rs
│   ├── credit.rs        # Credit balance tracking
│   └── notify.rs        # Notification bus (tokio::sync::broadcast)
├── observability/
│   ├── mod.rs
│   ├── metrics.rs       # Prometheus/OTLP metrics
│   └── tracing.rs       # OpenTelemetry tracing setup
└── proto.rs             # Generated protobuf code (build.rs output)

proto/
└── sluice/v1/
    └── sluice.proto     # Canonical API definition

tests/
├── contract/
│   ├── publish_test.rs  # Publish RPC contract tests
│   └── subscribe_test.rs # Subscribe RPC contract tests
├── integration/
│   ├── durability_test.rs # SIGKILL survival test
│   ├── backpressure_test.rs # Credit-based flow control test
│   └── cursor_test.rs   # Consumer group cursor persistence
└── common/
    └── mod.rs           # Test utilities (server harness, helpers)
```

**Structure Decision**: Single project layout. Rust binary with embedded library (`src/lib.rs`) for testability. Proto definitions in `proto/` with build-time code generation.
