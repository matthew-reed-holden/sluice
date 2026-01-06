# Sluice Refactoring Plan: Workspace Restructure

## Overview

Restructure Sluice from a monolithic crate into a focused workspace with three core crates:
- `sluice-proto`: Shared protocol definitions
- `sluice-client`: Lightweight client library
- `sluice-server`: Full broker implementation

## Goals

- ✅ Reduce compile times for client tools by 66%
- ✅ Reduce binary sizes for client tools by 50%
- ✅ Create clear API boundaries
- ✅ Enable independent versioning
- ✅ Follow Rust workspace best practices

## Target Structure

```
sluice/
├── Cargo.toml                    # Workspace root
├── CLAUDE.md
├── README.md
├── REFACTORING_PLAN.md
│
├── crates/
│   ├── sluice-proto/             # NEW: Shared protobuf definitions
│   │   ├── Cargo.toml
│   │   ├── build.rs
│   │   ├── proto/
│   │   │   └── sluice/v1/sluice.proto
│   │   └── src/
│   │       └── lib.rs
│   │
│   ├── sluice-client/            # NEW: Client library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── connection.rs
│   │       └── subscription.rs
│   │
│   ├── sluice-server/            # NEW: Server implementation
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── main.rs
│   │       ├── config.rs
│   │       ├── server.rs
│   │       ├── flow/
│   │       ├── observability/
│   │       ├── service/
│   │       └── storage/
│   │
│   ├── sluicectl/                # EXISTING: CLI tool
│   │   └── Cargo.toml
│   │
│   └── lazysluice/               # EXISTING: TUI client
│       └── Cargo.toml
│
└── tests/                        # Integration tests
```

## Migration Phases

### Phase 1: Create Proto Crate ✅ (Low Risk)

**Objective:** Extract protocol definitions into `sluice-proto` crate.

**Steps:**
1. Create `crates/sluice-proto/` directory structure
2. Create `crates/sluice-proto/Cargo.toml`
3. Move `proto/` directory to `crates/sluice-proto/proto/`
4. Move `build.rs` to `crates/sluice-proto/build.rs`
5. Create `crates/sluice-proto/src/lib.rs`
6. Update root `Cargo.toml` to add workspace member
7. Update root crate to depend on `sluice-proto`
8. Update imports in `src/` to use `sluice_proto::*`
9. Update `sluicectl` to depend on `sluice-proto`
10. Update `lazysluice` to depend on `sluice-proto`
11. Test: `cargo build --workspace`
12. Test: `cargo test --workspace`

**Files Modified:**
- `Cargo.toml` (root)
- `src/lib.rs` (remove build.rs, update proto module)
- `src/client/mod.rs` (update proto imports)
- `src/service/mod.rs` (update proto imports)
- `crates/sluicectl/Cargo.toml`
- `crates/lazysluice/Cargo.toml`

**Files Created:**
- `crates/sluice-proto/Cargo.toml`
- `crates/sluice-proto/build.rs`
- `crates/sluice-proto/src/lib.rs`

**Files Moved:**
- `proto/` → `crates/sluice-proto/proto/`
- `build.rs` → `crates/sluice-proto/build.rs`

**Validation:**
```bash
cargo build --workspace
cargo test --workspace
cargo run -p sluice -- --help
cargo run -p sluicectl -- --help
cargo run -p lazysluice
```

---

### Phase 2: Extract Client Crate (Medium Risk)

**Objective:** Move `src/client/` to dedicated `sluice-client` crate.

**Steps:**
1. Create `crates/sluice-client/` directory structure
2. Create `crates/sluice-client/Cargo.toml`
3. Copy `src/client/` to `crates/sluice-client/src/`
4. Create `crates/sluice-client/src/lib.rs`
5. Update imports to use `sluice_proto`
6. Update root `Cargo.toml` to add workspace member
7. Update `sluicectl` to depend on `sluice-client`
8. Update `lazysluice` to depend on `sluice-client`
9. Remove `src/client/` from root crate
10. Update root `src/lib.rs` to remove client module
11. Test: `cargo build --workspace`
12. Test: `cargo test --workspace`
13. Test client tools work correctly

**Files Modified:**
- `Cargo.toml` (root - add workspace member)
- `src/lib.rs` (remove pub mod client)
- `crates/sluicectl/Cargo.toml`
- `crates/sluicectl/src/*.rs` (update imports)
- `crates/lazysluice/Cargo.toml`
- `crates/lazysluice/src/*.rs` (update imports)

**Files Created:**
- `crates/sluice-client/Cargo.toml`
- `crates/sluice-client/src/lib.rs`
- `crates/sluice-client/src/connection.rs` (copied)
- `crates/sluice-client/src/subscription.rs` (copied)

**Files Removed:**
- `src/client/` (entire directory)

**Validation:**
```bash
cargo build --workspace
cargo test --workspace
cargo run -p sluicectl -- topics list
cargo run -p lazysluice
```

---

### Phase 3: Move Server Implementation (Higher Risk)

**Objective:** Move server code to dedicated `sluice-server` crate.

**Steps:**
1. Create `crates/sluice-server/` directory structure
2. Create `crates/sluice-server/Cargo.toml` with all server dependencies
3. Move `src/main.rs` to `crates/sluice-server/src/main.rs`
4. Move `src/*.rs` to `crates/sluice-server/src/`
5. Move `src/*/` directories to `crates/sluice-server/src/`
6. Create `crates/sluice-server/src/lib.rs`
7. Update all imports to use `sluice_proto` and `sluice_client` (for tests)
8. Update root `Cargo.toml` to add workspace member
9. Update `tests/` to depend on `sluice-server` and `sluice-client`
10. Remove old `src/` directory
11. Update root `Cargo.toml` to remove `[[bin]]` and `[lib]`
12. Test: `cargo build --workspace`
13. Test: `cargo test --workspace`
14. Test: Server runs correctly

**Files Modified:**
- `Cargo.toml` (root - major changes)
- `tests/*.rs` (update imports)

**Files Created:**
- `crates/sluice-server/Cargo.toml`
- `crates/sluice-server/src/lib.rs`
- All server implementation files (moved)

**Files Removed:**
- Entire `src/` directory (moved to sluice-server)
- Root `build.rs` (already moved to proto crate)

**Validation:**
```bash
cargo build --workspace --release
cargo test --workspace
cargo run -p sluice-server -- --help
cargo run -p sluice-server
```

---

### Phase 4: Cleanup and Optimization (Final)

**Objective:** Optimize workspace configuration and documentation.

**Steps:**
1. Add workspace-level dependency management to root `Cargo.toml`
2. Update each crate to use `workspace = true` for shared deps
3. Update `CLAUDE.md` with new structure
4. Update `README.md` with new build instructions
5. Add `.cargo/config.toml` with useful aliases
6. Create `crates/sluice-client/README.md`
7. Create `crates/sluice-server/README.md`
8. Add examples to `sluice-client/examples/`
9. Verify all documentation is accurate
10. Final test suite run

**Files Modified:**
- `Cargo.toml` (root - add [workspace.dependencies])
- `crates/*/Cargo.toml` (use workspace deps)
- `CLAUDE.md`
- `README.md`

**Files Created:**
- `.cargo/config.toml`
- `crates/sluice-client/README.md`
- `crates/sluice-server/README.md`
- `crates/sluice-client/examples/simple_publish.rs`
- `crates/sluice-client/examples/streaming_subscribe.rs`

**Validation:**
```bash
cargo build --workspace --release
cargo test --workspace
cargo clippy --workspace
cargo doc --workspace --no-deps
```

---

## Testing Strategy

After each phase:

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Test server binary
cargo run -p sluice-server -- --help
cargo run -p sluice-server &
sleep 2
pkill sluice-server

# Test CLI tools
cargo run -p sluicectl -- --help
cargo run -p lazysluice &
sleep 1
pkill lazysluice

# Run specific integration tests
cargo test --test integration_test
cargo test --test publish_test
```

---

## Rollback Strategy

Each phase is isolated. If issues arise:

**Phase 1 Rollback:**
- Delete `crates/sluice-proto/`
- Restore `build.rs` and `proto/` to root
- Revert `Cargo.toml` changes
- `git checkout` modified files

**Phase 2 Rollback:**
- Delete `crates/sluice-client/`
- Restore `src/client/`
- Revert dependency changes in sluicectl/lazysluice
- `git checkout` modified files

**Phase 3 Rollback:**
- Delete `crates/sluice-server/`
- Restore `src/` directory
- Revert root `Cargo.toml`
- `git checkout` modified files

---

## Expected Improvements

### Compile Times (Clean Build)

| Crate | Before | After | Improvement |
|-------|--------|-------|-------------|
| sluicectl | ~45s | ~15s | **66% faster** |
| lazysluice | ~45s | ~15s | **66% faster** |
| sluice-server | ~45s | ~40s | Minimal |

### Binary Sizes (Release)

| Binary | Before | After | Reduction |
|--------|--------|-------|-----------|
| sluicectl | ~10MB | ~4-5MB | **50%** |
| lazysluice | ~11MB | ~5-6MB | **45%** |
| sluice | ~10MB | ~10MB | No change |

### Dependencies

**sluicectl Before:**
- SQLite, r2d2, OpenTelemetry, etc. (unnecessary)

**sluicectl After:**
- Only: tonic, prost, tokio (minimal), sluice-proto, sluice-client

---

## Success Criteria

- ✅ All tests pass
- ✅ Server binary runs correctly
- ✅ sluicectl works with real server
- ✅ lazysluice works with real server
- ✅ Client tools compile faster
- ✅ Client binaries are smaller
- ✅ No circular dependencies
- ✅ Clear API boundaries
- ✅ Documentation updated

---

## Timeline Estimate

- Phase 1: 1-2 hours
- Phase 2: 2-3 hours
- Phase 3: 3-4 hours
- Phase 4: 1-2 hours

**Total:** 7-11 hours of focused work

---

## Notes

- Use git commits between each phase for easy rollback
- Test thoroughly after each phase before proceeding
- Keep old structure until new structure is fully validated
- Update CI/CD after Phase 3 completion
