//! Source readers: the [`SourceReader`] trait plus its implementations.
//!
//! A reader knows how to *list* the items available in a source, *read* the
//! content of one item, and turn one item into a
//! [`StoreItem`] ([`SourceReader::read_store_item`]).
//! The trait is intentionally narrow so the host can drive ingestion uniformly
//! across every source kind.
//!
//! ## Ownership boundary
//!
//! Every reader here is local: [`folder::FolderReader`], [`file::FileReader`]
//! and [`conversation::ConversationReader`] read the workspace and nothing
//! else. Network-backed sources (Composio toolkits, GitHub, RSS, web pages)
//! are not part of this crate. [`is_locally_readable`] and [`reader_for`]
//! therefore cover every [`SourceKind`].

pub mod conversation;
pub mod file;
pub mod folder;
pub mod local_file;

use std::path::Path;

use crate::documents::DocumentConverter;
use async_trait::async_trait;
use tinymemory_api::StoreItem;

use crate::sources::error::Result;
use crate::sources::items;

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
    /// The reader's failure: missing configuration ([`crate::sources::Error::Invalid`]),
    /// a missing root ([`crate::sources::Error::NotFound`]), or a network failure.
    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        workspace: &Path,
    ) -> Result<Vec<SourceItem>>;

    /// Read the content of a single item by its reader-scoped `item_id`.
    ///
    /// # Errors
    ///
    /// The reader's failure: an unknown item ([`crate::sources::Error::NotFound`]), a
    /// path that escapes its root ([`crate::sources::Error::PathEscape`]), a body over
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
    /// Whatever [`Self::read_item`] returns, plus [`crate::sources::Error::Document`]
    /// when conversion fails and [`crate::sources::Error::Invalid`] for an item with no
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
/// Every [`SourceKind`] is local, so this is always `true`; it stays as the
/// single place a host asks the question.
#[must_use]
pub fn is_locally_readable(kind: &SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::Folder | SourceKind::File | SourceKind::Conversation
    )
}

/// Get the reader for a source kind.
///
/// Returns the folder, file or conversation reader.
#[must_use]
pub fn reader_for(kind: &SourceKind) -> Option<Box<dyn SourceReader>> {
    match kind {
        SourceKind::Folder => Some(Box::new(folder::FolderReader)),
        SourceKind::File => Some(Box::new(file::FileReader)),
        SourceKind::Conversation => Some(Box::new(conversation::ConversationReader)),
    }
}
