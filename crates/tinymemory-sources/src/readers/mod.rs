//! Source readers: the [`SourceReader`] trait plus local implementations.
//!
//! A reader knows how to *list* the items available in a source and *read* the
//! content of one item. The trait is intentionally narrow so the host can drive
//! ingestion uniformly across every source kind.
//!
//! ## Ownership boundary
//!
//! Fetching and parsing a source is engine work, so the `github_repo`,
//! `rss_feed`, and `web_page` readers live here behind the `sync` feature
//! alongside the always-compiled local kinds ([`folder::FolderReader`],
//! [`conversation::ConversationReader`]). What TinyCortex still does **not**
//! own is *when* a network read happens: scheduling, polling cadence, OAuth,
//! credentials, and egress/cost budgeting stay with the host.
//!
//! That is why [`reader_for`] and [`is_locally_readable`] draw their line at
//! **local vs. network**, not at implemented vs. absent. A network reader is
//! constructed explicitly (`github::GithubReader`, `rss::RssReader`,
//! `web_page::WebPageReader`) by a caller that has already decided the fetch is
//! allowed; it is never handed out by the kind-dispatch that
//! the workspace sync loop drives on a timer. A `None` from
//! [`reader_for`] therefore still means "route this through the host's sync
//! runner", which is what keeps the host in charge of hitting the network.
//!
//! `composio` and `twitter_query` are represented by placeholder readers
//! ([`composio::ComposioReader`], [`twitter::TwitterReader`]): the former lists
//! a connection as a single sync target because its data arrives through the
//! credentialed provider pipeline, the latter validates the source and reports
//! that the API integration is not configured. Neither fetches anything, and
//! neither is handed out by [`reader_for`].
//!
//! A host servicing an *explicit user request* (not a timer) that wants one
//! reader for any kind uses [`reader_for_request`].

pub mod composio;
pub mod conversation;
pub mod folder;
#[cfg(feature = "network")]
pub mod github;
#[cfg(feature = "network")]
pub mod rss;
pub mod twitter;
#[cfg(feature = "network")]
pub mod web_page;

/// SSRF guard + fetch hygiene shared by the sync-gated network readers
/// (`web_page`, `rss`). See the `ssrf` module docs.
///
/// Public because it is not only the readers that fetch a URL any more:
/// `tinymemory-documents` pulls a page into memory on request, and a second
/// SSRF implementation in the same workspace would mean one of the two is the
/// weaker without anyone knowing which.
#[cfg(feature = "network")]
pub mod ssrf;

use async_trait::async_trait;

use crate::SourceResult;
use tinymemory_api::error::MemoryError;

use super::types::{MemorySourceEntry, SourceContent, SourceItem, SourceKind};

/// A reader that can list items and read content from a memory source.
///
/// Implementations are synchronous internally but expose an async surface so a
/// network-backed reader (host-owned) can satisfy the same contract.
#[async_trait]
pub trait SourceReader: Send + Sync {
    /// The [`SourceKind`] this reader serves.
    fn kind(&self) -> SourceKind;

    /// List the items currently available in `source`.
    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        workspace: &std::path::Path,
    ) -> SourceResult<Vec<SourceItem>>;

    /// Read the content of a single item by its reader-scoped `item_id`.
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &std::path::Path,
    ) -> SourceResult<SourceContent>;
}

/// Whether a kind can be read from local state alone, with no network egress.
///
/// Network-backed kinds return `false` even when this build ships their reader
/// (see the module docs): the host decides when a fetch is allowed.
pub fn is_locally_readable(kind: &SourceKind) -> bool {
    matches!(kind, SourceKind::Folder | SourceKind::Conversation)
}

/// Get the reader for a source kind that is safe to drive on a timer.
///
/// Returns `Some` for [`SourceKind::Folder`] and [`SourceKind::Conversation`].
/// Network-backed kinds (`composio`, `github_repo`, `rss_feed`, `web_page`,
/// `twitter_query`) return `None` so the caller defers to the host's sync
/// runner — including the three whose readers this crate now implements, which
/// callers construct by name once the host has authorized the fetch.
pub fn reader_for(kind: &SourceKind) -> Option<Box<dyn SourceReader>> {
    match kind {
        SourceKind::Folder => Some(Box::new(folder::FolderReader)),
        SourceKind::Conversation => Some(Box::new(conversation::ConversationReader)),
        SourceKind::Composio
        | SourceKind::GithubRepo
        | SourceKind::TwitterQuery
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
pub fn reader_for_request(kind: &SourceKind) -> Box<dyn SourceReader> {
    match kind {
        SourceKind::Composio => Box::new(composio::ComposioReader),
        SourceKind::Conversation => Box::new(conversation::ConversationReader),
        SourceKind::Folder => Box::new(folder::FolderReader),
        SourceKind::GithubRepo => Box::new(github::GithubReader),
        SourceKind::TwitterQuery => Box::new(twitter::TwitterReader),
        SourceKind::RssFeed => Box::new(rss::RssReader::new()),
        SourceKind::WebPage => Box::new(web_page::WebPageReader),
    }
}

/// Wrap a reader's plain-string failure as a [`MemoryError`].
///
/// The network readers below carry their diagnostics as `String` internally.
/// [`MemoryError::Other`] is `#[error(transparent)]`, so `to_string()` on the
/// result reproduces the original message byte-for-byte — callers that match on
/// reader error text keep working unchanged.
pub(crate) fn into_engine_error(message: String) -> MemoryError {
    MemoryError::Other(anyhow::anyhow!(message))
}
