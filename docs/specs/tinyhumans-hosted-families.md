# Optional families on the TinyHumans hosted wire

## Status and owner

Draft, pending acceptance on openhuman#6718. The first five families shipped
first; the per-turn families, retrieval, ingestion and the derived forest
followed once the product decisions below were taken (D2: recall scores are
ranks; D4: serve every family a hosted account can back). Owner: TinyMemory
maintainers.

Extends [CortexDB via the TinyHumans backend](tinyhumans-hosted-cortex.md),
which stays the source of truth for the wire, auth, errors and idempotency.

## Problem

The `tinyhumans` provider serves the mandatory families plus ingestion and
answers. A host that binds it loses every feature built on another family:

- a host's connector sync has nowhere to write, because there is no source
  sink;
- goals, per-tool rules and titled documents cannot be kept;
- a host's memory health surface reports "does not serve Maintenance" as if
  memory were broken.

In OpenHuman this reads as Composio sync failing on hosted memory, Brain panels
marked "Not available", no episodic or learned-profile memory, an empty Brain
graph, and memory E2E suites that can only run on the local module
(openhuman#6718 acceptance criterion 10).

The backend already exposes what these families need beyond the mandatory
routes: a `labels=` filter on `GET memory/events`, and `GET memory/events/{id}`.
The adapter uses neither today.

## Goals and non-goals

Goals:

- Serve `Goals`, `ToolMemory`, `Documents`, `Sources` (the sink),
  `Maintenance`, `Retrieval`, `Ingest`, `Profile`, `Episodic`, `Scoring` and
  `Tree` over the TinyHumans wire.
- Keep the Direct (`cortex`) wire byte-for-byte unchanged, including its
  advertised capabilities.
- Build on the record format the adapter already writes. A record written by
  these families is readable by the mandatory surface and the other way round.
- Send every keyed hosted write through the existing one-claim retry path, so
  the families keep the write guarantees of `store` and `forget`.
- Keep bookkeeping out of the user's namespaces, recall and derived memory.
- Bound the cost of keyed reads and writes: every hosted call is billed and
  counts against a 300-a-minute limit.

Non-goals:

- A similarity score. The engine ranks recall and returns no score, so hits
  are scored by rank (D2), and nothing claims a similarity.
- Summaries written by a model. The adapter reaches none, so `Tree::summarise`
  folds only nothing.
- `SourceSync`. The pipelines stay in the host, which syncs local sources
  through the sink.
- `People` and `CodingSessions` (privacy), and `Entities`, `Graph`, `Diff`,
  `Chunks` (no server support).
- Changing the backend, the memory API, or the record format (D3).

## Proposed behavior

### Record layer

A keyed record is still one event per version, carrying the adapter's JSON
envelope in `content.text`: key `k`, content `c`, category `cat`, session `s`,
taint `t`, tombstone `d`. On the TinyHumans wire only:

- **Lookup labels** in `context.labels`: `tm:kh:<h>` for the key,
  `tm:srh:<h>` for the source when there is one, and `tm:sh:<h>` for the
  session when there is one. `<h>` is the first 16 lowercase
  hex digits of the value's SHA-256: fixed length, and never a comma, which the
  engine's label filter splits on. `store` and its tombstone carry the key label
  too, so a family read of a key sees what the storage tier wrote.
- **Provenance** in the envelope's optional `x.prov` object: `src` (source id),
  `ref` (the item's reference within its source), `doc` (document id). A record
  without provenance carries no `x`, so its text is exactly what `store` writes.
  Provenance sits under `prov` because ingestion already uses `x` for its own
  payload.
- **Inert directives** `{"embed":"none","extract":[]}` on bookkeeping records,
  so the engine neither embeds them into recall nor extracts facts or beliefs
  from them. Content records keep the engine's defaults.

Reads:

- A keyed read lists `GET memory/events?scope=S&limit=200&labels=tm:kh:<h>`,
  pages to the end, drops the engine's duplicate listing entries, and folds
  newest-wins by `wal_offset`, re-checking `k`, so a digest collision or an
  ignored filter cannot return the wrong key.
- In a user namespace, a labelled read that finds nothing falls back to a
  whole-scope walk, because a record written before `store` carried labels has
  none. Every labelled version is newer than every unlabelled one, so a read
  that finds any labelled version needs no walk. Bookkeeping scopes and source
  namespaces are written only by the families and never fall back.
- Exactly one `labels=` parameter is sent per request; the backend refuses a
  repeated one.

Writes:

- Every write is an append through the hosted write path: up to three
  attempts under one `Idempotency-Key` claim, with outcome-unknown recovery.
- A bookkeeping write waits until its event is listed. A content write also
  waits for recall to carry it, like `store`.
- **Supersede-forget.** A write first lists the key's labelled versions. Once
  the new version is readable, those older versions are removed with `POST
  memory/forget` by event id, in batches of at most 100; otherwise recall would
  keep ranking stale versions. A version the write did not see, such as a
  concurrent newer write, is never removed. If removal fails, the write still
  succeeds: the new version is already the one every read returns, and the
  key's next write or removal retires what was left.
- A remove writes a labelled tombstone, waits for it, then removes the older
  versions the same way.
- A clear removes every event in one scope by id, never its children. `POST
  memory/forget` is never sent with `confirm_all`.

### Bookkeeping scopes

Bookkeeping lives in scopes of type `tmi`, which no namespace maps to and which
the adapter's namespace listing drops:

| Scope | Holds |
| --- | --- |
| `tmi:goals` | the goals document |
| `tmi:documents/<namespace scope>` | each document's title, tags and other details |
| `tmi:profile` | the learned profile's facets |
| `tmi:turns`, `tmi:segments` | episodic turns and conversation segments, session-labelled |
| `tmi:episodic-events`, `tmi:segment-embeddings` | events extracted from segments, and segment embeddings |

So bookkeeping never appears in `namespaces`, `list`, `export_page` or
namespace recall. As a consequence, `migrate::copy` moves a document's content
but not its details, and does not move goals, the profile or episodic memory;
`migrate::copy_all` moves those through their families (see
[store-migration.md](store-migration.md)).

### Goals

`goals()` reads the key `goals` in `tmi:goals`. A missing document is the empty
`GoalsDoc`, and an unreadable one is a `Backend` error. `set_goals` replaces the
document whole, as one inert bookkeeping record.

### Tool rules

Rules use the layout the contract already documents, and the one hosts read
through the keyed store directly: namespace `tool_memory_namespace(tool)`, key
`ToolMemoryRule::storage_key(id)`, category `tool_memory`, and the rule as JSON
content. `tool_rules` returns them highest priority first, then most recently
updated. `put_tool_rule` refuses a rule with an empty id or tool name as
`Invalid`. `delete_tool_rule` answers `false` for a missing rule, or for one
held under another tool.

### Documents

- **Every record is a document.** In the embedded engine, `store` writes a
  document, so here every live keyed record in a namespace is one. A record
  without details of its own reads with the embedded engine's defaults
  (title = key, source type `chat`, priority `medium`, empty metadata, times
  from the engine's `recorded_at`) and an id derived from namespace and key.
- **Content.** A document's content is the namespace's own keyed record under
  the document's key, carrying its id in `x.prov.doc`, so `get(namespace, key)`
  returns the body.
- **Details.** The document's details (id, title, source type, priority, tags,
  metadata, created and updated times) are an inert record under the same key
  in `tmi:documents/<namespace scope>`.
- **Stale details.** Details apply only while the content record still
  carries their document id. After a later plain `store` of the key, it reads
  with the defaults again.
- **Ids.** A new document's id is the one the caller supplied, else a stable
  digest of namespace and key. An existing document keeps its id.
- **Replacement.** `put_document` replaces an existing key, so a document has
  one live version.
- **Listing.** `list_documents` answers the contract's camelCase shape, newest
  first. `list_namespaces` names every namespace with at least one live record.
- **Source namespaces.** `sources/…` hold synced items, which the embedded
  engine keeps apart from its documents, so they are never listed as
  documents.
- **Deleting.** `delete_document` finds the key by document id and removes
  both records. `clear_namespace` clears the namespace scope and its details
  scope.
- **Querying.** `query_documents` ranks with engine recall over the namespace
  and keeps document records. Each hit's `score` is an estimate, because the
  engine returns rank but no similarity: `0.3 × (1 − rank/total) + 0.7 ×
  (share of the query's content words found in the key and content)`. The
  breakdown states which part is which.
- **Recalling.** `recall_documents` returns the newest documents, scored by
  freshness, without a query.

### Sources (the sink)

`accept_source_items(source_id, source_kind, items, taint)`:

- **Where items land.**

  | Source | Namespace |
  | --- | --- |
  | `composio`, toolkit `gmail` or `outlook` | `sources/email` |
  | `composio`, toolkit `slack`, `discord`, `telegram` or `whatsapp` | `sources/chat` |
  | anything else | `sources/documents` |

  The toolkit is the part of `source_id` before its first `:`.
- **Records.** Each item is a content record keyed `item:<source_id>:<item_id>`.
  - Its text is the title, then a blank line, then the content, unless the
    content already opens with the title.
  - Provenance is `src` = source id and `ref` = the item's URL, else its id.
  - The event's `observed_at` is the item's `updated_at_ms`, when set.
  - The taint is the batch's.
- **Dedupe.** The batch first reads what is already held, 40 keys a listing.
  An item whose live record is unchanged is counted in `skipped`, not written,
  and a batch of nothing but unchanged items reports `already_ingested`. An
  item with empty content is skipped. Dedupe is not left to the engine's
  body key, because the engine never releases a key: an item re-synced after
  `forget_source` would replay onto an event that no longer exists.
- **Pacing.** Writes are paced at one per 300 ms across concurrent batches,
  about 200 a minute, leaving the rest of the backend's limit to lookups, polls
  and chat.
- **One wait per batch.** The batch waits once, for its last event to be
  listed, rather than once per item.
- **Supersede.** The versions each changed item replaced are then removed, as
  for any write.
- **Partial failure.** A failure mid-batch keeps its error class, and its
  message says how many items were accepted first.

`forget_source(source_id)` removes every event carrying the source's label in
the three source namespaces, after re-checking `x.prov.src`. It returns the
number of distinct live items removed.

`forget_matching`:

| Selector | Behavior |
| --- | --- |
| `Source` | as `forget_source`, limited to the namespace of its kind: `chat`, `email` or `document` (the contract's `SourceKind`) |
| `Source` with another kind | `Invalid`, never a count of zero |
| `Chunk` | reads the event by id (`GET memory/events/{id}`) and, only if its scope is one of this account's source namespaces, removes every version of that item |
| `SourcePrefix`, `Owner` | `Unsupported` |

### Maintenance

The hosted service does its own upkeep, so the family reports rather than
works:

- **Upkeep.** `reembed`, `compact` and `consolidate` answer an empty report
  saying the service runs them itself.
- **Probe.** `doctor`, `diagnose` and `degraded_state` read one health probe
  (the existing `GET memory/scopes?prefix=tmh:probe&limit=1`), cached for 60 s
  after a healthy answer and 5 s after a failed one.
- **Failure codes.** A failed probe is classified with the host health
  vocabulary:

  | Probe error | Code | Class | Degraded |
  | --- | --- | --- | --- |
  | `Unauthorized` | `auth_invalid` | unrecoverable | `storage` |
  | `BudgetExceeded` | `budget_exhausted` | unrecoverable | `storage` |
  | `Unavailable`, `Unreachable`, `Timeout` | `storage_unavailable` | transient | `storage` |
  | anything else | `transient` | transient | `storage` |

  The remediation key is `memory.health.remediation.<code>`.
- **Everything else.** Store and queue statistics keep the contract's empty
  defaults. `purge_all`, `reset_derived_index` and the backfill calls are not
  offered.

### Retrieval

The engine ranks recall but returns no score and offers no way to ask for one,
so a hit's score is its rank: 1.0 for the first, 0.1 less for each after, never
below 0.1. That fills `score` and `final_score` only, and every signal of the
breakdown (similarity, keyword, graph, episodic, freshness) stays 0: a positive
final score over no signal marks a ranking that measured nothing. A host floor
on a signal reads these hits as carrying no evidence; a host that knows the
mark keeps the engine's order. Recent recall reports its recency as freshness.

- **Source recall.** `fast_retrieve`, `cover_window` and `retrieve_source`
  recall with `view: descend` from `sources`, or one kind's namespace, and
  answer each record's newest version as a leaf. The caller's source scope, or
  the one source asked for, is a `filters.metadata.labels` filter inside the
  recall (any label matches), so other sources cannot fill the events budget;
  each leaf is re-checked against its `x.prov.src`. An empty scope answers
  nothing without a request. A time window is `temporal.valid_during`.
- **Leaves by id.** `retrieve_leaves` reads `GET memory/events/{id}` and keeps
  an event only when the answer names one of this account's scopes.
- **Namespace recall.** `recall_namespace_scored` answers the engine's first
  three hits at most, since a rank says nothing about whether the tail is
  relevant. `recall_namespace_recent` scores by freshness.
- **No tree, no entities.** `retrieve_children` answers empty;
  `search_entities` refuses an unknown kind and is otherwise `Unsupported`.

### Ingest

`ingest_document`, `ingest_chat` and `ingest_email` write content records:

- **Where.** An item's own namespace, else its kind's source namespace
  (`sources/chat`, `sources/email`, `sources/documents`), so retrieval, the
  forest and its leaves reach it.
- **Keys.** A document is `document:<source_id>`, and a new version retires the
  old. A message is `message:<source_id>:<digest of the whole message>`: a host
  may send every batch of a conversation under one source id, and keys by
  position would fold one batch into the next.
- **Text.** Mail carries `From:`, `To:`, `Cc:`, `Subject:` and
  `List-Unsubscribe:` lines above its content, as the embedded engine writes
  them. A chat message carries its owning session.
- **Cost.** A message or document already held unchanged is not written again.
  The backend has no bulk route: a batch is one paced write per message, then
  one wait for the last.

### Profile

Each facet is an inert record keyed by the facet's key in `tmi:profile`,
holding the facet as JSON. The host owns stability and state; listings are
filtered and ordered here. `upsert_provider_facet` merges as the embedded
engine does: a re-observation adds evidence and its segment, and overwrites
the value only when at least as confident; a new facet's class comes from its
key prefix, then its type. `workflow_identity_matches` applies SQL `LIKE`.

### Episodic

Turns, segments, extracted events and segment embeddings are inert records in
their own scopes. Turns and segments carry their session's label, so the reads
a host makes on every turn — the session's turns, its open segment — list one
session, never the history. A turn's id is the microsecond it was recorded at,
bumped past the last id the process handed out. Nothing is ever pending a
summary: a recap needs a model the adapter does not reach.

`EpisodicPortability` (contract 4.3) pages each of the four scopes out in key
order — turns by id — folding the scope per page, and writes pages back by
appending without per-record waits, then waiting once for the last. A turn
keeps its id unless another turn holds it; then its session is searched for
the copy an earlier run made, and only then does it take a fresh id. See
[store-migration.md](store-migration.md).

### Scoring

`extract_entities` runs on the device: emails, URLs, `@handles`, `name#1234`
discriminators and `#hashtags` (also as topics). `embed_text` is `Unsupported`:
the memory API has no route to embed text on request. `embedder_slug` is
`cloud`.

### Tree

The server derives facts, beliefs and concepts per scope, each citing what it
came from, served at `GET memory/{facts,beliefs,understanding}`. The forest
reads those layers for `global` and the three source namespaces:

- facts are level 1 over the events they cite, beliefs level 2 over facts,
  concepts level 3 over beliefs and facts;
- each node hangs under the most confident node a level up that cites it, so
  the forest stays a tree, and carries its text as `TreeSummary::preview`
  (contract 4.2);
- what the server set aside (superseded, struck, deprecated, merged) is left
  out, and a source-scoped caller is answered no derived nodes;
- a layer is read to 2,000 items per namespace, and one reading is reused for
  a minute, because the layer routes share the backend's rate limit.

`recent_leaves` reads the newest records of the same namespaces, each under
the fact that cites it, a source scope narrowing each listing by label.
`summarise` folds nothing into an empty summary and is otherwise `Unsupported`;
`flush_source_tree` is 0, `root_summaries_with_caps` empty, `flavour_profile`
`None`. The members that write or walk a tree are `Unsupported`.

### Capabilities

| Wire | Advertises |
| --- | --- |
| TinyHumans | mandatory + `DocumentIngest`, `ConversationIngest`, `LearningIngest`, `EventIngest`, `Answer` + `Goals`, `ToolMemory`, `Documents`, `Sources`, `Maintenance`, `Retrieval`, `Ingest`, `Profile`, `Episodic`, `Scoring`, `Tree`, `EpisodicPortability` |
| Direct | unchanged |

Every `as_*` accessor matches, and `audit_provider` holds on both wires.

## Invariants and constraints

- Direct-mode requests, records and capabilities are unchanged.
- No `tmi` scope is reachable from a namespace. No bookkeeping record is ever
  embedded or extracted.
- Every hosted keyed write goes through the one-claim retry path.
- Labels are fixed-length digests. Exactly one `labels=` parameter per request.
- No request carries `confirm_all`. Every removal names its events.
- Every hosted scope fits 31 segments, including a details scope, which spends
  one segment more than its namespace.
- No response key named `scope` or `path` is used to carry record data: the
  memory API rewrites those keys everywhere in a response.
- A rank is never reported as a signal.
- A source scope is applied inside the request, never only after it.

## Acceptance criteria

- The conformance suite, including `assert_documents_round_trip` and the
  capability audit, passes against the `/memory/*` double.
- The double enforces the backend's behavior this relies on: one `labels=`
  parameter, `GET memory/events/{id}`, and the tenant scope grammar.
- Unit tests cover each family's contract, the supersede and tombstone paths,
  the label-miss fallback, pacing, partial-batch failure, the probe
  classification and cache, rank scores and the three-hit cap, scope filters
  inside the request, per-message keys, session-labelled reads, the facet
  merge, and the forest's placement, set-aside rules, paging and cache.
- On a live account (`TINYMEMORY_TEST_TINYHUMANS_*`), goals, a tool rule, a
  document and a source batch round-trip, and `forget_source` removes the
  batch.

## Open questions

- `query_documents` keeps its rank-and-overlap estimate, while `Retrieval`
  reports bare ranks (D2). Whether documents should report ranks too is open.
- Every per-turn family write is billed. A host should expect a few calls per
  conversation turn, and more when a segment closes.
- Source namespaces are fixed per kind. A host that wants one namespace per
  connection needs a contract field.
- A new or changed source item costs one billed write, plus a removal when it
  changes; an unchanged one costs a share of one lookup.
