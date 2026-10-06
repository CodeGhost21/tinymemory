# CortexDB engine: the wire

What `CortexEngine` sends to CortexDB and how it lays an item out as events.
Part of the CortexDB engine docs: [overview and transport](cortex.md) ·
this page · [operation flows](cortex-flows.md). The source is
`crates/tinymemory-integrations/src/cortex/`.

CortexDB is an append-only event log with ranked recall and a grounded answer
route. TinyMemory stores each item as one or more events in that log, and
reads them back through the listing and recall routes.

## Two wires

One engine type, `CortexEngine`, speaks two HTTP surfaces. `CortexWire`
selects the surface; `CortexWire::path` is the only place a route name lives.

| | `Direct` (`cortexdb`) | `TinyHumans` (`tinyhumans`) |
| --- | --- | --- |
| Constructor | `CortexEngine::direct(endpoint, CortexCredential)` | `CortexEngine::tinyhumans(base_url, Arc<dyn BearerSource>)` |
| Default endpoint | `https://api-v1.cortexdb.ai` (`CORTEX_API_ENDPOINT`) | `https://api.tinyhumans.ai` (`TINYHUMANS_API_ENDPOINT`) |
| Route prefix | `/v1/*` | `/memory/*` |
| Success body | bare JSON | `{"success": true, "data": ...}`; `data` is unwrapped |
| Failure body | any text (an excerpt is kept) | `{"success": false, "error": "...", "errorCode": "CODE"}` |
| Credential | API key (static) or a bearer source | bearer source (session JWT or `tiny_live_` key) |
| `X-Cortex-Actor` | learned from `v1/auth/whoami` | not sent (the backend names the actor) |
| Write extras | `?wait=indexed`, a bulk route | none: one event per request, an `Idempotency-Key` claim |
| Health route | `v1/admin/health` | none: lists one scope under a prefix |

Both descriptors declare `fetch_modes = [Hybrid]`. CortexDB's recall body
accepts only `scope`, `query`, `budgets`, `view`, `include`, `temporal` and
`filters`; nothing switches between lexical and embedding retrieval, so
declaring `Keyword` or `Vector` would promise a ranking the wire cannot ask
for. Both fail with `Error::Unsupported` before any request.

### Routes

| Logical route | Direct | TinyHumans | Method |
| --- | --- | --- | --- |
| Experience (append one event) | `v1/experience` | `memory/experience` | POST |
| Bulk (append an ordered batch) | `v1/experience/bulk` | `memory/experience` (never used for a batch) | POST |
| Events (list a scope) | `v1/events` | `memory/events` | GET |
| Recall (build a pack) | `v1/recall` | `memory/recall` | POST |
| Forget | `v1/forget` | `memory/forget` | POST |
| Answer | `v1/answer` | `memory/answer` | POST |
| Health | `v1/admin/health` | `memory/scopes` | GET |
| Scopes (registered scopes under a prefix) | `v1/scopes/list` | `memory/scopes` | GET |
| Build beliefs (one scope) | `v1/beliefs/build` | none (never sent) | POST |
| Whoami (Direct only) | `v1/auth/whoami` | | GET |

The endpoint is joined with the route, so a base URL with a path prefix keeps
it (a trailing `/` is added when missing).

## Endpoints and their shapes

Only the fields the engine reads or writes are listed. Unlisted response
fields are ignored.

### Append: `experience` and `bulk`

Request body (one event). It is the same on both wires:

```json
{
  "scope": "app:tinymemory/agent:researcher/app:documents",
  "modality": "document",
  "idempotency_key": "tm3:<the first 56 hex digits (224 bits) of the SHA-256 of this body without the key>",
  "content": { "kind": "message", "role": "user", "text": "<envelope JSON, see below>" },
  "context": {
    "labels": ["tm:i:<16 hex>", "tm:k:<16 hex>"],
    "observed_at": "2026-01-02T03:04:05+00:00"
  }
}
```

- `modality` is `document` for a document, `observation` for a learning, and
  `conversation` for a turn. `content.role` is `user` for documents and
  learnings and the turn's speaker (`user`, `assistant`, `system`, `tool`)
  for a conversation turn.
- `idempotency_key` is derived from the body, so an identical retry is a
  replay (see [flows: store](cortex-flows.md#store-and-store_many)). The
  answer's `replayed_from_idempotency` is read; absent counts as `false`.
- `context.observed_at` is the turn's `at`, else the item's
  `meta.observed_at`; it is omitted when neither is set.
- `context.labels[0]` is always the item label; the writer relies on that.

Response: `{"event_id": "..."}` (Direct answers `202`, with `status` and
`replayed_from_idempotency` the engine does not read). A response without
`event_id` is `Error::Engine`.

Direct appends with `?wait=indexed`, except for a store that waits only for
acceptance (`store_with` with `WaitFor::Accepted`, which the agent lifecycle
uses for live turns). That store omits the parameter and also skips the
visibility waits, so the call returns once CortexDB has captured the event.
A single event goes to `v1/experience`;
**two or more** go to `v1/experience/bulk` with

```json
{ "items": [ ...experience bodies... ], "ordering": "strict_temporal" }
```

and the response must carry `results` with one entry per request, the last
naming `event_id`. A missing `results`, or a count that differs from the
number sent, is `Error::Engine`. Note that a conversation of one turn, or one
with a single missing turn, goes the single-event route.

TinyHumans always sends one event per request, in order, each under an
`Idempotency-Key` header claim (see [transport](cortex.md#idempotency-claims)).

### List: `events`

```text
GET {events}?scope=<scope>&limit=200[&labels=<l1,l2,...>][&cursor=<cursor>]
```

Response:

```json
{ "items": [ { "id": "evt_1", "scope": "...", "content": { "text": "..." },
               "context": { "labels": [], "observed_at": "..." } } ],
  "has_more": true, "next_cursor": "..." }
```

- Newest first. The engine emits **every event twice** and `limit` counts the
  copies, so a page of 200 holds about 100 distinct events. Readers dedupe.
- `labels` is **one** comma-separated parameter (the hosted backend refuses a
  repeated `labels=`); at most 50 labels per request. An event matches when it
  carries any one of them.
- A next page exists only when `has_more` is `true` **and** `next_cursor` is
  present. A `next_cursor` equal to the cursor just sent is `Error::Engine`
  (a listing that does not advance).
- Unknown query parameters are ignored by the engine, so the paging parameter
  is exactly `cursor`; a misspelling would serve page one for ever.

### Recall: `recall`

```json
{ "scope": "app:tinymemory/app:documents", "query": "...",
  "view": "granular", "include": ["events"],
  "budgets": { "max_tokens": 23592960, "per_layer_limits": { "events": 30 } },
  "filters": { "metadata": { "labels": ["tm:t:<16 hex>"] } } }
```

`filters` is present only when the metadata filter has a labelled field.

- `view` is always `"granular"`: exactly the named scope. CortexDB's public
  recall defaults to `holistic` (the scope, its ancestors and its
  descendants), and a pack at a parent scope is filled from its children in
  storage order (0.10.4 marks it `parent_pack_unranked_sample`), so no read
  relies on either.
- `include` lists `events` first. `budgets.max_tokens` (4000 by default) is a
  cross-layer budget that funds `include`'s layers first, then
  facts > beliefs > episodes > understanding > events, so without it events
  are evicted first. A fetch that wants beliefs sends `["events", "beliefs"]`,
  an answer pack `["events", "facts", "beliefs", "episodes",
  "understanding"]`, a beliefs read `["beliefs"]`.
- `max_tokens` is sent, sized so every event asked for comes back whole: a
  token per byte of the largest event this crate writes (768 KiB), for each
  event (and, in an answer pack, each derived item). The default, 4000
  tokens (about 14 KB), cuts a longer event to a `budget_excerpt` (0.10.4
  API §9.5): a slice of the stored envelope that no longer decodes, so a
  document piece would never be a hit. It also evicts.
  The budget only stops the cutting: `per_layer_limits` still bounds a pack,
  so a pack of `n` events carries at most `n` × 768 KiB of event text (a
  fetch of 5 asks 18 events: at most 13.5 MiB, typically far less). The
  budget is capped at 8 Mi tokens, about 24 to 28 MiB at the 3 to 3.5 bytes
  a token CortexDB 0.10.4 counts (measured on English, CJK and random text),
  so a token per byte is at least three times the room an event needs. Only
  a pack holding more than that (at least 32 events of the largest size, or
  about 100 at the chunk target) gets excerpts again, which do not decode
  and are logged at warn (`log/notes.rs`).
- `temporal` is not sent. `temporal.reference_date` only anchors
  `temporal.natural` (a phrase such as "last 30 days", reduced to a
  capture-time filter) and already defaults to the request time; the field
  that ranks by the time a question refers to is `temporal.refers_during`
  (boost-only, capability `refers_to_v1`). Using it needs the turn's time and
  IANA zone in the contract and the referred date extracted client-side,
  which is follow-up work.
- Every pack's `warnings[]` is logged (debug), with
  `parent_pack_unranked_sample` and any knapsack eviction
  (`diagnostics.knapsack_evictions`, or a `context_contributors` row with
  `evicted_from_layers: true`) at warn, with the scope (`log/notes.rs`).
Response: `{"pack_id": "...", "layers": {"events": [...]}}`. Events in a pack
render their text for a reader as `[role] {...}`; the decoder strips that
prefix. A pack's events are read from `/layers/events` and decoded exactly
like listing events.

For `recall` (the answer path) the budget also names the derived layers:
`events` is `2 * limit`, and `facts`, `beliefs`, `episodes` and
`understanding` share `limit` between them (the remainder goes to the first
ones).

### Answer: `answer`

```json
{ "scope": "...", "question": "...", "use_pack_id": "pack_...",
  "cite_sources": true, "include_context": true,
  "answer_instructions": "..." }
```

Response fields read: `answer` (required, string) and
`diagnostics.answer_model` (optional, becomes `RecallAnswer.model`).

A 404 means the pack is gone: packs live 60 s, and CortexDB drops every
pack it holds once anything is forgotten. Every scope's pack is then built
again and the answer asked from the new chosen pack, up to three rounds in
all (see
[recall](cortex-flows.md#recall), step 5).

`answer_instructions` is the request's instructions when set. When unset,
Direct sends `null` and TinyHumans **omits the key**: its answer schema is
strict (an unknown key, or a `null` instructions, is a 400).

### Forget: `forget`

```json
{ "scope": "...", "layers": ["events"],
  "selector": { "memory_ids": ["evt_1", "evt_2"] },
  "audit_note": "tinymemory: forget" }
```

At most 100 ids per request. The id field is exactly `memory_ids`: an
unrecognised or empty selector means *the whole scope* to CortexDB (an empty
selector needs `confirm_all`, and `confirm_all` beside a selector is refused).
The engine never sends an empty selector and never sends `confirm_all`; a
scope with nothing to remove sends no request at all.

### Scopes: `v1/scopes/list` and `memory/scopes`

```text
GET {scopes}?prefix=<scope prefix>&limit=1000
```

The reader accepts either `{"items": [{"path": "..."}]}` (Direct) or
`{"scopes": ["..."]}` (hosted), and for each entry either a bare string or an
object with `path`. A `404` means "no scope listing" and is treated as no
scopes.

### Build beliefs: `v1/beliefs/build` (Direct only)

`consolidate` resolves its reach and kinds to the kind scopes that CortexDB
has registered. It reads them through `v1/scopes/list` under
`app:tinymemory`, keeping only the scopes the reach admits. It then posts one
request per scope, in order:

```json
{ "scope": "app:tinymemory/source:pdf/app:documents" }
```

CortexDB v0.10 builds within the request, from the facts its enrichment has
already extracted, and answers with what it built:

```json
{ "built": 2, "items": [ … ], "facts_scanned": 4, "events_scanned": 4,
  "reasons": { "no_subject_or_predicate": 2 } }
```

When every answer carries `built`, the receipt is `Completed` with the counts
summed in `built`. An answer naming a job instead (`job_id`, `build_id` or
`id`) means the build was queued: the receipt is `Started` with the handles.
A build takes seconds per scope with a real model (8–30 s for a whole
scenario in [the eval](../evals/agent-memory.md)), so it belongs off the turn.
Each build is sent once and never retried: a host can always ask again.

The beliefs land in a derived layer, read two ways:

- **Inside a fetch** (`FetchRequest::beliefs > 0`): each scope's recall
  pack also carries `"beliefs": N` in `per_layer_limits`. One pack, and one
  query embedding, serves both the events and the beliefs. This is how
  holistic recall reads them.
- **`beliefs` with a query** (`engine/beliefs.rs`), for a caller that wants
  beliefs alone: one recall per scope held in reach, with a budget for the
  `beliefs` layer only:

  ```json
  { "scope": "…", "query": "…",
    "budgets": { "max_tokens": 6291456,
                 "per_layer_limits": { "events": 0, "facts": 0, "episodes": 0,
                                       "understanding": 0, "beliefs": 8 } } }
  ```

- **Without one:** `GET v1/beliefs?scope=…&limit=…` per scope, ordered most
  confident and then newest. The hosted wire has no listing and answers
  none.
- **Each belief** (`{id, scope, claim: {subject, predicate, object},
  stance, confidence, valid_from}`) becomes a `Learning` hit:
  - its text is `subject predicate object`, with the predicate's
    underscores as spaces;
  - it is tagged `belief`, at its scope's node;
  - only `supported` and `contested` stances are read, and a contested
    belief says so.

The answer route reads the same layer, so a `SectionQuery::Answer` section
(a compaction summary, a `context.md` brief) uses beliefs as well. Fetch
and list are unchanged: they decode only this crate's events. The TinyHumans backend has no such route; its descriptor declares
`Consolidation::Scheduled`, and `consolidate` sends nothing.

### Health

Direct: `GET v1/admin/health`. TinyHumans has no health route, so it lists one
scope under a prefix the engine never writes:
`GET memory/scopes?prefix=tmh%3Aprobe&limit=1`. The memory API refuses a
prefix that is not `type:id` segments (a bare word is a 400, which would
report a healthy service as broken). This proves reachability and the
credential in one round trip. `Error::Unavailable` is `Degraded`, any other
failure is `Down`; the reason keeps the message head and withholds the
backend's own text (everything after a spaced em-dash).

### Whoami (Direct only)

`GET v1/auth/whoami` returns `{"caller": "user:local"}`. See
[the actor header](cortex.md#the-actor-header).

## Scope layout

Every item lives in the scope of its **kind** at its **namespace node**,
under the TinyMemory root `app:tinymemory` (`envelope::scope_path`):

```text
app:tinymemory/app:{documents,conversations,learnings}                   the root node
app:tinymemory/agent:researcher/app:{documents,conversations,learnings}  an agent
app:tinymemory/team:acme/agent:writer/app:learnings                      a team member
```

So within every node, documents, conversations and learnings are separate
scopes and CortexDB can recall, retain and erase each on its own. The
hosted backend re-roots every scope under the caller's tenant, which is
invisible to the engine except that scope paths it reads back may carry a
prefix: `parse_scope` finds `app:tinymemory` wherever it sits.

**Scope-type mapping.** A namespace segment `kind:id` becomes the CortexDB
scope segment of the same text, using the contract's prefixes:

| Namespace segment | Scope segment type |
| --- | --- |
| Agent | `agent` |
| Team | `team` |
| User | `user` |
| Workspace | `ws` |
| Project | `project` |
| Source | `source` |
| TinyMemory root and each kind leaf | `app` |

These are CortexDB's built-in types, chosen on purpose. From CortexDB v0.10 a
deployment admits only the types in its policy's `allowed_scope_types`
(`org, dept, team, app, user, agent, service, ws, project, global, system,
source` in every shipped preset) and refuses any other with
`422 UNREGISTERED_SCOPE_TYPE`. A private type such as `tm:` would need every
operator to register it first, so the engine uses only types that every
preset allows. A namespace nests at most 8 deep, which keeps the path far
inside the hosted grammar (at most 31 `type:id` segments, as the hosted
double enforces).

**Which scopes a read touches.** `MetaFilter.kinds` picks the kinds and
`MetaFilter.reach` the nodes (`engine/scopes.rs`). Ordering is by kind
(`ItemKind::ALL`) and then namespace, so a cursor can resume by position.

- A reach **without descendants** reads `at` and, when it inherits, each
  ancestor. The nodes are known, so no request is made; a node nothing was
  written to simply lists empty.
- A **subtree reach, or no reach**, needs the nodes below. They are
  discovered once per call from the scopes registered under the TinyMemory
  root (or under the reach's own node), and the root's kind scopes are always
  read.
- Reads are always exact: every pack is `view: "granular"` over one scope,
  so one agent's read never reaches a sibling's scope and no read is a
  parent-scope sample.
- A filter whose `kinds` admits nothing reads no scopes.

## The envelope (v3, and v2)

A learning is one event; a conversation is one event per turn, appended in
order. A document is one event unless its envelope would pass 256 KiB; then it
is one event per piece (see [cortex-chunks.md](cortex-chunks.md)).
CortexDB's experience schema is closed (an unknown field is a 422), and its
`context.labels` are the app-metadata extension point: stored verbatim,
returned on listings and on recall's `layers.events`, and not counted toward
the 1 MiB text limit.

**v3 (written now).** An event's `content.text` is the item's own text: the
document body or piece, the turn's text, or the learning's statement. This is
what CortexDB extracts from and splits for search at sentence boundaries,
which a JSON text lacks. Its `context.labels` are, in order:

- the lookup labels (below), `tm:i:` first;
- readable labels, each at most 256 bytes or left out: `kind:<kind>`,
  `file:<path>`, and a piece's `page:<n>` or `page:<first>-<last>` and
  `section:<title>`. Never filtered on; no `lang:` label is ever written
  (CortexDB reserves it);
- the **envelope parts**: the envelope below with `"v": 3` and an empty
  `text`, as compact JSON cut at char boundaries into slices of at most 240
  bytes, each written `tm:e:<NN>:<slice>`. `NN` is the part's number as a
  decimal integer, zero-padded to two digits (`00`, `01`, …); an event has
  at most 64 labels, so there are at most 63 parts. A reader orders parts by
  that number, never by the label's text, and needs every number from 0 up.

An event whose text is empty (CortexDB requires message text), or whose
labels would be more than 64, is written as v2 instead.

**v2 (every event before v3).** `content.text` is the whole envelope as JSON,
with `"v": 2`. Both layouts can live in one scope, and every reader takes
either: an event whose labels hold envelope parts numbered `0..n` that join to
a v3 envelope is v3, with its text as the envelope's `text`; otherwise its
text is tried as a v2 envelope. The envelope:

```json
{ "v": 2, "id": "<40-hex fingerprint>", "kind": "conversation",
  "text": "<body | turn text | statement>",
  "meta": { "...": "the item's whole MemoryMeta, on every event" },
  "title": "...", "mime": "...",
  "learning_kind": "preference", "confidence": 0.8, "evidence": "...",
  "turn": { "index": 0, "count": 3, "role": "user", "at": "...", "tool_calls": [] } }
```

| Field | Present on | Meaning |
| --- | --- | --- |
| `v` | every event | `3` in envelope parts, `2` in a v2 text; any other value is ignored |
| `id` | every event | the item id, `StoreItem::fingerprint()` (a content digest) |
| `kind` | every event | `document`, `conversation` or `learning` |
| `text` | every event | body, the turn's text, or the learning's statement |
| `meta` | every event | the whole `MemoryMeta`, including the namespace |
| `title`, `mime` | documents, when set | |
| `learning_kind`, `confidence`, `evidence` | learnings (`evidence` when set) | |
| `turn` | conversation turns | `index` (0-based), `count`, `role`, `at`, `tool_calls` |
| `chunk` | pieces of a chunked document only (a document written whole has none) | `index` (0-based), `count`; optional `pages` (`[first, last]`, only when the text marks pages) and `section` (only when the piece starts under a heading) |

An event that is neither is someone else's and is ignored by every reader. A
v2 text is first tried as written, then without a leading `[role] ` (an older
recall rendered one; 0.10.3 and 0.10.4 return the stored text in
`layers.events`, the marker only in `context_block`). A document whose body is still an unresolved URI is
refused at write time as `Error::InvalidRequest`.

Rebuilding an item from envelopes: a learning, or a document written whole,
takes the first envelope; a chunked document orders its pieces by `index`,
keeps one per index and concatenates their text (the pieces are contiguous
slices, so all of them give back the body exactly, and one gives that piece);
a conversation orders turns by `index` and keeps one per index (so a
duplicated or re-written turn does not repeat, and a turn that was never
written is absent). A learning with no `learning_kind` reads back as `Other`.

## Lookup labels and digests

Each event carries up to eight `context.labels`, each `tm:<tag>:` followed by
the first 16 lowercase hex digits (64 bits) of the SHA-256 of the value:

| Label | Value hashed | On |
| --- | --- | --- |
| `tm:i:` | the item id | every event |
| `tm:t:` | `meta.thread_id` | when set |
| `tm:s:` | `meta.source.id` | when set |
| `tm:r:` | `meta.repo` | when set |
| `tm:w:` | `meta.workspace` | when set |
| `tm:a:` | `meta.agent_id` | when set |
| `tm:l:` | `meta.language` | when set |
| `tm:k:` | the source kind (`meta.source.kind`) | every event |

A label holds a **digest**, not the value, because the engine splits a label
filter on commas and bounds a label's length, and a path or source id may be
long or hold a comma.

Reads use labels two ways:

- **Item lookup.** Replay detection, `get`, conversation assembly and forget
  by id ask the listing for `tm:i:<digest(id)>` labels. Because a label is a
  digest, every hit is re-checked against the envelope's real `id`.
- **Narrowing.** A read whose filter has a labelled field sends **one** label
  filter to narrow server-side: the first set field of thread, source id, repo,
  workspace, agent, language (in that order, most selective first), else the
  filter's source kinds (several `tm:k:` labels, which the engine reads as
  any-of). Only labels of one field may be sent together, since the engine
  keeps events carrying *any* of the labels.

The label only ever narrows. Every reader **always** re-applies the full
`MetaFilter` to the decoded envelope, so a digest collision costs a wasted row
and never a wrong answer. `folder` and `file_path` match as prefixes, which a
digest cannot, so they are never labelled and are filtered client-side only.

## CortexDB behaviours the engine is shaped around

Each was measured against a live CortexDB and was wrong in the first adapter.
The loopback doubles reproduce all of them (see [testing](testing.md)).

- **Append-only.** There is no update route. A reused body `idempotency_key`
  with the same body is a replay for 24 hours, and with another body a 409
  (which this crate's keys, derived from the body, never produce).
  Forget by `memory_ids` releases the keys of what it removes (measured on
  0.10.4; the v1 adapter's notes said the opposite, on an unrecorded build).
- **Accepted is not readable.** An append answers `202` and indexes afterwards.
  The status route and the lifecycle stream are not readiness signals, so the
  engine waits on the listing and on recall itself (see
  [flows](cortex-flows.md#waiting-for-a-write-to-be-readable)).
- **The listing emits every event twice**, and `limit` counts the copies.
- **Unknown query parameters are ignored.**
- **Recall and the listing return different bytes** (`[role] {...}` versus the
  stored text).
- **The forget selector field is `memory_ids`**, and anything else reads as
  empty, which means the whole scope.
- **TinyHumans** rate-limits a user to 300 requests a minute and has no bulk,
  `?wait=indexed` or health route.
