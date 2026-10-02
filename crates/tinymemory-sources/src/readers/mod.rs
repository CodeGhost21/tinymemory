//! Source readers: the [`SourceReader`] trait plus its implementations.
//!
//! A reader knows how to *list* the items available in a source, *read* the
//! content of one item, and turn one item into a
//! [`StoreItem`](tinymemory_api::StoreItem) ([`SourceReader::read_store_item`]).
//! The trait is intentionally narrow so the host can drive ingestion uniformly
//! across every source kind.
//!
//! ## Ownership boundary
//!
//! The local kinds ([`folder::FolderReader`], [`file::FileReader`],
//! [`conversation::ConversationReader`]) are always compiled. The network
//! kinds (`github`, `rss`, `web_page`, plus [`fetch`](crate::fetch)) sit
//! behind the `network` feature. What this crate does **not** own is *when*
//! a network read happens: scheduling, polling cadence, OAuth, credentials,
//! and egress/cost budgeting stay with the host.
//!
//! That is why [`reader_for`] and [`is_locally_readable`] draw their line at
//! **local vs. network**, not at implemented vs. absent. A network reader is
//! constructed explicitly (`github::GithubReader`, `rss::RssReader`,
//! `web_page::WebPageReader`) by a caller that has already decided the fetch is
//! allowed; it is never handed out by the kind-dispatch that a sync loop drives
//! on a timer. A `None` from [`reader_for`] therefore means "route this through
//! the host's sync runner", which keeps the host in charge of the network.
//!
//! `composio` is represented by a placeholder reader
//! ([`composio::ComposioReader`]): its data arrives through the credentialed
//! provider pipeline, and [`crate::composio`] turns those payloads into items.
//!
//! A host servicing an *explicit user request* (not a timer) that wants one
//! reader for any kind uses `reader_for_request`.

pub mod composio;
pub mod conversation;
pub mod file;
pub mod folder;
#[cfg(feature = "network")]
pub mod github;
pub mod local_file;
#[cfg(feature = "network")]
pub mod rss;
#[cfg(feature = "network")]
pub mod web_page;

/// SSRF guard + fetch hygiene shared by the network readers and
/// [`crate::fetch`]. See the `ssrf` module docs.
///
/// Public so a host fetching a user-supplied URL by other means applies the
/// same policy rather than a second, weaker one.
#[cfg(feature = "network")]
pub mod ssrf;

use std::path::Path;

use async_trait::async_trait;
use tinymemory_api::StoreItem;
use tinymemory_documents::DocumentConverter;

use crate::error::Result;
use crate::items;

use super::types::{MemorySourceEntry, SourceContent, SourceItem, SourceKind};

/// A reader that can list items and read content from a memory source.
///
/// Implementations may be synchronous internally but expose an async surface
/// so a network-backed reader satisfies the same contract.
#[async_trait]
pub trait SourceReader: Send + Sync + std::fmt::Debug {
    /// The [`SourceKind`] this reader serves.
    fn kind(&self) -> SourceKind;

    /// List the items currently available in `source`.
    ///
    /// # Errors
    ///
    /// The reader's failure: missing configuration ([`crate::Error::Invalid`]),
    /// a missing root ([`crate::Error::NotFound`]), or a network failure.
    async fn list_items(&self, source: &MemorySourceEntry, workspace: &Path)
    -> Result<Vec<SourceItem>>;

    /// Read the content of a single item by its reader-scoped `item_id`.
    ///
    /// # Errors
    ///
    /// The reader's failure: an unknown item ([`crate::Error::NotFound`]), a
    /// path that escapes its root ([`crate::Error::PathEscape`]), a body over
    /// the size cap, or a network failure.
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<SourceContent>;

    /// Read one listed item as a [`StoreItem`] with its
    /// [`MemoryMeta`](tinymemory_api::MemoryMeta) filled for this kind.
    ///
    /// The default reads the item with [`Self::read_item`] and maps it through
    /// [`items::content_item`]. Local readers override it to work from the raw
    /// file (so a host converter can handle PDF or DOCX) or the parsed thread.
    ///
    /// # Errors
    ///
    /// Whatever [`Self::read_item`] returns, plus [`crate::Error::Document`]
    /// when conversion fails and [`crate::Error::Invalid`] for an item with no
    /// text.
    async fn read_store_item(
        &self,
        source: &MemorySourceEntry,
        item: &SourceItem,
        workspace: &Path,
        converter: &dyn DocumentConverter,
    ) -> Result<StoreItem> {
        let _ = converter;
        let content = self.read_item(source, &item.id, workspace).await?;
        items::content_item(source, content, item.updated_at_ms)
    }
}

/// Whether a kind can be read from local state alone, with no network egress.
///
/// Network-backed kinds return `false` even when this build ships their reader
/// (see the module docs): the host decides when a fetch is allowed.
#[must_use]
pub fn is_locally_readable(kind: &SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::Folder | SourceKind::File | SourceKind::Conversation
    )
}

/// Get the reader for a source kind that is safe to drive on a timer.
///
/// Returns `Some` for [`SourceKind::Folder`], [`SourceKind::File`] and
/// [`SourceKind::Conversation`]. Network-backed kinds (`composio`,
/// `github_repo`, `rss_feed`, `web_page`) return `None` so the caller defers to
/// the host's sync runner, which constructs those readers once it has
/// authorized the fetch.
#[must_use]
pub fn reader_for(kind: &SourceKind) -> Option<Box<dyn SourceReader>> {
    match kind {
        SourceKind::Folder => Some(Box::new(folder::FolderReader)),
        SourceKind::File => Some(Box::new(file::FileReader)),
        SourceKind::Conversation => Some(Box::new(conversation::ConversationReader)),
        SourceKind::Composio
        | SourceKind::GithubRepo
        | SourceKind::RssFeed
        | SourceKind::WebPage => None,
    }
}

/// Get a reader for **any** source kind, for a caller servicing an explicit user
/// request naming one source (an RPC handler), not a timer.
///
/// Unlike [`reader_for`] this hands out the network readers, so the caller has
/// already decided the fetch is allowed. **Do not reuse it from a polling
/// loop**: the host stays in charge of egress, OAuth and cost budgeting by
/// constructing a network reader deliberately there.
#[cfg(feature = "network")]
#[must_use]
pub fn reader_for_request(kind: &SourceKind) -> Box<dyn SourceReader> {
    match kind {
        SourceKind::Composio => Box::new(composio::ComposioReader),
        SourceKind::Conversation => Box::new(conversation::ConversationReader),
        SourceKind::Folder => Box::new(folder::FolderReader),
        SourceKind::File => Box::new(file::FileReader),
        SourceKind::GithubRepo => Box::new(github::GithubReader),
        SourceKind::RssFeed => Box::new(rss::RssReader::new()),
        SourceKind::WebPage => Box::new(web_page::WebPageReader),
    }
}
