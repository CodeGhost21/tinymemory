# tinymemory-cortex

The CortexDB memory engine for TinyMemory v2. One type, `CortexEngine`,
implements `tinymemory_api::MemoryEngine` over CortexDB's append-only event
log on two wires:

| Engine id | Constructor | Wire | Auth | Default endpoint |
| --- | --- | --- | --- | --- |
| `cortexdb` | `CortexEngine::direct` | `/v1/*`, bare JSON | API key (`CortexCredential`) | `https://api-v1.cortexdb.ai` |
| `tinyhumans` | `CortexEngine::tinyhumans` | `/memory/*`, `{success,data}` envelopes | `BearerSource`, resolved per request | `https://api.tinyhumans.ai` |

Both descriptors declare `fetch_modes = [Hybrid]`: CortexDB's recall body
accepts only `scope`, `query`, `budgets`, `view`, `include`, `temporal` and
`filters`, with no keyword/vector switch. `Keyword` and `Vector` fail with
`Error::Unsupported` before any request.

## Public surface

- `CortexEngine::{new, direct, tinyhumans, with_request_timeout, wire}`
- `CortexWire { Direct, TinyHumans }`, `CortexCredential { Static, Dynamic }`
- `BearerSource` (async `bearer()`), `StaticBearer` (redacted `Debug`)
- `CORTEXDB_ENGINE_ID`, `TINYHUMANS_ENGINE_ID`, `CORTEX_API_ENDPOINT`,
  `TINYHUMANS_API_ENDPOINT`, `cortexdb_descriptor()`, `tinyhumans_descriptor()`
- `Error`/`Result` (the contract's own `tinymemory_api::Error`),
  `error_code`, `is_insufficient_credits`, `INSUFFICIENT_CREDITS_CODE`

## Storage layout

**Scopes.** One per item kind under the TinyMemory root:
`app:tinymemory/app:documents`, `app:tinymemory/app:conversations`,
`app:tinymemory/app:learnings`. The hosted backend also re-roots every scope under
the caller's tenant. `MetaFilter.kinds` picks the scopes read.

Every segment uses CortexDB's built-in `app` scope type. From v0.10 a
deployment admits only the scope types in its policy's `allowed_scope_types`
(`org, dept, team, app, user, agent, service, ws, project, global, system,
source` in every shipped preset) and refuses any other with `422
UNREGISTERED_SCOPE_TYPE`, so a private type such as `tm:` would need every
operator to register it first. `integration/cortexdb/` runs the engine against
a real server (v0.10.4 by default; `CORTEXDB_VERSION=v0.9.9` checks the older
release).

**Events.** A document or learning is one event; a conversation is one event
per turn, appended in order. Each event's `content.text` is a JSON envelope:

```json
{ "v": 2, "id": "<40-hex fingerprint>", "kind": "conversation",
  "text": "<body | turn text | statement>", "meta": { ... MemoryMeta ... },
  "title": "...", "mime": "...", "learning_kind": "...", "confidence": 0.8,
  "evidence": "...",
  "turn": { "index": 0, "count": 3, "role": "user", "at": "...", "tool_calls": [] } }
```

Kind-specific fields appear only when set. Text that is not a v2 envelope is
someone else's event and is ignored. `context.observed_at` carries the turn's
`at` or the item's `meta.observed_at`.

**Labels.** Each event carries up to eight `context.labels`, each a 16-hex
SHA-256 digest: `tm:i:` (item id) on every event, plus `tm:t:` thread,
`tm:s:` source id, `tm:r:` repo, `tm:w:` workspace, `tm:a:` agent,
`tm:l:` language, and `tm:k:` source kind. A read whose filter has a labelled
field sends **one** label filter (`labels=` comma list on events,
`filters.metadata.labels` on recall) to narrow server-side, then **always**
re-applies the full `MetaFilter` client-side. `folder` and `file_path` match
as prefixes, so they cannot be labelled and are filtered only client-side.

## Operations

- **Store.** The item id is `StoreItem::fingerprint()`. The item's events are
  looked up by its label first. If all of them are already there, the store is
  a replay (`replayed: true`) and nothing is written. If only some turns of a
  conversation are present (an earlier store failed part-way), only the
  missing turns are written. Direct writes `v1/experience?wait=indexed`, or for
  a conversation `v1/experience/bulk?wait=indexed` with `ordering:
  strict_temporal`. Hosted writes one event at a time, in order. Every write
  uses a fresh `idempotency_key`, never a content-derived one, because
  CortexDB keeps a forgotten event's key and would swallow a re-store. The
  write then waits for its last event to be readable (see below).
- **List.** Pages the admitted kind scopes in order, newest first. The opaque
  cursor holds the scope, the engine cursor, the offset into that page and the
  last event id, which is enough to drop the engine's duplicate copies across
  page boundaries. A conversation is emitted once, on the page holding its
  turn 0, with its text assembled from all its turns (one label lookup per
  page). Scores are `0`.
- **Fetch (hybrid).** One recall per admitted kind scope with
  `budgets.per_layer_limits.events`. Events are decoded to items and the full
  filter is applied. Each item is kept once, at its best rank, and kinds are
  interleaved rank by rank. The score is `1/(1+rank)`, because CortexDB
  reports none. Conversation hits carry the whole conversation. The cursor is
  an offset into the merged ranking; the next page asks again with a larger
  budget, capped at 1000 events.
- **Recall.** If exactly one kind is admitted, one pack is built over that
  kind's scope. Otherwise the pack is built over `app:tinymemory` with
  `view: "descend"`. The answer route is then called **once** with
  `use_pack_id`. Hosted omits a null `answer_instructions`, because its schema
  is strict; Direct sends `null`. Citations come from the pack's
  `layers.events`, decoded, filtered, one per item, capped at `limit`, with
  `score: None`. `model` is `diagnostics.answer_model`. A pack with no
  decodable events still returns the answer, with no citations.
- **Forget.** `Ids` looks the items' labels up in each kind scope. `Filter`
  (which must be non-empty) walks the admitted scopes and matches the full
  filter. Either way the matched events are then removed with
  `selector.memory_ids`, in batches of 100. An empty selector is never sent,
  and neither is `confirm_all`. `forgotten` counts items.
- **Health.** Direct probes `GET v1/admin/health`. Hosted lists
  `memory/scopes?prefix=tmh:probe&limit=1`. `Unavailable` maps to `Degraded`
  and any other failure to `Down`. The reason keeps the message head and
  withholds the backend's own text.

## Engine behaviours this crate is shaped around

These were measured against a live CortexDB by the v1 adapter. The doubles in
`src/testing/` reproduce all of them.

- **Append-only.** There is no update route. Forget removes events but not
  their idempotency records.
- **Accepted is not readable.** A write first waits until the label-narrowed
  listing carries its event (fatal after 30s). It then waits until ranked
  recall returns it (best-effort, 10s); a recall that is down or slow does not
  fail a write that is already durable. Hosted polling backs off to a 2s
  ceiling and treats 429/5xx while waiting as "not yet".
- **The listing emits every event twice**, and `limit` counts the copies.
  Readers dedupe by event id. A full walk refuses past 500 pages, and a cursor
  that does not advance is an error.
- **Unknown query parameters are ignored**, so paging uses exactly `cursor`.
- **Recall renders text** as `[role] {...}`; the prefix is stripped when
  decoding.
- **The forget selector field is `memory_ids`.** An empty or unrecognised
  selector means the whole scope.

## Transport

- Credentialed cleartext endpoints that are not loopback are refused with
  `Error::Config`.
- The bearer is resolved on every attempt and sent in a header marked
  sensitive. A source failure, a blank token, or a token containing CR/LF is
  `Unauthorized`, and no request is sent.
- Success bodies are capped at 64 MiB and error bodies at 64 KiB.
- Status mapping: 401/403 → `Unauthorized`, 404 → `NotFound`,
  400/413/422 → `InvalidRequest`, 409 → `Conflict`,
  429/500/502/503/504 → `Unavailable`, anything else → `Engine`. Transport
  faults (timeout, DNS, TLS, connect) are `Unavailable`.
- Hosted failures carry the backend's `errorCode` as a `[CODE] ` message
  prefix. **402 is `Engine` with `[USER_INSUFFICIENT_CREDITS]`**: it is not
  transient, so `Unavailable` would invite a retry loop, and it is not a
  credential fault, so `Unauthorized` would send the host to sign in.
  `is_insufficient_credits` detects it.
- Reads (listings, recall) are retried 3 times with 250ms·2ⁿ backoff on
  `Unavailable`. Writes are sent once.
- Hosted writes carry a random `Idempotency-Key` claim, reused across that
  write's own transient retries (up to 3). A 409 on a retry means the earlier
  attempt reached the engine. The write is then looked for, by its exact
  stored text under its item label, until the visibility budget runs out. If
  it is never found, the error is `Unavailable` and says the outcome is
  unknown.

## Tests

`cargo test -p tinymemory-cortex` runs the unit tests and the shared
`tinymemory-conformance` suite against both wires, through loopback doubles
with short test-only timeouts.
