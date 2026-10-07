# tinymemory-api

The TinyMemory core contract: the operations a host needs from memory, and the
types they speak. The crate performs **no I/O** and links no runtime, HTTP
stack or storage engine, so an engine, a tool layer or a host can depend on it
alone. Engines live in
[`tinymemory-integrations`](../tinymemory-integrations); the agent-facing tools
and `context.md` compiler live in [`tinymemory-tools`](../tinymemory-tools).

## Surface

| Item | Purpose |
| --- | --- |
| `MemoryEngine` | The object-safe async trait: `recall`, `fetch`, `store`, `store_many`, `forget`, `list`, `export`, `explore`, `get`, plus `descriptor` and `health` |
| `EngineDescriptor`, `EngineHealth` | What an engine is and offers (including its `fetch_modes`), and whether it can serve |
| `StoreItem` | A `Document`, `Conversation` or `Learning`, each with a `MemoryMeta`; `validate` and `fingerprint` |
| `MemoryMeta`, `MetaFilter` | Typed metadata on every item, and the query that selects by it |
| `Namespace`, `Segment`, `Reach` | The memory tree: where an item lives and how far a reader reaches |
| `RecallRequest`, `FetchRequest`, `ListRequest`, `ForgetTarget`, ... | Requests and responses, each with a `validate` engines call first |
| `Facet`, `ExploreRequest`, `GetRequest` | Browsing: counts per metadata facet, and reading items by id |
| `Error`, `Result` | The one error every engine returns |

`store_many`, `explore` and `get` have default implementations (one `store` at
a time; paging through `list`), so a minimal engine implements seven methods
and overrides the defaults only when it can do better. `export` (items back
whole, for moving memory) defaults to `Unsupported`.

## Example

```rust
use tinymemory_api::{ItemKind, MemoryMeta, MetaFilter, SourceKind, StoreItem};

let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("notes".into()));
meta.file_path = Some("/notes/rust/ownership.md".into());
let item = StoreItem::document("Ownership moves values.", meta);
item.validate()?;

let filter = MetaFilter {
    file_path: Some("/notes/rust".into()),
    ..MetaFilter::kinds([ItemKind::Document])
};
assert!(filter.matches(item.kind(), item.meta()));
# Ok::<(), tinymemory_api::Error>(())
```

## Limits

| Constant | Value | Bounds |
| --- | --- | --- |
| `MAX_STORE_MANY` | 100 | items per `store_many` |
| `MAX_GET_IDS` | 200 | ids per `GetRequest` |
| `MAX_BUCKETS` | 500 | `ExploreRequest::limit` |
| `MAX_SCAN_LIMIT` | 50 000 | `ExploreRequest::scan_limit` (default 5 000) |

A namespace nests at most 8 deep and a segment id is 1 to 128 characters of
`A-Za-z0-9_-`. `recall`, `fetch` and `list` limits must be positive.

## Things worth knowing

- **Idempotency.** `StoreItem::fingerprint` hashes the whole item except
  `meta.observed_at`; an engine turns an identical store into a replay
  (`StoreReceipt::replayed`).
- **Empty forget is refused.** `ForgetTarget` with no ids or an empty filter is
  `Error::InvalidRequest`; it would mean "everything".
- **Namespaces.** An item lives at one node. `MetaFilter::reach` confines reads
  (`None` reads every node); `forget` by ids is not scoped.
- **Fetch modes.** A mode missing from `EngineDescriptor::fetch_modes` fails
  with `Error::Unsupported`.

## The `conformance` feature

`features = ["conformance"]` adds `tinymemory_api::conformance`:
`run(&dyn MemoryEngine)`, the behavioural suite every engine must pass, and
`ReferenceEngine`, an in-memory engine that passes it. It adds no dependency.

```toml
[dev-dependencies]
tinymemory-api = { path = "../tinymemory-api", features = ["conformance"] }
```

## Further reading

Architecture documents live in
[`docs/architecture`](../../docs/architecture/README.md): the
[contract reference](../../docs/architecture/api.md),
[operation semantics](../../docs/architecture/operations.md) and
[namespaces](../../docs/architecture/namespaces.md). The accepted behaviour is
[`docs/specs/memory-v2.md`](../../docs/specs/memory-v2.md).
