//! The TinyMemory v2 contract.
//!
//! A host needs three things from memory:
//!
//! - **Recall** — a question in, a synthesised answer with citations out
//!   ([`MemoryEngine::recall`]).
//! - **Fetch** — raw keyword, vector or hybrid retrieval over stored items,
//!   filtered by metadata ([`MemoryEngine::fetch`]).
//! - **Store** — ingest a document, a conversation or a learning, each with
//!   typed [`MemoryMeta`] ([`MemoryEngine::store`]).
//!
//! plus [`MemoryEngine::list`] and [`MemoryEngine::forget`] to page through
//! and remove what was stored, and [`MemoryEngine::explore`] and
//! [`MemoryEngine::get`] for explorers: counts of stored items per metadata
//! [`Facet`], and items read whole by id ([`explore`]).
//!
//! Every item lives at one [`Namespace`] node (the root, an agent, a team, a
//! nested sub-agent); a [`Reach`] in the filter says which nodes a read sees
//! ([`namespace`]).
//! An engine advertises what it offers through its
//! [`EngineDescriptor`]; a fetch mode it does not list fails with
//! [`Error::Unsupported`].
//!
//! This crate performs no I/O. Engines live in their own crates
//! (`tinymemory-cortex`), and the `tinymemory` facade builds one from
//! configuration.
//!
//! # Example
//!
//! ```
//! use tinymemory_api::{ItemKind, MemoryMeta, MetaFilter, SourceKind, StoreItem};
//!
//! let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("notes".into()));
//! meta.file_path = Some("/notes/rust/ownership.md".into());
//! let item = StoreItem::document("Ownership moves values.", meta);
//! item.validate()?;
//!
//! let filter = MetaFilter {
//!     file_path: Some("/notes/rust".into()),
//!     ..MetaFilter::kinds([ItemKind::Document])
//! };
//! assert!(filter.matches(item.kind(), item.meta()));
//! # Ok::<(), tinymemory_api::Error>(())
//! ```

pub mod engine;
pub mod error;
pub mod explore;
pub mod item;
pub mod meta;
pub mod namespace;
pub mod query;

pub use engine::{EngineDescriptor, EngineHealth, MAX_STORE_MANY, MemoryEngine, validate_many};
pub use error::{Error, Result};
pub use explore::{ExplorePage, ExploreRequest, Facet, FacetBucket, GetRequest};
pub use item::{DocumentBody, ItemId, ItemKind, LearningKind, Role, StoreItem, StoreReceipt, Turn};
pub use meta::{MemoryMeta, MetaFilter, SourceKind, SourceRef, ToolCallRef, TurnRange};
pub use namespace::{Namespace, Reach, Segment, SegmentKind};
pub use query::{
    Citation, FetchMode, FetchPage, FetchRequest, ForgetReport, ForgetTarget, Hit, ListPage,
    ListRequest, RecallAnswer, RecallRequest,
};

/// Re-exported so engines and hosts name the same `async_trait` and `chrono`
/// the contract was compiled with.
pub use async_trait::async_trait;
pub use chrono;
