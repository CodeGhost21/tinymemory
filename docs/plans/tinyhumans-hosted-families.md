# Plan: optional families on the TinyHumans hosted wire

Implements [the specification](../specs/tinyhumans-hosted-families.md). The
spec is the source of truth for behavior; this plan is the order of work.

## Assumptions

- The backend forwards `labels=` on `GET memory/events`, serves
  `GET memory/events/{id}` and forwards `prefix` on `GET memory/scopes`
  (backend `origin/main`, the same memory surface production runs).
- The record format does not change (openhuman#6718, D3 open).
- The Direct wire is out of scope: no request, record or capability of it
  changes.

## Layout

New code lives under `crates/tinymemory-remote/src/cortex_provider/families/`:

| File | Holds |
| --- | --- |
| `mod.rs` | module docs and wiring |
| `labels.rs` | the lookup-label digest |
| `scopes.rs` | bookkeeping scope names |
| `records.rs` | the keyed record layer: put, live, remove, clear, by-label listings |
| `goals.rs` | `MemoryGoals` |
| `tool_rules.rs` | `MemoryToolMemory` |
| `documents.rs` | `MemoryDocuments` |
| `relevance.rs` | the query score estimate |
| `sources.rs` | `MemorySourceSink` |
| `maintenance.rs` | `MemoryMaintenance` |

Each file's tests live beside it in `<file>_test.rs`, wired with
`#[cfg(test)] #[path = "…"] mod …;`. Test-only builders go in
`test_support.rs` files, never inline in production files.

## Tasks

1. **Cortex primitives** (`cortex.rs`).
   - Test first: a labelled listing sends exactly one `labels=`; an event read by
     id answers `None` on a 404 and for an id outside `[A-Za-z0-9_-]{1,128}`;
     removal batches ids 100 at a time and never sends `confirm_all`.
   - Then:
     - generalise `events` to take an optional label;
     - generalise `append_entry` to carry labels, provenance and inert
       directives through `append_keyed`;
     - give `forget_events` a note and batching;
     - add `event_by_id` and a raw, prefix-filtered scope listing.
2. **Hosted double** (`hosted_test.rs`).
   - Test first: the double refuses a repeated `labels=`.
   - Then:
     - echo `context` on experience;
     - filter events by label;
     - serve `GET memory/events/{id}`;
     - filter scopes by prefix;
     - record forgets.
3. **Record layer** (`families/{labels,scopes,records}.rs`). Test first:
   - labels are fixed-length and comma-free, and the key, session and source
     lookups stay apart;
   - a bookkeeping scope is reachable from no namespace;
   - a rewrite retires exactly the older versions;
   - a remove leaves nothing recallable;
   - a label miss in a user namespace falls back to the whole scope;
   - a retire failure does not fail the write;
   - provenance round-trips;
   - the Direct wire writes exactly what it wrote before.
4. **Goals** (`families/goals.rs`). Test first:
   - an unwritten document reads empty;
   - a write replaces the whole document;
   - an unreadable document is `Backend`;
   - the document is invisible to `namespaces` and `list`.
5. **Tool rules** (`families/tool_rules.rs`). Test first:
   - the rule lands under `tool-<tool>` / `rule/<id>` and is readable through
     `get`;
   - rules sort by priority, then updated time;
   - a delete under another tool is a miss;
   - an empty id is `Invalid`.
6. **Documents** (`families/documents.rs`, `families/relevance.rs`).
   Test first:
   - the conformance round trip;
   - a plain `store` over a document drops its details;
   - list shape and order;
   - `list_namespaces`;
   - delete by id;
   - clear;
   - query scoring and context text;
   - recall without a query.
7. **Sources** (`families/sources.rs`). Test first:
   - placement per kind;
   - text and provenance;
   - a re-sent unchanged item is skipped;
   - an empty item is skipped;
   - a mutable item's rewrite retires its old version;
   - one wait per scope;
   - pacing;
   - a mid-batch failure keeps its class and says how far it got;
   - `forget_source`;
   - `forget_matching`: `Source`, unknown kind, `Chunk` in and out of sources,
     and the unsupported arms.
8. **Maintenance** (`families/maintenance.rs`). Test first:
   - the upkeep reports;
   - a healthy probe diagnoses healthy;
   - each probe failure maps to its code and class;
   - the cache holds for 60 s after a healthy answer and 5 s after a failure,
     under a test clock.
9. **Capabilities and accessors** (`cortex_provider/operations.rs`). Test
   first: hosted advertises the five families and `audit_provider` holds on
   both wires; Direct is unchanged.
10. **Docs.**
    - the crate README's capability table;
    - `lib.rs`'s claim that hosted has the same capabilities as `cortex`;
    - the hosted spec's non-goals;
    - this plan's checklist.
11. **Live** (`tests/live_remote_engines.rs`). The goals, tool rule, document
    and source round trips, gated on the existing `TINYMEMORY_TEST_TINYHUMANS_*`
    variables.

## Verification

Focused, while iterating:

```sh
cargo test -p tinymemory-remote --features tinyhumans cortex_provider::families
cargo test -p tinymemory-remote --features tinyhumans hosted
```

Full, before review, from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

## Checklist

- [ ] 1 Cortex primitives
- [ ] 2 Hosted double
- [ ] 3 Record layer
- [ ] 4 Goals
- [ ] 5 Tool rules
- [ ] 6 Documents
- [ ] 7 Sources
- [ ] 8 Maintenance
- [ ] 9 Capabilities
- [ ] 10 Docs
- [ ] 11 Live
