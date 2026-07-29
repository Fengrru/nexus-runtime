# Changelog

All notable changes to Nexus Runtime are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **State Machine**: `transition()` pure function with 12 session states, 25+ event types, and causal vector enforcement
- **Event Store**: SQLite (WAL mode) and PostgreSQL implementations with full schema (9 tables, foreign keys, STRICT mode)
- **Recovery Engine**: Full event replay with 7-step recovery protocol, $N$ crash-recovery `kill -9` tests across 6 execution phases
- **Phoenix Acceptance Test Framework**: 8 design invariants with 26 tests (unit + property + golden + kill-9 + migration)
- **Side-Effect Guard**: Two-phase commit with 4 safety classifications (Pure / Idempotent / Reversible / Irreversible) and compensation data for reversible side effects
- **LLM Proxy**: Multi-provider support (OpenAI, Anthropic, DeepSeek) with prompt-hash idempotency caching and automatic simulation fallback when no API key is set
- **Content Vault**: BLAKE3 content-addressed artifact storage with integrity verification on recovery
- **Worker Protocol**: JSON-RPC 2.0 over stdio with NDJSON framing — workers are stateless, network-isolated, capability-token-gated
- **Worker Spawner**: Real `fork`/`exec` process management for Python, Node.js, Rust workers with `kill -9` simulation support
- **CLI**: Full session lifecycle commands (`run`, `status`, `log`, `resume`, `suspend`, `archive`, `export`, `import`, `inspect`)
- **Security**: HMAC-SHA256 capability tokens, sandbox tiers, path traversal protection, `Zeroizing` credential wipe
- **WASM Sandbox**: Real `wasmtime` runtime with fuel metering and epoch interruption for untrusted tool execution
- **Deterministic Serialization**: `BTreeMap` throughout, `rmp-serde` with StructMap, verified by golden fixtures
- **Causal Consistency**: Vector clocks (`BTreeMap<SessionId, u64>`) with monotonicity enforcement, happens-before pruning, and conflict detection
- **Cargo-deny**: License and security advisory enforcement in CI
- **ADR records**: 5 Architecture Decision Records documenting core design trade-offs

### Infrastructure
- **CI/CD**: GitHub Actions matrix testing (Ubuntu, Windows, macOS) with clippy, rustfmt, benchmarks, llvm-cov, cargo-deny, Python/Node.js SDK smoke tests
- **Docker Compose**: Lite mode (SQLite) and Pro mode (PostgreSQL + Docker) deployment manifests
- **Kubernetes**: Enterprise deployment configuration for K8s clusters
- **OPA/Rego Policies**: Budget governance and capability restriction policies

### SDKs
- **Python SDK**: Session lifecycle, budget management, LLM planning, event intake
- **Node.js SDK**: Session lifecycle with async API
- **Rust SDK**: Re-exports `nexus-core` with `SessionDriver` API

### Adapters
- **OpenClaw Gateway**: HTTP session bridging adapter
- **Hermes CLI**: File-based checkpoint persistence and cross-tool session transfer

### Experimental / Beta
- **nexus-coordinator**: Multi-agent coordination with propose/vote/commit/converge and quorum policy
- **nexus-message-bus**: Distributed causal message bus (broadcast, gossip, TTL, causal ordering)
- **nexus-temporal**: Temporal.io durable execution adapter with EventStore implementation
- **nexus-scheduler**: Docker (Bollard) and Kubernetes (kube-rs) worker schedulers
- **nexus-metrics**: Prometheus metrics endpoint for events, transitions, workers, LLM calls, entropy

## [1.0.0] — 2026-06-01

Initial release: core state machine, event store, recovery engine, worker fabric, and Phoenix test suite.
