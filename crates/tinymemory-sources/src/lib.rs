//! Engine-neutral memory-source contracts (#18 §B4).
//!
//! What a configured source *is* ([`types::MemorySourceEntry`]), what a reader
//! hands back when it lists ([`types::SourceItem`]) and when it fetches
//! ([`types::SourceContent`]).
//!
//! # Why these are not the contract crate's types of the same name
//!
//! `tinymemory-api` has a `SourceItem` and a `SourceKind` already, and neither
//! is this one:
//!
//! - `provider::types::SourceItem` is an **ingest** entry — it carries content,
//!   because it is what `MemorySourceSink` accepts. The one here is a
//!   **listing** entry, deliberately without content: `list_items` enumerates
//!   cheaply and `read_item` fetches per item, so a reader never downloads a
//!   repository to tell you what is in it.
//! - `chunks::SourceKind` is `Chat | Email | Document` — the kind of *content* a
//!   chunk came from. The one here is `Composio | Folder | GithubRepo | …` — the
//!   kind of *connector*.
//!
//! They are different concepts that happen to share two names. Renaming was
//! considered and rejected: the pairs never appear in one scope, and the churn
//! would be ~150 call sites here plus 24 in OpenHuman to fix a collision that
//! does not bite. Recorded so the next reader does not mistake the duplication
//! for an oversight.
//!
//! # Why a crate rather than the contract
//!
//! This is a *host-side ingestion* protocol, upstream of the driver contract:
//! a reader produces listings, the pipeline turns them into
//! `provider::types::SourceItem`s, and only then does a driver see them. Putting
//! it in `tinymemory-api` would widen the driver contract with something no
//! driver implements.

// The crate's lints hold library code to no `unwrap`/`expect`. Tests are held
// to a different standard on purpose: a panic in a test *is* the failure
// report, and rewriting 159 assertions into `let ... else` would obscure what
// each one checks. Scoped to `cfg(test)` so the library rule is untouched.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod raw_kind;
pub mod readers;
pub mod reconcile;
pub mod registry;
pub mod types;
pub mod validation;

/// What a reader returns.
///
/// The engine spelled this `MemoryEngineResult`; the error is the contract's
/// [`tinymemory_api::error::MemoryError`], so a reader now fails in the same
/// vocabulary as the driver that will store what it read.
pub type SourceResult<T> = Result<T, tinymemory_api::error::MemoryError>;

/// Largest file a folder source will read.
///
/// Moved with the readers: it is a reader policy, and the engine's config was
/// only its previous address.
pub const FOLDER_FILE_SIZE_CAP_BYTES: u64 = 10 * 1024 * 1024;

pub use registry::{
    apply_kind_defaults, memory_sync_defaults_for_toolkit, ComposioUpsertTarget, SourceRegistry,
};
pub use types::{
    ContentType, MemorySourceEntry, MemorySourcePatch, SourceContent, SourceItem, SourceKind,
};
