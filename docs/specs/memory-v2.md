# Memory v2: Recall, Fetch, Store

Status: accepted. Supersedes every other spec in this directory; those files are
deleted with the code they describe.

## Why

TinyMemory grew a 27-family capability contract, eight engines, an embedded
engine, a TinyBus module, a tool layer and a summary tree. A host needs three
things from memory, plus a way to choose who provides them:

| Operation | Meaning |
| --- | --- |
| **Recall** | A question in, a synthesized answer with citations out. The engine owns how it answers (agentic loop, native ask route, …). |
| **Fetch** | Raw retrieval: keyword, vector or hybrid search over stored items, filtered by metadata. No synthesis. |
| **Store** | Ingest one of three kinds of item — document, conversation, learning — each carrying typed metadata. |

On top of the engine sits one engine-neutral product: **`context.md`**, a
token-budgeted brief compiled from Recall and Fetch that a host injects at the
start of a session.

## Crates

| Crate | Owns |
| --- | --- |
| `tinymemory-api` | The contract: `MemoryEngine`, request/response types, `MemoryMeta`, `MetaFilter`, `EngineDescriptor`, `Error`. No I/O. |
| `tinymemory-cortex` | The CortexDB engine, registered twice: `cortexdb` (direct `/v1/*`, endpoint + key) and `tinyhumans` (CortexDB behind the TinyHumans backend `/memory/*`, host bearer). |
| `tinymemory-documents` | Format sniffing and conversion to markdown (Markdown, plain text, HTML, code, PDF/DOCX via a host `DocumentConverter`). Emits `StoreItem::Document`. |
| `tinymemory-sources` | Readers that turn a source into `StoreItem`s: folder, file, link (web page), GitHub repo, RSS, Composio toolkit payloads. Includes the SSRF guard. |
| `tinymemory-safety` | Secret/PII scrubbing applied to every item before `store`. |
| `tinymemory-context` | `ContextCompiler`: builds `context.md` from an engine. |
| `tinymemory-import` | Reads a legacy (v1, embedded TinyCortex) workspace and yields `StoreItem`s. |
| `tinymemory-conformance` | Behavioural suite every engine must pass, plus a reference in-memory engine. |
| `tinymemory` | Facade: engine registry, `MemoryConfig`, `build_engine`, re-exports. One feature per optional crate. |

Deleted: `tinymemory-bus`, `tinymemory-core`, `tinymemory-tinycortex`,
`tinymemory-remote` (CortexDB moves to `tinymemory-cortex`; mem0, supermemory,
cognee, agentmemory, livingbrain are dropped), `tinymemory-tools`,
`tinymemory-conversations`, `tinymemory-guard`, `tinymemory-gate`,
`tinymemory-sync` (normalisers move into `tinymemory-sources`),
`tinymemory-module`, `tinymemory-testing-ui`, and the `vendor/tinycortex`,
`vendor/tinybus` and `vendor/tinyinference` submodules (`tinymemory-import`
reads the v1 on-disk layout directly, so it needs no engine dependency).
`tinymemory-conversations` (the chat thread store) moves to
`tinyagents-session::threads` in tinyagents.

## Contract (`tinymemory-api`)

```rust
#[async_trait]
pub trait MemoryEngine: Send + Sync {
    fn descriptor(&self) -> &EngineDescriptor;
    async fn health(&self) -> EngineHealth;
    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer>;
    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage>;
    async fn store(&self, item: StoreItem) -> Result<StoreReceipt>;
    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport>;
    async fn list(&self, req: ListRequest) -> Result<ListPage>;
    // Explorers; both have listing-based defaults (see "Explore and get").
    async fn explore(&self, req: ExploreRequest) -> Result<ExplorePage>;
    async fn get(&self, req: GetRequest) -> Result<Vec<Hit>>;
    // Bulk ingestion; default stores one at a time (see "Bulk store").
    async fn store_many(&self, items: Vec<StoreItem>) -> Result<Vec<StoreReceipt>>;
}
```

### Metadata

```rust
pub struct MemoryMeta {
    pub workspace: Option<String>,   // absolute path or logical workspace id
    pub folder: Option<String>,      // containing folder (absolute or workspace-relative)
    pub file_path: Option<String>,
    pub language: Option<String>,    // code language or natural language tag
    pub repo: Option<String>,        // "owner/name" or remote URL
    pub commit: Option<String>,
    pub url: Option<String>,
    pub thread_id: Option<String>,
    pub turns: Option<TurnRange>,    // { first: u32, last: u32 }
    pub agent_id: Option<String>,
    pub tool_call: Option<ToolCallRef>, // { name, id }
    pub source: SourceRef,           // { kind: SourceKind, id: Option<String> }
    pub tags: Vec<String>,
    pub observed_at: Option<DateTime<Utc>>,
}
pub enum SourceKind { Folder, File, Link, Github, Rss, Composio, Conversation, Agent, Import }
```

`MetaFilter` has the same optional fields (each an exact match, `folder` and
`file_path` also match as a prefix), plus `kinds: Vec<ItemKind>`,
`sources: Vec<SourceKind>`, `tags_any: Vec<String>`, and an
`observed_after`/`observed_before` window. An empty filter matches everything.

### Items

```rust
pub enum ItemKind { Document, Conversation, Learning }

pub enum StoreItem {
    Document { title: Option<String>, body: DocumentBody, mime: Option<String>, meta: MemoryMeta },
    Conversation { turns: Vec<Turn>, meta: MemoryMeta },
    Learning { text: String, kind: LearningKind, confidence: f32, evidence: Option<String>, meta: MemoryMeta },
}
pub enum DocumentBody { Text(String), Uri(String) } // Uri is resolved by sources before store
pub struct Turn { pub role: Role, pub text: String, pub at: Option<DateTime<Utc>>, pub tool_calls: Vec<ToolCallRef> }
pub enum LearningKind { Preference, Fact, Procedure, Correction, Other }
```

`StoreReceipt { id: ItemId, replayed: bool }`. Engines derive idempotency from
the full item except `meta.observed_at` (when it was seen, not what it is), so
an identical retry, or an unchanged file re-synced, is a replay, not a
duplicate.

### Recall

```rust
pub struct RecallRequest { pub question: String, pub filter: MetaFilter, pub limit: usize, pub instructions: Option<String> }
pub struct RecallAnswer { pub answer: String, pub citations: Vec<Citation>, pub model: Option<String> }
pub struct Citation { pub id: ItemId, pub kind: ItemKind, pub snippet: String, pub meta: MemoryMeta, pub score: Option<f32> }
```

How an engine answers is its own business. CortexDB builds a recall pack and
calls its answer route once with that pack (`/v1/answer`, `/memory/answer`).

### Fetch and list

```rust
pub enum FetchMode { Keyword, Vector, Hybrid }
pub struct FetchRequest { pub query: String, pub mode: FetchMode, pub filter: MetaFilter, pub limit: usize, pub cursor: Option<String> }
pub struct FetchPage { pub hits: Vec<Hit>, pub next_cursor: Option<String> }
pub struct Hit { pub id: ItemId, pub kind: ItemKind, pub text: String, pub meta: MemoryMeta, pub score: f32, pub confidence: Option<f32> }
pub struct ListRequest { pub filter: MetaFilter, pub limit: usize, pub cursor: Option<String> }
pub struct ListPage { pub items: Vec<Hit>, pub next_cursor: Option<String> } // score = 0
pub enum ForgetTarget { Ids(Vec<ItemId>), Filter(MetaFilter) } // Filter must not be empty
```

A mode the engine does not list in `EngineDescriptor::fetch_modes` fails with
`Error::Unsupported`. Hosts read the descriptor and never offer it.

`Hit::text` is the item's `StoreItem::render_text()` form, and
`Hit::confidence` carries a learning's confidence (`None` for other kinds), so a
listing can be ordered by it. An item's id is its `StoreItem::fingerprint()`.

### Explore and get

An explorer walks stored items by **facet**, a metadata dimension fixed by the
contract rather than by an engine's storage layout, so one explorer works on
every engine:

```rust
pub enum Facet { Kind, Source, SourceId, Workspace, Folder, FilePath, Language, Repo, Url, Thread, Agent, ToolCall, Tag }
pub struct ExploreRequest { pub facet: Facet, pub filter: MetaFilter, pub limit: usize /* 1..=500 buckets */, pub scan_limit: usize /* 1..=50_000, default 5_000 */ }
pub struct FacetBucket { pub value: String, pub count: u64 }
pub struct ExplorePage { pub facet: Facet, pub buckets: Vec<FacetBucket>, pub total: u64, pub missing: u64, pub more_buckets: u64, pub truncated: bool }
pub struct GetRequest { pub ids: Vec<ItemId> /* 1..=200 */ }
```

- `explore` groups the items `filter` admits by one facet: buckets largest
  first (ties by value), `missing` counts items with no value, `more_buckets`
  the values cut by `limit`. `Tag` is multi-valued; an item counts once per
  tag.
- `Facet::narrow(&mut filter, value)` turns a bucket back into the filter
  field, so drilling down is `explore`, pick a bucket, `narrow`, then
  `explore` or `list` again. `Folder` and `FilePath` narrow by prefix, as
  their filter fields do.
- `get` reads items whole, in the order named; unknown ids are left out.
- **Defaults.** `explore_by_listing` pages through `list` up to `scan_limit`
  items and sets `truncated` when it stops early, so counts are then a lower
  bound. `get_by_listing` pages until every id is found. An engine overrides
  either when it can do better: CortexDB looks ids up by their labels.

### Bulk store

`store_many(items)` (1 to `MAX_STORE_MANY` = 100 items) is for imports,
backfills and syncs. Receipts come back in item order; an item repeated in the
batch is a replay of its first copy. Every item is readable through `list`,
`get` and `forget` on return, as with `store`; ranked `fetch`/`recall` may lag
for all but the last. On an error the earlier items are stored, and storing
them again is a replay.

CortexDB pays per batch, not per item: one id lookup per kind for replay
detection, all missing events written without waiting, then one listing wait
per scope for the last event written there (a scope's log is indexed in
order), and the ranked-recall wait for the final event only. Its event
listing slows as a scope grows, so per-item waits made a 5,000-item import
take hours.

### Descriptor and health

```rust
pub struct EngineDescriptor {
    pub id: &'static str, pub label: &'static str, pub description: &'static str,
    pub hosted: bool, pub needs_endpoint: bool, pub needs_key: bool,
    pub default_endpoint: Option<&'static str>, pub fetch_modes: Vec<FetchMode>,
}
pub enum EngineHealth { Ok, Degraded(String), Down(String) }
```

### Errors

There is one `Error` enum: `Unsupported`, `InvalidRequest`, `Unauthorized`,
`NotFound`, `Conflict`, `Unavailable` (transient), `Engine` (the engine's own
failure, already sanitised), and `Config`. Messages never carry credentials.

## Facade (`tinymemory`)

```rust
pub struct MemoryConfig { pub engine: String, pub engines: BTreeMap<String, EngineSettings> }
pub struct EngineSettings { pub endpoint: Option<String> }
pub enum EngineCredential { None, Static(String), Dynamic(Arc<dyn BearerSource>) }
pub fn list_engines() -> Vec<EngineDescriptor>;
pub fn build_engine(id: &str, settings: &EngineSettings, credential: EngineCredential) -> Result<Arc<dyn MemoryEngine>>;
```

`build_engine` refuses an unknown id, a missing required endpoint or key, and a
credentialed cleartext non-loopback endpoint.

## Engine: CortexDB (`tinymemory-cortex`)

- **Wires.** `Direct` (`v1/experience`, `v1/events`, `v1/recall`, `v1/forget`, `v1/answer`) and `TinyHumans` (`memory/*` with `{success,data}` envelopes), as in the v1 adapter.
- **Store.**
  - Each item becomes one experience: a conversation becomes a bulk append of its turns.
  - The envelope carries `{v:2, kind, meta, title?, learning_kind?, confidence?}`, and `meta` maps to scope labels where CortexDB can filter.
  - Writes wait for the indexed barrier, keeping the v1 `await_readable` behaviour.
- **Scope.** One scope per item kind under the TinyMemory root `app:tinymemory` (which the hosted backend further roots under the tenant): `app:tinymemory/app:documents`, `app:tinymemory/app:conversations`, `app:tinymemory/app:learnings`. A `MetaFilter.kinds` restricts the scopes searched. The segments use CortexDB's built-in `app` scope type because CortexDB v0.10+ refuses scope types outside the deployment's `allowed_scope_types` (`422 UNREGISTERED_SCOPE_TYPE`).
- **Fetch.**
  - `Hybrid` maps to `recall` layers. `Keyword` and `Vector` are declared only if the wire exposes a mode switch; otherwise `fetch_modes = [Hybrid]`. The recall body accepts only `scope`, `query`, `budgets`, `view`, `include`, `temporal` and `filters`, with no mode switch, so both wires declare `[Hybrid]`.
  - Metadata filters CortexDB cannot apply server-side are applied client-side on the page, and the cursor is still the engine's.
- **Recall.** Pack, then answer, as in v1. The pack is recalled from the one admitted kind's scope, or from `app:tinymemory` with `view: "descend"` when several kinds are admitted, so the answer route is called once. Citations come from the pack's `layers.events`, decoded back to `Hit`s.
- **List / forget.** These use `v1/events` paging and `v1/forget` by `memory_ids`. `ForgetTarget::Filter` lists first, then forgets ids, and never sends an empty selector.

## Context (`tinymemory-context`)

```rust
pub struct ContextSpec { pub budget_tokens: usize, pub briefs: Vec<Brief>, pub learnings_limit: usize }
pub struct Brief { pub heading: String, pub question: String, pub filter: MetaFilter }
pub struct ContextDoc { pub markdown: String, pub tokens: usize, pub generated_at: DateTime<Utc>, pub engine: String, pub refs: Vec<ItemId> }
pub async fn compile(engine: &dyn MemoryEngine, spec: &ContextSpec) -> Result<ContextDoc>;
```

The default briefs are:
- **About the user:** identity, role, and how they like to work.
- **Active work:** current projects, workspaces and repos.
- **Preferences and standing instructions.**
- **Recent important events.**

After the briefs comes a "Learnings" list, from a `list` of `kind = Learning` sorted by recency and confidence.

Output rules:
- Each section is trimmed so the whole document fits `budget_tokens`, estimated at 4 chars per token. The briefs keep their order, and learnings are trimmed first.
- Frontmatter records `generated_at`, `engine`, `tokens` and `refs`.
- An engine with nothing stored yields an empty document (`markdown` is empty), not an error. A brief that fails is skipped and logged; it does not fail the document.

## Import (`tinymemory-import`)

`LegacyWorkspace::open(path)` detects a v1 TinyCortex store. `items()` yields
`StoreItem`s:
- Documents become `Document`.
- Episodic turns grouped by thread become `Conversation`.
- Learning-section and `global` records become `Learning`.
- Profile facets become `Learning(Preference)`.

Every item gets `source.kind = Import`. A `Checkpoint` (last yielded cursor per
section, persisted by the host) makes import resumable. Behind `legacy-import`.

## Testing

`tinymemory-conformance::run(engine)` covers:
- store/list round-trip for each kind;
- replay idempotency;
- `explore` counts agreeing with `list` per kind and per workspace, and each
  bucket narrowing to exactly its count;
- `get` returning listed items by id, in request order, unknown ids left out;
- `store_many` storing in order, listing every item on return, replaying a
  repeated batch, and refusing an empty one;
- fetch filtering by every meta field;
- forget by id and by filter;
- refusing an empty filter;
- `Unsupported` for undeclared modes;
- recall returning citations that resolve via `list`.

It runs against the reference engine and against both CortexDB wires through an HTTP double.
