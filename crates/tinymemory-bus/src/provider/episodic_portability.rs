//! The episodic-portability family: moving the conversation record between
//! drivers.
//!
//! A driver advertising
//! [`Capability::EpisodicPortability`](crate::capabilities::Capability::EpisodicPortability)
//! can hand out its whole episodic record a page at a time and take one in.
//! The mandatory export covers keyed records only, and the episodic family
//! has no way to enumerate what it holds — `session_turns` needs a session id
//! the caller does not have — so without this family a switch of drivers
//! leaves every past conversation behind.
//!
//! # Why a family of its own
//!
//! Adding these members to [`Capability::Episodic`](crate::capabilities::Capability::Episodic)
//! would be a major bump: negotiation is per family, so a driver that already
//! advertises episodic would be called for members it never implemented. A new
//! family is the minor-safe shape, and it states the difference truthfully — a
//! driver can record turns without being able to hand them over.
//!
//! # Why an import member, when the episodic family already writes
//!
//! The episodic writes are the lifecycle a live conversation goes through: a
//! turn gets a fresh id, a segment is created with one turn and grows one turn
//! at a time. A copy has finished segments to write, with their turn counts,
//! statuses and summaries, and replaying them through that lifecycle cannot
//! produce the same rows. Writing one record per call would also cost a remote
//! driver one request per turn and a wait for each.
//!
//! # Turn ids
//!
//! Segments and events name turns by id, so an import keeps a turn's id when
//! it can. When the target already holds a *different* turn under that id, the
//! turn is stored under a new one and the import reports the pair in
//! [`EpisodicImportOutcome::remapped`]; the caller rewrites the references in
//! the segments and events it imports afterwards. Parts are therefore imported
//! in [`EpisodicPart::ALL`] order.

use serde::{Deserialize, Serialize};

use super::episodic::{ConversationSegment, EpisodicEvent, EpisodicTurn};

/// One part of the episodic record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodicPart {
    /// Recorded turns.
    Turns,
    /// Conversation segments, whole.
    Segments,
    /// Events extracted from segments.
    Events,
    /// Segment embeddings, one per segment and model signature.
    SegmentEmbeddings,
}

impl EpisodicPart {
    /// Every part, in the order a copy imports them: turns first, because
    /// segments and events refer to turns by id.
    pub const ALL: [EpisodicPart; 4] = [
        EpisodicPart::Turns,
        EpisodicPart::Segments,
        EpisodicPart::Events,
        EpisodicPart::SegmentEmbeddings,
    ];

    /// Stable snake_case identifier, as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turns => "turns",
            Self::Segments => "segments",
            Self::Events => "events",
            Self::SegmentEmbeddings => "segment_embeddings",
        }
    }
}

impl std::fmt::Display for EpisodicPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One stored segment embedding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegmentEmbedding {
    /// The segment it embeds.
    pub segment_id: String,
    /// The embedding space it was computed in. A reader compares it with its
    /// own before using the vector, so a copied vector from another space is
    /// inert rather than wrong.
    pub model_signature: String,
    /// The vector.
    pub embedding: Vec<f32>,
    /// When it was computed, seconds since the epoch.
    pub created_at: f64,
}

/// The records of one part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "part", content = "records", rename_all = "snake_case")]
pub enum EpisodicRecords {
    /// Turns, each carrying its id.
    Turns(Vec<EpisodicTurn>),
    /// Segments, each in its full current state.
    Segments(Vec<ConversationSegment>),
    /// Extracted events.
    Events(Vec<EpisodicEvent>),
    /// Segment embeddings.
    SegmentEmbeddings(Vec<SegmentEmbedding>),
}

impl EpisodicRecords {
    /// No records of `part`.
    pub fn empty(part: EpisodicPart) -> Self {
        match part {
            EpisodicPart::Turns => Self::Turns(Vec::new()),
            EpisodicPart::Segments => Self::Segments(Vec::new()),
            EpisodicPart::Events => Self::Events(Vec::new()),
            EpisodicPart::SegmentEmbeddings => Self::SegmentEmbeddings(Vec::new()),
        }
    }

    /// Which part these records belong to.
    pub fn part(&self) -> EpisodicPart {
        match self {
            Self::Turns(_) => EpisodicPart::Turns,
            Self::Segments(_) => EpisodicPart::Segments,
            Self::Events(_) => EpisodicPart::Events,
            Self::SegmentEmbeddings(_) => EpisodicPart::SegmentEmbeddings,
        }
    }

    /// How many records there are.
    pub fn len(&self) -> usize {
        match self {
            Self::Turns(records) => records.len(),
            Self::Segments(records) => records.len(),
            Self::Events(records) => records.len(),
            Self::SegmentEmbeddings(records) => records.len(),
        }
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One page of an episodic export.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodicExportPage {
    /// The page's records, all of the part that was asked for.
    pub records: EpisodicRecords,
    /// Where the next page starts; `None` on the last page.
    ///
    /// Opaque to the caller, like the mandatory export's cursor. A page may
    /// hold fewer records than were asked for, or none, without being the
    /// last: only a missing cursor ends the walk.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A turn an import stored under an id other than the one it carried.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TurnIdRemap {
    /// The id the turn was exported with.
    pub from: i64,
    /// The id the target stored it under.
    pub to: i64,
}

/// What an episodic import wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodicImportOutcome {
    /// Records written.
    pub imported: u64,
    /// Records the target already held exactly as given.
    pub skipped: u64,
    /// Records the target refused.
    pub failed: u64,
    /// Why records were refused, naming the record and never its content.
    #[serde(default)]
    pub errors: Vec<String>,
    /// Turns stored under a new id because the target held a different turn
    /// under theirs. Empty for every part but [`EpisodicPart::Turns`].
    #[serde(default)]
    pub remapped: Vec<TurnIdRemap>,
}

#[cfg(test)]
#[path = "episodic_portability_tests.rs"]
mod tests;
