# Nexus Runtime Documentation

Welcome to the Nexus Runtime documentation. Nexus is a **causally-consistent execution substrate** for autonomous agent systems, built in Rust.

## What You'll Find Here

| Section | Content |
|---------|---------|
| **[Getting Started](tutorials/getting-started.md)** | Install, run your first session, crash recovery demo |
| **[Architecture](architecture.md)** | Layer model, deployment modes, crate map |
| **[Protocol Specification](protocol/overview.md)** | State machine, event schema, serialization rules, worker protocol |
| **[ADR](adr/ADR-001-deterministic-runtime.md)** | Architecture Decision Records — the "why" behind design choices |
| **[API Reference](https://docs.rs/nexus-core)** | Rust crate docs on docs.rs |

## Core Concepts

```
┌─────────────────────────────────────────────┐
│  User Intent (plain English)                │
│  "audit auth.js for security flaws"         │
└──────────────────┬──────────────────────────┘
                   ↓
             LLM generates plan
                   ↓
    State Machine: Created → Intake → Planning
         → Planned → Executing → Checkpointing
                   ↓
    Worker spawned via JSON-RPC 2.0 / stdio
    (Python / Node.js / Rust / WASM)
                   ↓
    Every action → append-only event log
                   ↓
    Crash? → recover from last checkpoint
             (never re-call LLM, never duplicate side effects)
```

**Key principle:** The event log is the source of truth. State is a materialized view. Workers are stateless. The Kernel owns causality.

## Quick Links

- [GitHub Repository](https://github.com/Fengrru/nexus-runtime)
- [Rust API Docs (docs.rs)](https://docs.rs/nexus-core)
- [Protocol Specification](protocol/overview.md)
- [Contributing Guide](https://github.com/Fengrru/nexus-runtime/blob/main/CONTRIBUTING.md)
