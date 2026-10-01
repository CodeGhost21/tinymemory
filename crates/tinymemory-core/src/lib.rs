//! `tinymemory-core` — the engine-neutral memory subsystem, extracted from
//! OpenHuman's `src/openhuman/memory/`.
//!
//! This crate owns the *substance* of a memory subsystem: the SQLite/vector
//! store, the markdown summary tree, the provider sync pipelines, ingestion,
//! recall/query/search, the ingest queue, conversations, people, goals and the
//! tool-memory rules. It is host-neutral: nothing here names an OpenHuman
//! type.
//!
//! What deliberately stays in the host (see the repository README's split):
//! the RPC surface, agent tools, security policy and the taint/scope guard,
//! credentials, schedulers, the event bus, and config mapping. The host
//! supplies those through the seam traits in [`tinymemory_api::host`].

/// The host's configuration, as this crate sees it.
///
/// This is the load-bearing trick of the whole extraction. Before the move,
/// every function in this crate took `config: &crate::Config`
/// — a concrete host struct. Aliasing `Config` to the *trait object* means those
/// signatures read `config: &Config` exactly as they did before, and the host's
/// concrete `Config` unsize-coerces at each of the ~550 call sites on the other
/// side of the seam with no edit at all.
///
/// What did change inside this crate: field reads became method calls
/// (`config.workspace_dir()` → `config.workspace_dir()`), by-value `Config`
/// parameters became `Arc<Config>`, and `TestHostConfig::default()` in tests became
/// [`tinymemory_api::host::test_support::TestHostConfig`], which cannot be built
/// from a trait object.
///
/// See [`tinymemory_api::host::MemoryHostConfig`] for the accessor surface and
/// why its return types are shaped the way they are.
pub type Config = dyn tinymemory_api::host::MemoryHostConfig;

pub mod backfill;
pub mod chat;
pub mod chat_host;
pub mod config_loader;
pub(crate) mod corruption;
pub mod diff;
pub mod embedding_adapter;
pub mod embedding_host;
pub mod engine;
/// The engine module under its pre-#18 name.
///
/// OpenHuman's shim re-exports `tinymemory_core::tinycortex` wholesale
/// (`memory/mod.rs`), and 25 call sites reach through that path. The rename to
/// `engine` (#18 §C1) would otherwise make the next pin bump a coordinated
/// two-repo edit for zero behavioural gain. An alias, not a module: one item
/// to delete once downstream says `engine`.
#[doc(hidden)]
pub use engine as tinycortex;
/// The process-global event sink — now owned by [`tinymemory_api::events`].
///
/// It moved with `sync_events`: both are things this
/// crate's own ownership note (`engine/mod.rs`) lists on the **host** side of
/// the split, and a host that reaches memory only over the TinyBus module must
/// be able to install a sink without linking this crate. Re-exported at the
/// historical path so existing `tinymemory_core::events::…` call sites resolve
/// unchanged.
pub use tinymemory_api::events;
pub mod global;
pub mod ingest_pipeline;
pub mod ingestion;
pub mod learning_candidate;
pub mod nlp_host;
pub mod observability;
pub mod people;
pub mod preferences;
pub mod queue;
pub mod remember;
pub mod scheduler_gate;
pub mod search;
pub mod shutdown;
pub mod source_scope;
pub mod sources;
pub mod store;
pub mod sync;
/// The memory-sync lifecycle vocabulary — now owned by
/// [`tinymemory_api::sync_events`]. See the note on [`events`].
pub use tinymemory_api::sync_events;
pub mod test_env_lock;
#[cfg(test)]
pub(crate) mod test_seams;
pub mod thread_context;
pub mod tool_memory;
pub mod traits;
pub mod tree;
pub mod tree_policy;
pub mod tree_source;
pub mod util;

// The host seam, re-exported so downstream code takes one dependency. These are
// the *only* types this crate accepts from its host.
pub use tinymemory_api::host::{
    format_embedding_signature, ComposioMode, EmbeddingProvider, MemoryEvent, MemoryEventSink,
    MemoryHostConfig, NoopEmbedding, NoopEventSink, COMPOSIO_MODE_BACKEND, COMPOSIO_MODE_DIRECT,
    DEFAULT_MEMORY_SYNC_INTERVAL_SECS,
};

pub use ingestion::{
    ExtractedEntity, ExtractedRelation, ExtractionMode, IngestionJob, IngestionQueue,
    IngestionState, IngestionStatusSnapshot, MemoryIngestionConfig, MemoryIngestionRequest,
    MemoryIngestionResult, DEFAULT_MEMORY_EXTRACTION_MODEL,
};
pub use store::types::NamespaceDocumentInput;
pub use store::{MemoryClient, UnifiedMemory};
pub use traits::{Memory, MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary, RecallOpts};
