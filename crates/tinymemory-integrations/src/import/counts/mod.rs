//! [`LegacyCounts`]: how much a legacy workspace holds, without importing it.
//!
//! A host sizes an import (and decides whether there is anything to import
//! at all) from [`crate::import::LegacyWorkspace::counts`]: one aggregate
//! query per `memory.db` section, and the chunk store's bodies read but no
//! item decoded. Every section counts with the very predicate its scan
//! filters by, so the counts are exactly what
//! [`crate::import::LegacyWorkspace::items`] yields.

use serde::{Deserialize, Serialize};

use crate::import::error::Result;
use crate::import::sections::{ORDER, Section};
use crate::import::workspace::LegacyWorkspace;

/// How many items each section of a legacy workspace yields: exactly what
/// [`crate::import::LegacyWorkspace::items`] yields from the same store.
///
/// Non-exhaustive: a later section adds a field without breaking a caller,
/// which reads fields, or builds one from `default()` and assigns them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
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
    /// `event_log` events.
    pub events: u64,
    /// `episodic_log` turns with a lesson.
    pub lessons: u64,
}

impl LegacyCounts {
    /// Every item, across the sections, saturating at `u64::MAX` (a value
    /// deserialised from elsewhere may hold anything).
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.documents,
            self.chunks,
            self.conversations,
            self.learnings,
            self.profile,
            self.events,
            self.lessons,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
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
            Section::Events => counts.events = n,
            Section::Lessons => counts.lessons = n,
        }
    }
    Ok(counts)
}
