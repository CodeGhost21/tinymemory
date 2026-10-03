# Live CortexDB harness

A real CortexDB server for the `cortexdb` engine's live tests
(`crates/tinymemory-cortex/tests/live_cortexdb.rs`), so the wire is proven
against the server and not only against the HTTP doubles in
`crates/tinymemory-cortex/src/testing/`.

```sh
./scripts/cortexdb-live.sh                          # boot, test, tear down
KEEP=1 ./scripts/cortexdb-live.sh                   # leave it running on :3141
CORTEXDB_VERSION=v0.10.4 ./scripts/cortexdb-live.sh # another server release
```

Or by hand:

```sh
docker compose -f integration/cortexdb/docker-compose.yml up -d --build
TINYMEMORY_LIVE_CORTEXDB_URL=http://127.0.0.1:3141 \
  cargo test -p tinymemory-cortex --test live_cortexdb
docker compose -f integration/cortexdb/docker-compose.yml down --volumes
```

Without `TINYMEMORY_LIVE_CORTEXDB_URL` the live tests skip, so a plain
`cargo test` never needs Docker.

## What runs

- `cortex`: `cortexdb/cortexdb` pinned to `v0.9.9` (override with
  `CORTEXDB_VERSION`), listening on `127.0.0.1:3141` (`CORTEXDB_PORT`) with the
  static bearer `tinymemory-cortex-test` (`TINYMEMORY_TEST_CORTEX_KEY`).
- `mock-inference`: a deterministic OpenAI-compatible double
  (`mock_inference.py`) for CortexDB's embeddings, extraction and answer
  models, so the harness needs no credential. Point `CORTEX_INFERENCE_URL` and
  `CORTEX_INFERENCE_KEY` at a real compatible endpoint to exercise real models;
  the double is a wiring fixture, not a quality benchmark.

## What the tests prove

- The shared conformance suite (`tinymemory_conformance::run`) passes against
  the live server.
- A document, a conversation with a tool call and a learning, each with its
  metadata, store and list back; metadata filters narrow; hybrid `fetch` finds
  the document; `recall` (CortexDB's answer route) answers with citations;
  `tinymemory_context::compile` builds a `context.md` that carries the
  learning; and a filter `forget` removes all three.

OpenHuman runs its own end-to-end test against this harness through the real
core binary (`scripts/test-memory-cortexdb-live.sh` in that repository).
