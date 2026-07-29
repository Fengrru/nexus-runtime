## Summary

<!-- Brief description of the changes -->

## Type of Change

- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update
- [ ] Performance improvement
- [ ] Test addition/improvement

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo test -p phoenix-tests -- --nocapture` passes (if core changes)
- [ ] Golden fixtures updated (if serialization format changed)
- [ ] `CHANGELOG.md` updated under `[Unreleased]`
- [ ] New tests added for new functionality
- [ ] No `HashMap` or `HashSet` introduced (determinism invariant)

## Design Review

<!-- For significant changes, describe the design trade-offs and any alternatives considered -->

## Breaking Changes

<!-- If this PR introduces breaking changes, describe them and the migration path -->
