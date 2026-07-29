# Nexus Runtime — Proposal Process

Significant changes to Nexus Runtime (new crates, protocol changes, architecture shifts) require a proposal before implementation. This ensures design decisions are documented, reviewed, and aligned with the project's determinism and causality guarantees.

## When to Write a Proposal

**Required for:**
- New workspace crates
- Changes to the `transition()` state machine contract
- New event types that alter the state transition graph
- Protocol changes (JSON-RPC method additions, NDJSON framing changes)
- New deployment modes or scheduler backends
- Breaking changes to public API (Rust, Python, Node.js SDKs)
- Security model changes

**Not required for:**
- Bug fixes (just open a PR with tests)
- Performance improvements within existing semantics
- Documentation improvements
- Test additions
- Dependency version bumps (Dependabot handles these)

## Process

1. **Copy the template** below into `proposals/NNNN-short-title.md` (use the next available number).
2. **Fill in all sections** — incomplete proposals will be asked to complete.
3. **Open a PR** with just the proposal file against `main`.
4. **Review & discuss** — at least one maintainer must approve.
5. **Accepted** proposals merge into `proposals/accepted/`.
6. **Implementation** PRs reference the proposal number in commit messages.

## Template

```markdown
# Proposal NNNN: [Short Descriptive Title]

**Status:** Draft | Review | Accepted | Rejected | Implemented
**Author:** @github-handle
**Date:** YYYY-MM-DD
**PR:** link (after proposal PR is created)

## Motivation

Why is this needed? What problem does it solve? Link to related issues or ADRs.

## Design

Describe the proposed change in detail. Include:

- **Architecture:** Which crates/modules are affected?
- **API Changes:** New types, new methods, breaking changes.
- **Determinism Impact:** Does this affect `transition()` determinism? How is it preserved?
- **Causal Consistency:** Does this introduce new causal relationships? How are vector clocks handled?

## Alternatives Considered

What other approaches were evaluated and why were they rejected?

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
|      |           |        |            |

## Phoenix Impact

Does this require new Phoenix invariants? Does it change existing invariant behavior?

## Implementation Plan

- [ ] Step 1: ...
- [ ] Step 2: ...
- [ ] Step 3: ...
```

## Accepted Proposals

| # | Title | Status |
|---|-------|--------|
| — | No accepted proposals yet | — |
