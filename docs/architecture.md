# Nexus Runtime Architecture

## Layer Model

Nexus Runtime is organized in five layers, each with clearly defined responsibilities:

```
┌──────────────────────────────────────────────────────────┐
│ L5: Agent Interface Adapters                             │
│    OpenClaw Gateway / Hermes CLI / Nexus CLI             │
├──────────────────────────────────────────────────────────┤
│ L4: Nexus Kernel (Rust)                                  │
│    Causal State Machine · Event Store · Recovery Manager │
│    Worker Scheduler · Entropy Controller · Side-Effect   │
│    Guard · Cost Governor · LLM Proxy                     │
├──────────────────────────────────────────────────────────┤
│ L3: Worker Fabric                                        │
│    Python · Node.js · Rust · WASM (JSON-RPC 2.0 / stdio) │
│    No ports, no network access, no persistent state      │
├──────────────────────────────────────────────────────────┤
│ L2: Causal Memory & Persistence                          │
│    Event Log · Memory Graph · Content Vault (BLAKE3)     │
│    Vector Clock · Two-Phase Commit                       │
├──────────────────────────────────────────────────────────┤
│ L1: External Toolchain                                   │
│    LLM APIs (OpenAI / Anthropic / DeepSeek)              │
│    Docker / Kubernetes · OPA/Rego Policies               │
└──────────────────────────────────────────────────────────┘
```

## Deployment Modes

| Mode | Storage | Scheduler | Requirements | Status |
|------|---------|-----------|--------------|--------|
| **Lite** | SQLite (WAL) | Local process | Zero dependencies | ✅ Ready |
| **Pro** | PostgreSQL | Docker | Docker daemon | ⚡ Beta |
| **Enterprise** | PostgreSQL + Temporal | Kubernetes | K8s cluster | 🚧 Roadmap |

All modes share identical protocol semantics and state machine behavior.

## Crate Map

| Crate | Purpose |
|-------|---------|
| `nexus-core` | State machine, types, events, recovery, side-effect guard, vault, LLM proxy, WASM sandbox |
| `nexus-event-store` | `EventStore` trait + SQLite and PostgreSQL implementations |
| `nexus-rpc` | JSON-RPC 2.0 codec over stdio (NDJSON framing) |
| `nexus-security` | HMAC-SHA256 capability tokens, sandbox tiers, path traversal protection |
| `nexus-cli` | CLI binary for session lifecycle management |
| `nexus-scheduler` | Local, Docker (Bollard), and Kubernetes (kube-rs) worker schedulers |
| `nexus-coordinator` | Multi-agent coordination (propose/vote/commit/converge) |
| `nexus-message-bus` | Distributed causal message bus |
| `nexus-temporal` | Temporal durable execution adapter |
| `nexus-metrics` | Prometheus metrics endpoint |
| `phoenix-tests` | 8-invariant acceptance test framework |

## Key Files

| File | Description |
|------|-------------|
| `Cargo.toml` | Workspace with 16 members |
| `crates/nexus-core/src/state_machine.rs` | `transition()` pure function |
| `crates/nexus-core/src/types.rs` | `NexusState`, `CausalVector`, `Budget` |
| `crates/nexus-event-store/src/` | SQLite/PostgreSQL event stores |
| `crates/phoenix-tests/` | 8-invariant, 6 kill-9 phase tests |
| `deny.toml` | cargo-deny license + advisory config |
| `.clippy.toml` | Disallowed types for determinism |

## Design Invariants

| Constraint | Enforcement |
|------------|------------|
| `transition()` is a pure function | No async, no I/O, no clock, no random |
| Event log is append-only | No UPDATE/DELETE on events table |
| Deterministic serialization | `BTreeMap`, `u64`, `rmp-serde` MessagePack |
| Workers are stateless | No persistent memory, no network |
| Two-phase side effects | Intent → Validate → Execute → Commit |
| Phoenix gate | All 8 invariants must pass before release |

## Further Reading

- [Technical Specification](../td.md) — Frozen design spec
- [ADR-001: Deterministic Runtime](adr/ADR-001-deterministic-runtime.md) — Why determinism matters
- [ADR-004: Authority Boundary](adr/ADR-004-authority-boundary.md) — Three-layer security model
- [Protocol Overview](protocol/overview.md) — Event sourcing, causal consistency, worker protocol
