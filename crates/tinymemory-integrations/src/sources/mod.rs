//! Source readers for TinyMemory: turn a folder, a file, a web page, a GitHub
//! repository, an RSS feed, a Composio toolkit payload or a local conversation
//! into [`StoreItem`](tinymemory_api::StoreItem)s.
//!
//! - **Configuration** — what a source *is* ([`MemorySourceEntry`], keyed by
//!   [`SourceKind`]), its partial updates ([`MemorySourcePatch`]), field rules
//!   ([`validation`]), the host's persisted registry ([`SourceRegistry`]) and
//!   Composio reconciliation ([`reconcile`]).
//! - **Readers** — [`readers::SourceReader`] lists a source's items and reads
//!   one. Local readers (folder, file, conversation) are always compiled; the
//!   network readers (GitHub, RSS, web page) and `fetch` sit behind the
//!   `network` feature, behind one SSRF guard (`readers::ssrf`).
//! - **Items** — [`items`] maps reader output to `StoreItem`s with
//!   [`MemoryMeta`](tinymemory_api::MemoryMeta) filled per kind; every text
//!   body is converted to markdown through `tinymemory-documents`.
//! - **Composio** — [`composio`] normalises toolkit payloads (Gmail, Slack,
//!   GitHub, Linear, Notion, ClickUp) and maps them to items.
//!
//! Scheduling, credentials and egress budgets stay with the host: this crate
//! reads when asked.
//!
//! # Example
//!
//! ```
//! use tinymemory_api::{SourceKind as ApiKind, StoreItem};
//! use tinymemory_integrations::documents::ConverterChain;
//! use tinymemory_integrations::sources::{items, readers, MemorySourceEntry, SourceKind};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let workspace = tempfile::tempdir()?;
//! std::fs::create_dir(workspace.path().join("notes"))?;
//! std::fs::write(workspace.path().join("notes/plan.md"), "# Plan\n\nShip v2.")?;
//! std::fs::write(workspace.path().join("notes/build.rs"), "fn main() {}\n")?;
//!
//! let mut entry = MemorySourceEntry::new("src_notes", SourceKind::Folder, "Notes");
//! entry.path = Some("notes".into());
//! let reader = readers::reader_for(&entry.kind).expect("folders are local");
//!
//! let converter = ConverterChain::default();
//! let mut collected =
//!     items::collect_items(reader.as_ref(), &entry, workspace.path(), &converter).await?;
//! collected.items.sort_by_key(|item| item.meta().file_path.clone());
//!
//! let rust = &collected.items[0];
//! assert_eq!(rust.meta().source.kind, ApiKind::Folder);
//! assert_eq!(rust.meta().source.id.as_deref(), Some("src_notes"));
//! assert_eq!(rust.meta().language.as_deref(), Some("rust"));
//! let StoreItem::Document { title, .. } = &collected.items[1] else {
//!     unreachable!("folder sources produce documents");
//! };
//! assert_eq!(title.as_deref(), Some("Plan"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Feature flags
//!
//! - `network` — the GitHub, RSS and web-page readers, `fetch`, and the
//!   SSRF guard. Off by default, so a host that only reads local sources
//!   links no HTTP stack.

pub mod composio;
pub mod error;
#[cfg(feature = "sources-network")]
pub mod fetch;
pub mod items;
pub mod raw_kind;
pub mod readers;
pub mod reconcile;
pub mod registry;
pub mod types;
pub mod validation;

/// Largest file a folder or file source will read.
pub const FOLDER_FILE_SIZE_CAP_BYTES: u64 = 10 * 1024 * 1024;

pub use error::{Error, Result};
pub use items::{collect_items, content_item, conversation_item, file_item, Collected};
pub use registry::{
    apply_kind_defaults, memory_sync_defaults_for_toolkit, ComposioUpsertTarget, SourceRegistry,
};
pub use types::{
    ContentType, MemorySourceEntry, MemorySourcePatch, SourceContent, SourceItem, SourceKind,
};
