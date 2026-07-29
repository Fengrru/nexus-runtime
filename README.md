<div align="center">

# Nexus Runtime

**Causally-consistent execution substrate for autonomous agent systems.**

[![CI](https://github.com/Fengrru/nexus-runtime/actions/workflows/ci.yml/badge.svg)](https://github.com/Fengrru/nexus-runtime/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/rust-1.80+-orange.svg)](https://www.rust-lang.org)
[![codecov](https://codecov.io/gh/Fengrru/nexus-runtime/branch/main/graph/badge.svg)](https://codecov.io/gh/Fengrru/nexus-runtime)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Security Audit](https://github.com/Fengrru/nexus-runtime/actions/workflows/ci.yml/badge.svg?job=security)](https://github.com/Fengrru/nexus-runtime/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-fengrru.github.io-blue)](https://fengrru.github.io/nexus-runtime/)
[![docs.rs](https://img.shields.io/docsrs/nexus-core)](https://docs.rs/nexus-core)

</div>

Nexus Runtime is an infrastructure layer that makes agent execution **durable, auditable, and portable**. The core state machine, event store, and recovery engine are stable after multiple design iterations since early 2026. Enterprise features (Kubernetes, Temporal) are in active development.

**Core principle:** The event log is the source of truth. State is a materialized view. Workers are stateless. The Kernel owns causality.

> 📖 **Documentation:** [Book](https://fengrru.github.io/nexus-runtime/) · [API (docs.rs)](https://docs.rs/nexus-core) · [Protocol Spec](https://fengrru.github.io/nexus-runtime/protocol/overview.html) · [ADRs](https://fengrru.github.io/nexus-runtime/adr/ADR-001-deterministic-runtime.html)

---

## What It Does

Nexus takes a user intent (plain English), plans via LLM, dispatches workers via JSON-RPC over stdio, checkpoints every action to an append-only event log, and can recover from any crash at any point — without duplicating LLM calls or side effects.

```
User intent
    ↓
LLM generates execution plan (OpenAI / Anthropic / DeepSeek)
    ↓
State machine transitions: Created → Intake → Planning → Planned → Executing
    ↓
Worker spawned via JSON-RPC 2.0 over stdio (Python / Node.js / Rust / WASM)
    ↓
Each action checkpoints to append-only event log
    ↓
Crash at any point → recover from last checkpoint, never re-call LLM
```

### Real-world Demo

```bash
$ export DEEPSEEK_API_KEY="sk-..."

$ nexus run "Read auth.js and user.js, audit security flaws, write report to audit-report.md" \
    --model deepseek-chat

[LLM]    deepseek-chat → 151 in, 777 out, $0.01, 6923ms
         Plan: [read_file(auth.js), read_file(user.js), analyze, write_report]

[WORKER] Spawned PID 12964
         ✅ Step 1/4: read_file  auth.js          [CKPT]
         ✅ Step 2/4: read_file  user.js          [CKPT]
         ✅ Step 3/4: run_command analysis        [CKPT]
         ✅ Step 4/4: write_file audit-report.md  [CKPT]
[OK]     Worker completed → Executing

$ nexus resume <session-id>
[OK] Session recovered  Status: Executing  Causal check: true  Replay check: true
```

---

## Architecture

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

---

## Crates

| Crate | Purpose | Status |
|-------|---------|--------|
| `nexus-core` | State machine, events, recovery, LLM proxy, side-effect guard, vault, WASM sandbox | ✅ Stable |
| `nexus-event-store` | EventStore trait + SQLite and PostgreSQL implementations | ✅ Stable |
| `nexus-rpc` | JSON-RPC 2.0 codec over stdio (NDJSON) | ✅ Stable |
| `nexus-security` | HMAC-SHA256 capability tokens, sandbox tiers | ✅ Stable |
| `nexus-cli` | CLI binary (`run`, `status`, `log`, `resume`, `suspend`, `archive`, `export`, `import`) | ✅ Stable |
| `nexus-scheduler` | Local, Docker, Kubernetes worker schedulers | ⚡ Beta |
| `nexus-coordinator` | Multi-agent coordination (propose/vote/commit/converge) | ⚡ Beta |
| `nexus-temporal` | Temporal durable execution adapter | ⚡ Beta |
| `phoenix-tests` | Acceptance tests — 8 invariants, 6 kill-9 tests | ✅ Stable |

> See [**API docs**](https://docs.rs/nexus-core) for full module documentation.

---

## Design Invariants

| Constraint | Enforcement |
|------------|------------|
| `transition()` is a pure function | No async, no I/O, no clock, no random — verified by `cargo test` |
| Event log is append-only | No UPDATE/DELETE on events table — enforced at schema level |
| Deterministic serialization | `BTreeMap` for maps, `u64` for timestamps/currency, `rmp-serde` (MessagePack) |
| Workers are stateless | No persistent memory, no network, no direct LLM API access |
| LLM outputs are externalized events | Results cached; never re-called during recovery |
| Side effects are two-phase | Intent → Validate → Execute → Commit. Classification: Pure/Idempotent/Reversible/Irreversible |
| Phoenix gate | All 8 invariants must pass before release |

---

## Supported LLM Providers

| Provider | API Base | Model Examples | Env Variable |
|----------|----------|---------------|--------------|
| OpenAI | `api.openai.com` | `gpt-4o`, `gpt-4o-mini` | `OPENAI_API_KEY` |
| Anthropic | `api.anthropic.com` | `claude-3.5-sonnet` | `ANTHROPIC_API_KEY` |
| DeepSeek | `api.deepseek.com` | `deepseek-chat`, `deepseek-reasoner` | `DEEPSEEK_API_KEY` |

Without an API key, the LLM proxy falls back to simulation mode.

---

## Deployment Modes

| Mode | Storage | Scheduler | Requirements | Status |
|------|---------|-----------|--------------|--------|
| **Lite** | SQLite (WAL) | Local process | Zero dependencies | ✅ Ready |
| **Pro** | PostgreSQL | Docker | Docker daemon | ⚡ Beta |
| **Enterprise** | PostgreSQL + Temporal | Kubernetes | K8s cluster | 🚧 Roadmap |

All modes share identical protocol semantics and state machine behavior.

---

## Quick Start

```bash
# Build (Rust 1.80+)
cargo build --bin nexus

# Run your first session
./target/debug/nexus run "read the README and summarize it"

# With a real LLM
export DEEPSEEK_API_KEY="sk-..."
./target/debug/nexus run "analyze auth.js for security flaws" --model deepseek-chat

# Crash recovery
./target/debug/nexus resume <session-id>
```

```bash
# Docker deploy
docker-compose -f docker-compose.lite.yml up       # Lite: SQLite, zero deps
docker-compose -f docker-compose.pro.yml up         # Pro: PostgreSQL + Redis
```

> 📚 Full setup guide: [Getting Started](https://fengrru.github.io/nexus-runtime/tutorials/getting-started.html)

---

## State Machine

```
CREATED → INTAKE → PLANNING → PLANNED → EXECUTING → CHECKPOINTING → EXECUTING → ...
                                        ↘ CONVERGING → REFLECTING → COMPLETED
                                        ↘ FAILED

Any state → HumanApprovalRequested → BLOCKED → HumanApproved → EXECUTING
Any state → SessionSuspended → CHECKPOINTING → SessionResumed → EXECUTING
Any state → SessionArchived
```

12 session states, 25+ event types, all transitions enforced by the `transition()` pure function.

---

## Worker Protocol

Workers communicate via **JSON-RPC 2.0 over stdio** with NDJSON framing:

```
Kernel → Worker:  execute { task_id, intent, capabilities, timeout }
Worker → Kernel:  checkpoint { step_index, actions, artifacts }
Worker → Kernel:  result { status, artifacts, metrics }
Kernel → Worker:  cancel { task_id, reason }
```

Workers have no network access, no persistent state, and receive capability tokens for every action.

---

## SDK Quick Start

### Python

```python
from nexus import NexusRuntime, Event, Budget

runtime = NexusRuntime()

# Open a session
session = runtime.session("refactor auth to JWT")
session.budget.set_limit_usd(5.00)

# Feed events to the state machine
session.intake(source="python-sdk")
session.parse(intent_graph={"nodes": {}})

# Plan with LLM (returns structured JSON plan)
plan = session.plan_with_llm(model="deepseek-chat")
print(f"Plan: {plan}")

# Execute
session.commit_plan()
session.mark_dependencies_met()

# Check status
print(f"Status: {session.status}")
print(f"Budget: {session.budget.remaining_cents()} / {session.budget.limit_cents} cents")
```

### Node.js

```javascript
const { NexusRuntime } = require('nexus-runtime');

const nexus = new NexusRuntime();

async function main() {
  const session = await nexus.session('audit security in auth.js');

  await session.intake({ source: 'node-sdk' });
  await session.parse({ intent_graph: {} });

  // Plan via LLM
  const plan = await session.planWithLLM({ model: 'gpt-4o' });
  console.log('Plan:', plan.slice(0, 200));

  // Commit and execute
  await session.commitPlan();
  await session.markDependenciesMet();

  console.log('Status:', session.status);
}

main().catch(console.error);
```

### Rust

```rust
use nexus_sdk::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = SessionDriver::new(
        SessionId::new(),
        store,
        LlmProxy::new(b"signing-key".to_vec()),
    );

    session.intake("refactor auth", "rust-sdk").await?;
    session.parse(IntentGraph::default()).await?;

    let plan = session.plan_with_llm("claude-3.5-sonnet", "Plan this task").await?;
    println!("Plan: {}", plan.content);

    session.commit_plan(Frontier::empty()).await?;
    session.mark_dependencies_met().await?;

    Ok(())
}
```

---

## Causal Consistency

Every event carries a vector clock (`BTreeMap<SessionId, u64>`). The state machine enforces monotonicity — events must have a causal vector ≥ the current state. This guarantees:

- **Happens-before** ordering across distributed sessions
- **Detection of concurrent** conflicting events
- **Deterministic replay** — same events → same state

---

## Testing

```bash
cargo test                           # Unit + integration + property tests
cargo test --package phoenix-tests   # 26 Phoenix acceptance tests (8 invariants, 10 scenarios)
cargo bench --bench benchmarks       # Performance benchmarks
cargo clippy --all-targets           # Zero warnings
cargo deny check                     # License/security audit
```

### Phoenix Invariants

| I-# | Invariant |
|-----|-----------|
| I-1 | State Authority — database passes integrity check |
| I-2 | Checkpoint Identity — checkpoint ID and step survive restart |
| I-3 | Replay Integrity — event replay produces byte-identical state |
| I-4 | Artifact Integrity — BLAKE3 hashes of vault files remain valid |
| I-5 | Determinism Context — seed, model, input_hash preserved |
| I-6 | Cost Integrity — no duplicated LLM calls |
| I-7 | Resume Continuity — execution resumes from step N+1 |
| I-8 | Eventual Consistency — all transitions reconstructable from event log |

### Kill-9 Tests

Recovery is tested at every phase: intake, planning, executing, checkpoint, converging, reflecting — plus worker crash, LLM timeout, side-effect crash, and cross-session resume.

---

## Project Structure

```
nexus/
├── crates/                   # 11 Rust crates
│   ├── nexus-core/           # State machine, types, protocol, recovery
│   ├── nexus-event-store/    # SQLite + PostgreSQL event stores
│   ├── nexus-rpc/            # JSON-RPC 2.0 codec
│   ├── nexus-security/       # Capability tokens, sandbox
│   ├── nexus-scheduler/      # Local, Docker, K8s schedulers
│   ├── nexus-cli/            # CLI binary
│   ├── nexus-metrics/        # Prometheus metrics
│   ├── nexus-coordinator/    # Multi-agent coordination
│   ├── nexus-message-bus/    # Distributed causal bus
│   ├── nexus-temporal/       # Temporal adapter
│   └── phoenix-tests/        # Acceptance test framework
├── workers/                  # Worker runtimes
│   ├── python-worker/        # Python JSON-RPC worker
│   ├── node-worker/          # Node.js JSON-RPC worker
│   └── rust-worker/          # Rust JSON-RPC worker
├── adapters/                 # Agent interface adapters
│   ├── openclaw/             # OpenClaw gateway
│   └── hermes/               # Hermes CLI
├── sdk/                      # Language SDKs
│   ├── python/               # Python SDK
│   ├── nodejs/               # Node.js SDK
│   └── rust/                 # Rust SDK
├── docs/                     # Protocol, architecture, ADRs
├── fixtures/                 # Golden test fixtures (MessagePack)
├── scripts/                  # Build and demo scripts
├── policies/                 # OPA/Rego policy definitions
├── k8s/                      # Kubernetes deployment configs
├── docker-compose.lite.yml   # Lite mode deployment
├── docker-compose.pro.yml    # Pro mode deployment
└── td.md                     # Frozen technical specification
```

---

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-MIT) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development conventions, testing requirements, and PR process. See [SECURITY.md](SECURITY.md) for our security policy and responsible disclosure process. New features follow the [proposal process](proposals/README.md).

## Community

- 💬 [GitHub Discussions](https://github.com/Fengrru/nexus-runtime/discussions) — Questions, ideas, RFCs
- 🐛 [GitHub Issues](https://github.com/Fengrru/nexus-runtime/issues) — Bug reports, feature requests
- 🔒 [Security Policy](SECURITY.md) — Responsible disclosure
