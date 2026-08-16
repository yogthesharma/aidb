Thanks for the pull request.

## Summary

<!-- What changed and why. Link PHASES.md / DESIGN.md if the contract moved. -->

## Test plan

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (build first so CLI tests are not stale)
- [ ] Binding / Studio / stock tests if those faces changed

Do not add an `agents` table, a second store, or Phase 27 (DataFusion) unless a
profile says SQLite is the bottleneck.
