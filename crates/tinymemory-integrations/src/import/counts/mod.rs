//! [`LegacyCounts`]: how much a legacy workspace holds, without importing it.
//!
//! A host sizes an import (and decides whether there is anything to import
//! at all) from [`crate::import::LegacyWorkspace::counts`]: one aggregate
//! query per section, no item decoded and no chunk body read from disk. That
//! is fast where a full [`crate::import::LegacyWorkspace::items`] pass reads
//! every body.

use serde::{Deserialize, Serialize};

use crate::import::error::Result;
use crate::import::sections::{ORDER, Section};
use crate::import::workspace::LegacyWorkspace;

/// How many items each section of a legacy workspace yields.
///
/// Exact for every section but two edge cases, where it may count an item
/// that [`crate::import::LegacyWorkspace::items`] then skips: a row whose
/// text is only whitespace other than space, tab, CR or LF, and a chunk
/// source whose bodies all resolve to blank files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyCounts {
    /// `memory_docs` documents.
    pub documents: u64,
    /// `memory_tree/chunks.db` sources.
    pub chunks: u64,
    /// `episodic_log` threads.
    pub conversations: u64,
    /// `memory_docs` learnings and `global` rows.
    pub learnings: u64,
    /// Live `user_profile` facets.
    pub profile: u64,
}

impl LegacyCounts {
    /// Every item, across the sections.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.documents + self.chunks + self.conversations + self.learnings + self.profile
    }

    /// Whether the workspace holds nothing to import.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// Counts every section of `ws`.
pub(crate) fn count(ws: &LegacyWorkspace) -> Result<LegacyCounts> {
    let mut counts = LegacyCounts::default();
    for section in ORDER {
        let n = section.count(ws)?;
        match section {
            Section::Documents => counts.documents = n,
            Section::Chunks => counts.chunks = n,
            Section::Conversations => counts.conversations = n,
            Section::Learnings => counts.learnings = n,
            Section::Profile => counts.profile = n,
        }
    }
    Ok(counts)
}
