# Contributing to Nexus Runtime

Thank you for your interest in contributing! This document outlines the conventions, tooling, and workflow.

## Development Philosophy

- **Determinism first**: The `transition()` state machine is a pure function. No I/O, no async, no random, no clock reads. Golden fixtures and property tests verify byte-identical replay across independent processes.
- **Event log is immutable**: The `events` table is append-only — no UPDATE or DELETE statements. Schema enforces this.
- **Workers are stateless**: No persistent state, no network access, no direct LLM API access. All external communication flows through the Kernel.

## Getting Started

```bash
# Prerequisites: Rust 1.80+
rustup update stable

# Clone and build
git clone https://github.com/Fengrru/nexus-runtime.git
cd nexus-runtime
cargo build --bin nexus

# Run the full test suite
cargo test --workspace
cargo test -p phoenix-tests -- --nocapture

# Check code quality — must pass with zero warnings
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo deny check

# Or use the Makefile
make lint
make test
make ci
```

## Code Conventions

### Determinism Rules

| Do | Don't |
|----|-------|
| `BTreeMap` for maps | `HashMap` / `HashSet` (blocked by clippy) |
| `u64` for timestamps and currency | `SystemTime`, `f64` for money |
| `rmp-serde` with `StructMap` for serialization | `serde_json` for protocol payloads |
| `blake3::Hasher` for content hashing | Non-cryptographic hashers |

### Module Structure

- `nexus-core`: Types, state machine, protocol, recovery — the deterministic nucleus
- `nexus-event-store`: Persistence trait + SQLite/PostgreSQL implementations
- `nexus-rpc`: JSON-RPC 2.0 codec over stdio
- `nexus-security`: Capability tokens, sandbox enforcement
- `nexus-cli`: User-facing CLI binary

### Testing

Every PR should include tests. Our test hierarchy:

1. **Unit tests** (`#[cfg(test)] mod tests`): Cover individual functions and types
2. **Property tests** (`prop_*` prefix): Verify algebraic laws (commutativity, associativity, transitivity)
3. **Golden tests**: Ensure deterministic serialization matches checked-in fixtures
4. **Phoenix tests**: End-to-end crash-recovery scenarios across all execution phases

Run the Phoenix suite before submitting:
```bash
cargo test -p phoenix-tests -- --nocapture
```

## Pull Request Process

1. Fork the repository and create a feature branch
2. Ensure `cargo fmt`, `cargo clippy`, and `cargo test --workspace` pass
3. Add tests for new functionality
4. Update `CHANGELOG.md` under `[Unreleased]`
5. Submit a PR against `main`
6. CI must pass all 7 jobs (check, test × 3 OS, bench, coverage, security, SDKs)

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add real Anthropic API integration
fix: correct causal vector merge monotonicity
docs: add getting-started tutorial
test: add kill-9 test for checkpoint phase
chore: update dependencies
```

## Architecture Decision Records

Significant design decisions are documented as ADRs under `docs/adr/`. If your change introduces a new architectural constraint, create an ADR following the same format.

## Feature Proposals

For significant changes (new crates, protocol changes, state machine contract changes), follow the proposal process in [`proposals/README.md`](proposals/README.md). This ensures design decisions are reviewed before implementation begins.

## Contributor License Agreement (CLA)

By contributing to Nexus Runtime, you agree that your contributions will be licensed under the same terms as the project (MIT OR Apache-2.0). You retain copyright of your contributions.

For substantial contributions (>50 lines of code), we may ask you to explicitly confirm this via a comment on the PR:

> I confirm that I hold the rights to this contribution and license it under the MIT OR Apache-2.0 license.

## Questions?

Open a GitHub Discussion or issue with the `question` label.

## Community

- **GitHub Issues** — Bug reports and feature requests
- **GitHub Discussions** — Questions, ideas, and general conversation
- **Security** — Report vulnerabilities to [fengru1005@gmail.com](mailto:fengru1005@gmail.com). See [SECURITY.md](SECURITY.md).
