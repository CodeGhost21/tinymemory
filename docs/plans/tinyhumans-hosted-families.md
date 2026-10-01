# Plan: optional families on the TinyHumans hosted wire

Implements [the specification](../specs/tinyhumans-hosted-families.md). The
spec is the source of truth for behavior; this plan is the order of work.

## Assumptions

- The backend forwards `labels=` on `GET memory/events`, serves
  `GET memory/events/{id}` and `GET memory/{facts,beliefs,understanding}`
  (backend `origin/main`, the same memory surface production runs).
- The engine lists a scope newest first, and treats several recall label
  filters as any-of (both measured on a local engine).
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
| `retrieval.rs` | `MemoryRetrieval`, scored by rank |
| `ingest.rs` | `MemoryIngest` |
| `profile.rs` | `MemoryProfile` |
| `episodic.rs` | `MemoryEpisodic` |
| `scoring.rs` | `MemoryScoring` |
| `understanding.rs` | the derived layers as a forest, and its cache |
| `tree.rs` | `MemoryTree` |

Each file's tests live beside it in `<file>_test.rs`, wired with
`#[cfg(test)] #[path = "…"] mod …;`. Test-only builders go in
`test_support.rs` files, never inline in production files.

## Tasks

1. **Cortex primitives** (`cortex.rs`).
   - Test first: a labelled listing sends exactly one `labels=`; an event read by
     id answers `None` on a 404 and for an id outside `[A-Za-z0-9_-]{1,128}`;
     removal batches ids 100 at a time and never sends `confirm_all`.
   - Then:
     - generalise `events` to take an optional label list;
     - label `store` and its tombstone on the hosted wire, so labelled reads
       see the storage tier's writes;
     - generalise `append_entry` to carry labels, provenance and inert
       directives through `append_keyed`;
     - give `forget_events` a note and batching;
     - add `event_by_id`.
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
   first: hosted advertises its families and `audit_provider` holds on
   both wires; Direct is unchanged.
10. **Docs.**
    - the crate README's capability table;
    - `lib.rs`'s claim that hosted has the same capabilities as `cortex`;
    - the hosted spec's non-goals;
    - this plan's checklist.
11. **Live** (`tests/live_remote_engines.rs`). The goals, tool rule, document
    and source round trips, gated on the existing `TINYMEMORY_TEST_TINYHUMANS_*`
    variables.
12. **Retrieval** (`families/retrieval.rs`). Test first: ranks score 1.0 down
    by 0.1 and never below 0.1; namespace recall answers three hits at most,
    with no similarity; a source scope narrows the recall itself, so other
    sources cannot crowd a permitted one out; leaves by id keep only this
    account's events; entities refuse an unknown kind.
13. **Ingest** (`families/ingest.rs`). Test first: two batches from one source
    never share a key; a resend writes nothing; each kind lands in its source
    namespace unless it names one; mail renders its headers; a reingested
    document retires its old version.
14. **Profile, Episodic, Scoring** (`families/{profile,episodic,scoring}.rs`).
    Test first: the provider-facet merge; listings, drops and `LIKE`; turn ids
    rise; a session's reads go by its label; segments open, grow, close and
    summarise; local entity extraction; `embed_text` is `Unsupported`.
15. **Tree** (`families/{understanding,tree}.rs`, and `TreeSummary::preview`
    in `tinymemory-bus`, contract 4.2). Test first: claims read as sentences;
    what the server set aside is left out; each node hangs under its most
    confident citer; a long layer pages and is cut; one reading serves a
    minute; a scoped caller gets no derived nodes; leaves hang under the facts
    citing them and a scoped listing goes by label; the walking members are
    `Unsupported`.

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

- [x] 1 Cortex primitives
- [x] 2 Hosted double
- [x] 3 Record layer
- [x] 4 Goals
- [x] 5 Tool rules
- [x] 6 Documents
- [x] 7 Sources
- [x] 8 Maintenance
- [x] 9 Capabilities
- [x] 10 Docs
- [ ] 11 Live: `live_tinyhumans_serves_its_families` is written; it has not
      been run against a production account yet
- [x] 12 Retrieval
- [x] 13 Ingest
- [x] 14 Profile, Episodic, Scoring
- [x] 15 Tree
