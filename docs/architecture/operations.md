# Operations

What each `MemoryEngine` operation does, independent of engine. For the
types see [api.md](api.md) and [api-items.md](api-items.md); for how CortexDB
realises them see [cortex.md](cortex.md).

Every operation starts the same way: **validate the request** (its `validate`
method) and return `Error::InvalidRequest` before touching storage. Filters
are applied identically everywhere through `MetaFilter::matches`, including
the `reach` that confines which [namespaces](namespaces.md) are visible.

## store

`store(item) -> StoreReceipt { id, replayed }`

1. `item.validate()`. A blank body, unresolved `Uri`, empty conversation,
   blank learning or out-of-range confidence is `InvalidRequest`.
2. Derive the item's identity from `item.fingerprint()`.
3. If the engine already holds that item, write nothing and return the
   existing id with `replayed: true`.
4. Otherwise write it at `meta.namespace` (the item lands at exactly one
   node) and return `replayed: false`.

Which fields are in the fingerprint, and why `observed_at` is not, is in
[Idempotency](#idempotency-and-fingerprints).

## store_many

`store_many(items) -> Vec<StoreReceipt>`, for imports, backfills and syncs.

1. `validate_many`: `1..=100` items (`MAX_STORE_MANY`), each valid. Empty or
   oversized is `InvalidRequest`; so is the first invalid item.
2. Store the items **in order**. The default implementation calls `store` per
   item; an engine may batch.
3. Receipts come back in item order. An item repeated within the batch is a
   replay of its first copy.
4. On return every item is readable through `list`, `get` and `forget`.
   Ranked `fetch` and `recall` **may lag** a moment behind for all but the
   last item; that is what lets an engine skip a per-item wait.
5. On an error, the items before the failing one are stored. Sending the batch
   again is safe: the stored ones come back as replays.

## fetch

`fetch(req) -> FetchPage { hits, next_cursor }`: raw retrieval, no synthesis.

1. The engine checks `req.mode` against its descriptor
   (`EngineDescriptor::ensure_mode`): a mode it does not serve is
   `Error::Unsupported`. Hosts read `fetch_modes` and never offer one the
   engine lacks.
2. `req.validate()`: blank query or zero limit is `InvalidRequest`.
3. Rank items admitted by `req.filter` in `Keyword` (lexical), `Vector`
   (embedding) or `Hybrid` (the engine's blend) mode. Hits are best first,
   each with a `score`.
4. Return up to `limit` hits and a `next_cursor` when more remain
   ([cursors](#cursors-and-paging)).

### Fetch modes and descriptor gating

`EngineDescriptor::fetch_modes` is the engine's declaration of what it
serves. An engine need not serve all three (CortexDB declares only `Hybrid`).
Gating is by declaration: `supports(mode)` answers, `ensure_mode(mode)`
fails with `Unsupported("engine `<id>` does not offer <mode> fetch")`.
`tinymemory-tools` mirrors this: the `memory_fetch` tool's `mode` enum lists
exactly the engine's modes, and an engine serving none gets no such tool.
The conformance suite checks that every declared mode works and every
undeclared one is `Unsupported`.

## recall

`recall(req) -> RecallAnswer { answer, citations, model? }`

1. `req.validate()`: blank question or zero limit is `InvalidRequest`.
2. Gather at most `limit` citations from items admitted by `req.filter`.
3. Synthesise an answer, optionally steered by `req.instructions`. How the
   engine answers is its own business.
4. Return the answer text, its `Citation`s and, when the engine reports it,
   the model.

Every citation's `id` must resolve through `get` or `list`; the conformance
suite checks it.

## list

`list(req) -> ListPage { items, next_cursor }`: a query-free listing.

1. `req.validate()`: zero limit is `InvalidRequest`.
2. Return up to `limit` items admitted by `req.filter`, each a `Hit` with
   `score == 0.0`, plus a `next_cursor` when more remain.

The contract does not promise an order across engines, only that following
cursors visits every matching item and that paging terminates. `list` is the
primitive the default `explore` and `get` are built on.

## forget

`forget(target) -> ForgetReport { forgotten }`

`ForgetTarget::validate` runs first:

| Target | Rule |
| --- | --- |
| `Ids(ids)` | At least one id, else `InvalidRequest("forget needs at least one id")`. |
| `Filter(filter)` | Must not be empty (`MetaFilter::is_empty`), else `InvalidRequest`: an empty filter would mean everything, so the contract refuses it. |

- **By ids**: remove those items, **wherever they live**. Ids are not scoped
  by namespace. Ids that name nothing are skipped and not counted. A caller
  confined to a reach reads the ids first with `get` under that reach and
  forgets only what came back (this is what `memory_forget` does).
- **By filter**: remove every item the filter admits. The filter's `reach`
  confines it. A filter holding only a `reach` is not empty, so
  `Filter(MetaFilter { reach: Some(..), .. })` forgets everything in that
  reach. Callers that take filters from untrusted input should require a
  second field, as `tinymemory-tools` does.

`forgotten` counts items actually removed.

## explore

`explore(req) -> ExplorePage`: counts of stored items per value of one facet,
for explorers (a UI tree, a CLI, an audit script).

The **facet** is a metadata dimension fixed by the contract (`Kind`,
`Source`, `SourceId`, `Workspace`, `Folder`, `FilePath`, `Language`, `Repo`,
`Url`, `Thread`, `Agent`, `ToolCall`, `Tag`, `Namespace`), so one explorer
works on every engine. `Facet::values(kind, &meta)` gives an item's values
for a facet: none when the field is unset, several only for `Tag`.

Semantics of the default (`explore_by_listing`), which every engine gets
unless it overrides `explore`:

1. `req.validate()`: `limit` in `1..=500`, `scan_limit` in `1..=50 000`
   (default 5 000).
2. Page through `list` with `req.filter`, 200 at a time, reading at most
   `scan_limit` items.
3. For each item read: `total += 1`; if the facet has no value for it,
   `missing += 1`; each value it has increments that value's count. A tagged
   item counts once per tag, so for `Tag` bucket counts can sum to more than
   `total`.
4. Sort buckets by count descending, ties by value ascending; cut to `limit`;
   `more_buckets` is the number of distinct values cut.
5. `truncated` is `true` when the scan stopped at `scan_limit` with more
   items remaining; counts are then a **lower bound**, and `total` is the
   number read.

An engine that aggregates server-side overrides `explore` and may ignore
`scan_limit`.

### Facet::narrow: drilling down

`facet.narrow(&mut filter, value)` turns a chosen bucket back into a filter
field, so drilling down is: `explore` → pick a bucket → `narrow` → `explore`
(another facet) or `list`.

| Facet | Sets on the filter |
| --- | --- |
| `Kind` | `kinds = [value]` (must name an item kind) |
| `Source` | `sources = [value]` (must name a source kind) |
| `SourceId` | `source_id` |
| `Workspace`, `Language`, `Repo`, `Url`, `Agent`, `ToolCall` | `workspace`, `language`, `repo`, `url`, `agent_id`, `tool_call` |
| `Folder`, `FilePath` | `folder`, `file_path` (**prefix** match, so a folder also admits its subfolders) |
| `Thread` | `thread_id` |
| `Tag` | `tags_any = [value]` |
| `Namespace` | `reach = Reach::exact(value.parse()?)`: exactly that node |

`narrow` **replaces** the one field it targets (a list field is replaced by a
one-element list; `Namespace` replaces any existing reach) and leaves others
alone. It fails with `InvalidRequest` for a blank value, an unknown kind or
source value, or a namespace that does not parse.

Drill-down example:

```text
explore(facet=source)                      → folder: 40, github: 7
Source.narrow(filter, "folder")
explore(facet=folder, filter)              → /notes: 31, /docs: 9
Folder.narrow(filter, "/notes")
list(filter)                               → the 31 items under /notes (and subfolders)
```

Because `Folder` matches by prefix, a bucket count for `/notes` (items whose
`folder` is exactly that value) can be smaller than the number of items a
narrowed `list` returns, since subfolder items match the prefix too.

## get

`get(req) -> Vec<Hit>`: read whole items by id.

1. `req.validate()`: `1..=200` ids (`MAX_GET_IDS`), none blank.
2. Look each id up. The default pages through `list` (200 at a time,
   confined to `req.reach` when set) until every id is found or the listing
   ends; an engine that can look an id up directly overrides it.
3. Return hits **in the order the ids were named**, each at most once. An id
   that names nothing is left out, with no error. So is an id whose item lies
   outside `req.reach`: it is indistinguishable from a missing one.

## Idempotency and fingerprints

Storing an identical item twice must not create a second item. The contract
expresses "identical" as `StoreItem::fingerprint`:

- **What is hashed**: SHA-256 over the item's JSON, keeping the first 20 bytes
  as 40 hex characters. That JSON holds the whole item: kind, title, body,
  mime, turns (with tool calls), learning kind, confidence, evidence, and all
  of `meta` (including `namespace`, `tags` and `source`).
- **What is excluded**: `meta.observed_at`, set to `None` before hashing. It
  says when the item was seen, which a host stamps on every store.
  Including it would make a retried learning, or an unchanged file re-synced,
  a new item every time.
- **Namespace is included**: the same text at two nodes is two items.
  Root-namespace items serialise without a namespace, so their fingerprints
  match those from before namespaces existed.
- **Replay**: an engine that finds the fingerprint already stored writes
  nothing and returns `StoreReceipt { id, replayed: true }`, with the same id
  as the first store. A retry after a timeout or a partial `store_many` is
  therefore safe.
- **Changing anything else is a new item**: editing one tag, one character of
  text, or the confidence produces a different fingerprint and a second
  item; the old one is not replaced.

Engines choose how the id relates to the fingerprint (`ItemId` is opaque); the
reference engine uses the fingerprint itself.

## Cursors and paging

`fetch` and `list` page with an opaque `cursor: Option<String>`.

- A first request has no cursor. A page that is not the last carries
  `next_cursor: Some(token)`; the last page has `None`.
- Pass `next_cursor` unchanged as the next request's `cursor`, with the same
  filter, query and mode. The format is the engine's business; do not parse or
  construct one. An engine rejects a cursor it does not recognise with
  `InvalidRequest`.
- `limit` is the page size and must be positive. A page may hold fewer items
  than `limit`; only a missing `next_cursor` means the end.
- Cursors must make progress: a repeated cursor means paging never ends, and
  the conformance suite fails an engine that does that.
- `explore` and `get` take no cursor: they page internally through `list`.

## health

`health() -> EngineHealth` (`Ok`, `Degraded(reason)`, `Down(reason)`) is
infallible and cheap to call. `Degraded` still serves; `Down` does not
(`is_serving()`). Failures calling the engine are reported as `Down` with a
sanitised reason, never as an error. The conformance suite's first check is
that the engine reports itself serving.
