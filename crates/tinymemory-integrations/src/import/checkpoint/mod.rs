//! Resumable import: the per-section cursor a host persists between runs.
//!
//! An import walks the legacy store in a fixed section order (documents,
//! chunks, conversations, learnings, profile) and, within a section, by a
//! stable key. A [`Checkpoint`] records the key of the last item yielded in
//! each section; [`crate::import::LegacyWorkspace::items_from`] skips everything at or
//! before it. Every [`ImportedItem`] carries the checkpoint to persist once
//! that item is stored, so a crash between two stores re-yields at most the
//! one item that was not acknowledged, which the engine then treats as a
//! replay.

use serde::{Deserialize, Serialize};
use tinymemory_api::StoreItem;

use crate::import::error::Result;

/// The last yielded key in each section of a legacy import.
///
/// `None` means the section has not yielded anything yet. Keys compare as
/// SQLite `TEXT` (byte order), the same order the importer walks them in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkpoint {
    /// Last `memory_docs.document_id` yielded as a document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documents: Option<String>,
    /// Last `mem_tree_chunks` source yielded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<ChunkCursor>,
    /// Last `episodic_log.session_id` yielded as a conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversations: Option<String>,
    /// Last `memory_docs.document_id` yielded as a learning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learnings: Option<String>,
    /// Last `user_profile.facet_id` yielded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl Checkpoint {
    /// Whether nothing has been yielded yet: resuming from this checkpoint
    /// imports everything.
    #[must_use]
    pub fn is_start(&self) -> bool {
        *self == Self::default()
    }

    /// Encodes the checkpoint as JSON for the host to persist.
    ///
    /// # Errors
    ///
    /// [`crate::import::Error::Json`] if serialisation fails, which a checkpoint of
    /// plain strings does not do in practice.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Decodes a checkpoint the host persisted with [`Checkpoint::to_json`].
    ///
    /// # Errors
    ///
    /// [`crate::import::Error::Json`] if `json` is not a checkpoint.
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

/// The key of one ingested source in `memory_tree/chunks.db`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChunkCursor {
    /// `mem_tree_chunks.source_kind` (`chat`, `email`, `document`).
    pub source_kind: String,
    /// `mem_tree_chunks.source_id`.
    pub source_id: String,
}

/// One imported item and the checkpoint to persist after storing it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedItem {
    /// The item to store.
    pub item: StoreItem,
    /// Resume point covering this item and everything before it.
    pub checkpoint: Checkpoint,
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
