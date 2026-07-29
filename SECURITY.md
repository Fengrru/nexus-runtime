# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 1.0.x   | :white_check_mark: |

## Reporting a Vulnerability

**Do not open a public issue.** Instead, send a detailed report to:

- Email: **fengru1005@gmail.com**

Include:
- Description of the vulnerability
- Steps to reproduce
- Affected components and versions
- Potential impact

You will receive an acknowledgment within 48 hours and a timeline for resolution within 5 business days.

## Security Model

Nexus Runtime enforces several security boundaries:

### Worker Isolation
- Workers communicate exclusively via JSON-RPC 2.0 over stdio — no network sockets, no shared memory
- Each worker receives HMAC-SHA256 capability tokens scoped to the specific task
- Workers cannot access the LLM API directly; all calls are proxied through the Kernel with budget enforcement

### Sandbox Tiers

| Tier | Description | Enforcement |
|------|-------------|-------------|
| `untrusted` | No filesystem access, no network, no subprocesses | WASM sandbox with fuel metering + path traversal guard |
| `readonly` | Read-only access to specified paths | Capability token scope + filesystem path validation |
| `sandboxed` | Read-write within a chroot-like boundary | Path traversal protection + sandbox directory |
| `full` | Reserved for kernel-internal operations | Signature verification required |

### Side-Effect Safety

All external actions are classified into four categories with distinct recovery strategies:

| Class | Examples | Recovery Strategy |
|-------|----------|-------------------|
| **Pure** | `read_file`, `grep`, `calculate` | Safe to replay |
| **Idempotent** | `upsert`, `write_file` | Replay with idempotency key |
| **Reversible** | `edit_file`, `patch` | Compensate then replay |
| **Irreversible** | `send_email`, `git_push`, `deploy` | Query external state before action |

### Data Integrity
- All artifacts are content-addressed with BLAKE3 hashes
- Event payloads carry integrity hashes verified on recovery
- Session state transitions are cryptographically linked via causal vectors
- LLM API keys are zeroized after use (`Zeroizing<String>`)

### Dependency Auditing
- `cargo-deny` runs on every CI push checking for known vulnerabilities (RUSTSEC advisories)
- License compliance enforced for all transitive dependencies
- Multiple crate versions are denied to prevent dependency confusion

## Responsible Disclosure

We follow the [CVD (Coordinated Vulnerability Disclosure)](https://www.cisa.gov/coordinated-vulnerability-disclosure-process) process. Critical vulnerabilities will be addressed with an emergency patch release.
