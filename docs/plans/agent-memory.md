# Plan: agent memory lifecycle

**Spec:** [../specs/agent-memory.md](../specs/agent-memory.md)

## Goal

Ship the standard layout, holistic recall, `AgentMemory`, `Brain` and
background jobs over any engine. Make CortexDB serve them natively, through
accepted writes and `v1/beliefs/build`.

## Non-goals

- Model calls.
- Task spawning.
- A hosted belief-build route, which does not exist yet.

## Tasks, in order

Each task starts from a failing test.

1. **Contract** (`crates/tinymemory-api`)
   - `namespace/mod.rs`: `SegmentKind::Source`, `Namespace::source`,
     `Namespace::child`, with tests in `namespace/mod_tests.rs`.
   - `write/mod.rs`: `WaitFor`, `WriteOptions`.
   - `consolidate/mod.rs`: the request, receipt, status and `Consolidation`,
     with tests in `consolidate/mod_tests.rs`.
   - `engine/mod.rs`: the `store_with` and `consolidate` defaults, and the
     `EngineDescriptor::consolidation` field.
   - `conformance/reference/distil.rs`: the reference engine's
     deterministic beliefs.
   - `conformance/suite/lifecycle.rs`: the `store_with` and `consolidate`
     checks.
   - `tests/conformance_reference.rs`: the faults `AcceptedWrongId`,
     `ConsolidateOffPromise`, `ConsolidateUndeclared` and
     `ConsolidateUnvalidated`, plus an engine without consolidation that
     still passes.
2. **CortexDB** (`crates/tinymemory-integrations/src/cortex`)
   - `log/write.rs` and `engine/store.rs`: `WaitFor` threaded through. An
     accepted write drops `?wait=indexed` and the waits.
   - `descriptor/mod.rs`: the `Route::BuildBeliefs` route and the
     `consolidation` values per wire.
   - `engine/scopes.rs`: `held` scopes.
   - `engine/consolidate.rs`, with tests in `consolidate_tests.rs`.
   - `testing/routes.rs`: the double serves `/v1/beliefs/build` and records
     the bodies it receives.
   - `envelope/mod_tests.rs`: a source scope round trip.
3. **Holistic recall** (`crates/tinymemory-tools/src/recall`)
   - `types.rs`, `gather.rs` (read, then settle), and `render.rs` (moved
     from `context/compile`, generalised to prose and lines).
   - `context/compile/mod.rs` rebuilt on `recall::run`, with the existing
     tests green.
4. **Layout, brain, background, lifecycle** — `layout/` with `BrainSource`,
   then `brain/`, `background/` and `lifecycle/`, each with its
   `mod_tests.rs`.
5. **Integrations**
   - `brain/` behind the new `brain` feature.
   - `cortex/lifecycle_tests.rs`, running over both doubles.
   - `tests/live_cortex_lifecycle.rs`, added to `scripts/cortexdb-live.sh`.
6. **Examples**
   - `tinymemory-tools/examples/{agent_loop,brain}.rs`.
   - `tinymemory-integrations/examples/cortex_agent.rs`, with its markdown
     and PDF fixtures.
7. **Docs**
   - This plan, the spec, and `docs/architecture/lifecycle.md`.
   - Updates to the namespace, wire, tools and overview pages, and to the
     READMEs.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo run -p tinymemory-tools --example agent_loop
./scripts/cortexdb-live.sh   # needs Docker
```

## Checklist

- [x] Contract and conformance
- [x] CortexDB accepted writes and belief builds
- [x] Holistic recall, with `context.md` rebased
- [x] Layout, brain, background, lifecycle
- [x] Integrations brain helper, doubles test, live test
- [x] Examples
- [x] Docs
