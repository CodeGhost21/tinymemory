# tinymemory-integrations

Everything in TinyMemory that touches the outside world, as one crate with a
Cargo feature per integration: the CortexDB engine and the registry that builds
it, document conversion, source readers, secret and PII scrubbing, and the
import of legacy v1 workspaces. The contract these integrations implement or
produce for lives in [`tinymemory-api`](../tinymemory-api/README.md); the
agent-facing tools are in [`tinymemory-tools`](../tinymemory-tools/README.md).

The crate's only unconditional dependency is `tinymemory-api`. A host that
wants one integration enables one feature and links nothing else.

## Modules and features

| Module | Feature | What it does | Module README |
| --- | --- | --- | --- |
| `cortex`, `registry`, `config` | `cortex` (default) | `CortexEngine` over two wires (`cortexdb`, `tinyhumans`); `list_engines`, `build_engine`, `EngineCredential`; `MemoryConfig` | [`src/cortex/README.md`](src/cortex/README.md) |
| `documents` | `documents` | Format sniffing and conversion to markdown, producing `StoreItem::Document`. No I/O. | [`src/documents/README.md`](src/documents/README.md) |
| `documents::OfficeConverter` | `documents-office` | PDF, DOCX, PPTX and XLSX to markdown, in process | (same) |
| `sources` | `sources` | Readers for folders, files and conversations; Composio payload normalisers; `collect_items`. Links no HTTP stack. | [`src/sources/README.md`](src/sources/README.md) |
| `sources::fetch`, GitHub, RSS and web-page readers | `sources-network` | The network readers and `fetch_url`, all behind the SSRF guard | (same) |
| `safety` | `safety` | Secret and PII scrubbing of a `StoreItem` before it is stored | [`src/safety/README.md`](src/safety/README.md) |
| `import` | `legacy-import` | Reads a v1 (embedded TinyCortex) workspace and migrates it into any engine, resumably | [`src/import/README.md`](src/import/README.md) |

`full` turns on `cortex`, `documents-office`, `sources-network`, `safety` and
`legacy-import`. Feature implications: `documents-office` implies `documents`;
`sources` implies `documents`; `sources-network` implies `sources`.

## Dependency weight per feature

What each feature adds to the dependency graph (on top of `tinymemory-api` and
the small `serde`, `serde_json`, `thiserror`, `async-trait` set the feature
already needs):

| Feature | Adds |
| --- | --- |
| `cortex` | `reqwest` (rustls TLS, streaming bodies), `tokio` (`time` only), `futures`, `sha2` |
| `documents` | nothing beyond the small set above |
| `documents-office` | `pdf-extract`, `calamine`, `quick-xml`, `zip` (all pure Rust, no system libraries) |
| `sources` | `schemars`, `regex`, `walkdir`, `chrono`, `log`, `tracing` |
| `sources-network` | `reqwest`, `futures`, `tokio` with `process`, `io-util` and `net` (the GitHub reader runs `gh` and `git`; the SSRF resolver does DNS) |
| `safety` | `regex`, `serde_json`, `log` |
| `legacy-import` | `rusqlite` with bundled SQLite (compiles C; no system SQLite needed) |

`documents-office` and `legacy-import` are the heavy ones, which is why neither
is on by default.

## The write pipeline

An item reaches an engine through up to four stages. Each stage is its own
module, none calls the next, and the host composes them; no engine scrubs or
converts on its own.

```text
sources ──▶ documents ──▶ safety ──▶ engine.store
(read)     (to markdown)   (scrub)    (cortex, or any MemoryEngine)
```

1. **sources** lists a configured source and reads each entry. Local readers
   hand raw bytes to the converter; network readers fetch through the SSRF
   guard. `sources::collect_items` drives one source and collects per-item
   failures instead of aborting.
2. **documents** sniffs the format and converts bodies to markdown. A
   `ConverterChain` decides which converter handles which format; a host can
   put its own PDF or Office converter in front.
3. **safety** (`scrub_item`) redacts credentials and personal identifiers from
   every free text the item carries. Metadata identifiers are left alone
   because filters match on them.
4. **engine** is any `tinymemory_api::MemoryEngine`, normally the one
   `build_engine` returns.

Imports skip the first three stages: `import::migrate` produces items
directly from a v1 workspace and stores them in batches.

## Example

```rust,no_run
use std::sync::Arc;
use tinymemory_integrations::{EngineCredential, MemoryConfig, StaticBearer};

# async fn demo() -> tinymemory_integrations::Result<()> {
// Select the engine by configuration; the credential comes from the host's
// secret store, never from the config.
let config: MemoryConfig = toml::from_str(r#"engine = "tinyhumans""#).unwrap();
let engine = config.build(EngineCredential::Dynamic(Arc::new(StaticBearer::new("tiny_live_..."))))?;
assert_eq!(engine.descriptor().id, "tinyhumans");
# Ok(())
# }
```

`cargo run -p tinymemory-integrations --example basic` lists the registered
engines and builds one without any network access.

## Errors

The crate-level `Error` is `tinymemory_api::Error`; `cortex` and the registry
return it directly. `documents`, `sources` and `import` each keep a typed
error (a path escaping its root, a non-v1 workspace are worth matching on),
and each converts into the contract error with `From`.

## Tests

Unit tests sit beside their modules in `mod_tests.rs` files. `tests/` holds
the integration tests: `reader_dispatch` (sources), `legacy_import`,
`documents_office`, `feature_surface` (every feature composing), and the live
suites `live_cortexdb` and `office_live`, which need a reachable CortexDB and
credentials from the environment and are named `live_*` so they are easy to
exclude.

## Architecture

[`docs/architecture/integrations.md`](../../docs/architecture/integrations.md)
describes documents, sources, safety and the legacy import in detail;
[`docs/architecture/cortex.md`](../../docs/architecture/cortex.md) covers the
engine and registry; [`docs/architecture/README.md`](../../docs/architecture/README.md)
indexes the rest.
