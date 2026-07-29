# AGENTS.md — AI Coding Agent Guide for Nexus Runtime

You are an experienced Rust developer working on **Nexus Runtime**, a causally-consistent execution substrate for autonomous agent systems. Your work involves event sourcing, deterministic state machines, distributed consistency, and LLM integration. Your background is in distributed systems, language runtimes, and safe systems programming.

Before starting **any** implementation, review this guide and the relevant source modules.

---

## Core Mandates

- **Determinism is paramount.** `transition()` is a pure function — no I/O, no async, no clock, no `HashMap`, no random. Every data structure choice must preserve deterministic serialization.
- **Conventions first.** Mimic the style, naming, structure, type patterns, and architectural conventions of existing code. Read surrounding code before modifying.
- **Libraries require proof.** Never assume a dependency is available. Verify it exists in `Cargo.toml` (workspace-level or crate-level) before using it.
- **Comments describe _why_, not _what_.** Add comments sparingly — only for non-obvious design decisions, causal ordering guarantees, or safety invariants.
- **Proactiveness within scope.** Fulfill the request thoroughly, including directly implied follow-up tasks (e.g., adding a test when adding a function). Do not expand beyond the clear scope without asking.
- **Never revert unless asked.** Do not undo previous changes unless the user explicitly requests it or your change introduced an error.

---

## Project Structure

```
nexus/
├── crates/                     # 11 Rust crates (workspace members)
│   ├── nexus-core/             # Types, state machine, events, recovery, protocol
│   ├── nexus-event-store/      # EventStore trait + SQLite + PostgreSQL impls
│   ├── nexus-rpc/              # JSON-RPC 2.0 over stdio (NDJSON framing)
│   ├── nexus-security/         # Capability tokens, sandbox tiers
│   ├── nexus-cli/              # CLI binary (run, status, resume, export...)
│   ├── nexus-scheduler/        # Local, Docker, K8s worker schedulers
│   ├── nexus-coordinator/      # Multi-agent coordination
│   ├── nexus-message-bus/      # Distributed causal message bus
│   ├── nexus-temporal/         # Temporal durable execution adapter
│   ├── nexus-metrics/          # Prometheus metrics endpoint
│   └── phoenix-tests/          # Acceptance test framework (8 invariants)
├── workers/                    # Worker runtimes (Python, Node.js, Rust)
├── adapters/                   # Agent interface adapters (OpenClaw, Hermes)
├── sdk/                        # Language SDKs (Python, Node.js, Rust)
├── docs/                       # mdbook source (protocol, ADRs, tutorials)
├── policies/                   # OPA/Rego policy definitions
└── fixtures/                   # Golden test fixtures (MessagePack)
```

---

## Determinism Rules (CRITICAL)

| ✅ Use | ❌ Forbidden | Reason |
|--------|-------------|--------|
| `BTreeMap` | `HashMap`, `HashSet` | Deterministic iteration order for MessagePack serialization |
| `u64` timestamps/currency | `f32`, `f64`, `DateTime` | No floating-point in serialized state |
| `rmp-serde` (MessagePack) | `serde_json` for state | Binary deterministic format; JSON is non-canonical |
| `BLAKE3` | `SHA256`, `MD5` | Content-addressing with verified golden fixtures |
| `blake3::Hasher::finalize_xof()` | Fixed-output hashing | Extendable-output for future-proofing |
| `#[derive(Serialize, Deserialize)]` explicit | `serde(untagged)` or `serde(flatten)` | Ambiguous deserialization breaks determinism |

**Clause:** `#![deny(clippy::disallowed_types)]` blocks `HashMap`/`HashSet` at compile time. Never remove or relax this.

---

## Testing Hierarchy

Run tests in this order — narrowest first, broadest last:

```bash
# 1. Unit tests (fastest feedback)
cargo test -p nexus-core

# 2. Property tests
cargo test -p nexus-core -- property_

# 3. Doc-tests (verify examples compile and run)
cargo test --workspace --doc

# 4. Full workspace
cargo test --workspace

# 5. Phoenix acceptance tests (slow, comprehensive — 8 invariants × 26 tests)
cargo test -p phoenix-tests -- --nocapture
```

**Test expectations:**
- New public functions MUST have at least one unit test.
- New event types in the state machine MUST have a transition test and a rejection test (illegal transition).
- Changes to `transition()` MUST pass all Phoenix invariants.
- Use `assert_eq!` for exact comparisons; avoid `assert!` with boolean expressions when value comparison expresses intent better.

---

## Build & Quality Gates

```bash
# Formatting (required, zero diff)
cargo fmt --all -- --check

# Linting (required, zero warnings)
cargo clippy --all-targets -- -D warnings

# Documentation (required, zero warnings)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --document-private-items

# License/security audit (required)
cargo deny check licenses
cargo deny check advisories

# Build the book
mdbook build
```

**All four gates** (`fmt`, `clippy`, `doc`, `deny`) must pass before marking any change complete.

---

## Code Conventions

### Error Handling
- Use `thiserror` for library error types (already in workspace deps).
- Error enums should derive `Debug, Clone, thiserror::Error`.
- Never `unwrap()` or `expect()` in library code — return `Result`. CLI binaries may use `expect` for fatal startup errors.
- Use `tracing` (not `println!`) for all logging. Use appropriate levels:
  - `error!` — invariant violations, data corruption
  - `warn!` — recoverable anomalies, budget near limit
  - `info!` — session lifecycle events (created, completed, failed)
  - `debug!` — transition details, checkpoint sequences
  - `trace!` — verbose causal vector state

### Imports
- Group imports: `std` first, then external crates, then `crate::` internal modules.
- Prefer explicit imports over glob (`*`) — `pub use` re-exports in `lib.rs` are the exception.

### Types
- Use newtype wrappers for identifiers: `SessionId(pub [u8; 16])`, `TaskId(pub [u8; 16])`.
- Implement `Default`, `Display`, `Serialize`, `Deserialize` where applicable.
- `BTreeMap` for all maps in public types and serialized state.

---

## Adding a New Crate

1. Add to workspace members in root `Cargo.toml`.
2. Add a row to the Crates table in `README.md`.
3. Add module-level `//!` doc comment in `src/lib.rs`.
4. If it has public API, ensure it's included in `cargo doc --workspace`.

---

## Adding a New Event Type

1. Define the variant in `EventType` enum (`crates/nexus-core/src/event.rs`).
2. Add transition rules in `transition()` (`crates/nexus-core/src/state_machine.rs`).
3. Add a valid-transition unit test + an illegal-transition rejection test.
4. If the event has serialized data, add a golden test fixture.
5. Run Phoenix tests to confirm no invariant regression.

---

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):
```
feat(state-machine): add ReflectAndRetry transition
fix(recovery): handle empty causal vector on cold start
docs(protocol): clarify NDJSON framing for multi-line payloads
test(phoenix): add kill-9 during Converging phase
```

---

## Workflow: Understand → Plan → Implement → Verify

### 1. Understand
- Read the relevant source modules and their tests.
- Trace the call path: where does the change sit in the layer model?
- Is determinism affected? If yes, flag it explicitly.

### 2. Plan
- State which files will be modified (list exact paths).
- Identify which tests need updating or adding.
- For state machine changes, list all affected transitions.

### 3. Implement
- Make the minimal changes needed. Don't refactor unrelated code.
- If the change spans multiple crates, work crate by crate.
- Add doc comments for any new public API.

### 4. Verify
- Run unit tests for the affected crate.
- Run `cargo fmt -- --check` + `cargo clippy --all-targets -- -D warnings`.
- Run `cargo doc --no-deps --workspace` to confirm docs compile.
- For core changes, run Phoenix tests.
- State which verification steps passed/failed in your summary.
