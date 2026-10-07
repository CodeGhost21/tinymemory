//! The five import sections and how each pages through its legacy table.
//!
//! A section scans its table in pages ordered by a stable key and returns one
//! [`Scanned`] per row (or per group of rows): the key, as a [`Mark`] that
//! advances a [`Checkpoint`], and the item the row maps to, or `None` when the
//! row is skipped (it belongs to another section, is a raw event, is blank,
//! or was dropped). Skipped rows still advance the scan, so a page of skipped
//! rows never stalls the iterator.

mod chunks;
mod episodic;
mod memory_docs;
mod profile;

pub use memory_docs::EXTERNAL_SYNC_TAG;

use tinymemory_api::{MemoryMeta, SourceKind, StoreItem};

use crate::import::checkpoint::{Checkpoint, ChunkCursor};
use crate::import::error::Result;
use crate::import::workspace::LegacyWorkspace;

/// One import section, in the order [`ORDER`] walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    /// `memory_docs` rows that are documents.
    Documents,
    /// `memory_tree/chunks.db` sources.
    Chunks,
    /// `episodic_log` threads.
    Conversations,
    /// `memory_docs` rows that are learnings or `global`.
    Learnings,
    /// `user_profile` facets.
    Profile,
}

/// The fixed section order.
pub(crate) const ORDER: [Section; 5] = [
    Section::Documents,
    Section::Chunks,
    Section::Conversations,
    Section::Learnings,
    Section::Profile,
];

/// A scanned key, naming the checkpoint field it advances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A `memory_docs.document_id` in the documents section.
    Document(String),
    /// A chunk source.
    Chunk(ChunkCursor),
    /// An `episodic_log.session_id`.
    Conversation(String),
    /// A `memory_docs.document_id` in the learnings section.
    Learning(String),
    /// A `user_profile.facet_id`.
    Profile(String),
}

impl Mark {
    /// Records this key as the section's position in `checkpoint`.
    pub(crate) fn apply(self, checkpoint: &mut Checkpoint) {
        match self {
            Self::Document(id) => checkpoint.documents = Some(id),
            Self::Chunk(cursor) => checkpoint.chunks = Some(cursor),
            Self::Conversation(id) => checkpoint.conversations = Some(id),
            Self::Learning(id) => checkpoint.learnings = Some(id),
            Self::Profile(id) => checkpoint.profile = Some(id),
        }
    }
}

/// One scanned row or row group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scanned {
    /// Its key.
    pub(crate) mark: Mark,
    /// The item it maps to, or `None` when skipped.
    pub(crate) item: Option<StoreItem>,
}

impl Section {
    /// The next page of at most `limit` keys after this section's position in
    /// `scan`. An empty page means the section is exhausted.
    pub(crate) fn page(
        self,
        ws: &LegacyWorkspace,
        scan: &Checkpoint,
        limit: usize,
    ) -> Result<Vec<Scanned>> {
        match self {
            Self::Documents => memory_docs::documents(ws, scan.documents.as_deref(), limit),
            Self::Chunks => chunks::page(ws, scan.chunks.as_ref(), limit),
            Self::Conversations => episodic::page(ws, scan.conversations.as_deref(), limit),
            Self::Learnings => memory_docs::learnings(ws, scan.learnings.as_deref(), limit),
            Self::Profile => profile::page(ws, scan.profile.as_deref(), limit),
        }
    }
}

impl Section {
    /// How many items this section yields, counted with one aggregate
    /// query and without reading any chunk body from disk. See
    /// [`crate::import::LegacyCounts`] for where it may overcount.
    pub(crate) fn count(self, ws: &LegacyWorkspace) -> Result<u64> {
        match self {
            Self::Documents => memory_docs::count(ws, false),
            Self::Chunks => chunks::count(ws),
            Self::Conversations => episodic::count(ws),
            Self::Learnings => memory_docs::count(ws, true),
            Self::Profile => profile::count(ws),
        }
    }
}

/// A SQL condition true when `column` holds text other than ASCII
/// whitespace: the importer skips blank rows.
pub(crate) fn has_text(column: &str) -> String {
    format!("trim({column}, ' ' || char(9) || char(10) || char(13)) <> ''")
}

/// A SQLite count as `u64`.
pub(crate) fn count_of(count: i64) -> u64 {
    u64::try_from(count).unwrap_or(0)
}

/// Metadata every imported item starts from: `source.kind = Import`, the
/// section-scoped legacy id, and the workspace path.
pub(crate) fn import_meta(ws: &LegacyWorkspace, legacy_id: String) -> MemoryMeta {
    MemoryMeta {
        workspace: Some(ws.workspace_id.clone()),
        ..MemoryMeta::from_source(SourceKind::Import, Some(legacy_id))
    }
}

/// `limit` as a SQLite integer.
pub(crate) fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// Appends `tag` unless it is already present.
pub(crate) fn push_unique(tags: &mut Vec<String>, tag: String) {
    if !tags.contains(&tag) {
        tags.push(tag);
    }
}
