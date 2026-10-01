//! The episodic family: the turn-by-turn record of conversations.
//!
//! A driver advertising [`Capability::Episodic`](crate::capabilities::Capability::Episodic)
//! stores every chat turn in a full-text index and groups consecutive turns
//! into *conversation segments* — a segment being a stretch of turns about one
//! thing, closed when the subject changes and then summarised and embedded.
//!
//! # Why this is a family rather than a raw connection
//!
//! It is the last thing in the host that held a live `rusqlite::Connection`.
//! The archivist hook was handed one straight out of the session factory and
//! called free functions on it, which worked only because the engine was
//! compiled into this process. A connection cannot cross a bus, so either the
//! archivist's operations become a contract family or episodic capture stays
//! behind and the engine can never leave.
//!
//! What crosses is small and already typed: insert a turn, read a session's
//! turns back, and six segment-lifecycle operations. That was the whole surface
//! the raw connection was used for — no ad-hoc SQL, no schema knowledge.
//!
//! # The host keeps the policy, and it is not a small share
//!
//! Two of the archivist's eight engine calls took no connection at all —
//! deciding *whether* a new turn starts a new segment, and composing a summary
//! when no model is available. Neither touches storage, so both stay host-side
//! in `agent::harness::archivist`, next to the recap logic and the boundary
//! thresholds they read. This family persists what the host decided; it does
//! not decide.
//!
//! # `insert_turn` returns the id, and that is load-bearing
//!
//! The old code inserted a row and then issued `SELECT last_insert_rowid()` on
//! the same connection to learn its id. That is two operations relying on a
//! *connection-local* side effect, and it is wrong the moment anything else
//! shares the connection or the two hops cross a bus — `last_insert_rowid` is
//! per-connection state, so an interleaved insert from another task yields the
//! wrong id and the turn is filed under the wrong segment.
//!
//! Returning the id from the insert removes both problems at once: one round
//! trip instead of two, and no reliance on connection-local state. The engine
//! knows the id it just wrote; nothing else has to guess.

use async_trait::async_trait;

use crate::error::MemoryError;

// The value types this family exchanges. They are defined in `tinymemory-bus`
// — they cross the module boundary, and a host that only makes calls must be
// able to name them without compiling this trait — and re-exported here so
// every historical path keeps resolving and the types stay the same types.
pub use tinymemory_bus::provider::episodic::{
    ConversationSegment, EpisodicEvent, EpisodicTurn, EventKind, SegmentStatus,
};
pub use tinymemory_bus::provider::episodic_portability::{
    EpisodicExportPage, EpisodicImportOutcome, EpisodicPart, EpisodicRecords, SegmentEmbedding,
    TurnIdRemap,
};

/// The turn-by-turn conversation record.
///
/// Reached through [`MemoryProvider::as_episodic`](super::MemoryProvider::as_episodic).
#[async_trait]
pub trait MemoryEpisodic: Send + Sync {
    /// Record one turn, returning the id the driver assigned it.
    ///
    /// See the module docs for why the id comes back from the insert rather
    /// than from a follow-up `last_insert_rowid` call.
    ///
    /// # Errors
    ///
    /// Backend failures. A driver that refuses a turn on safety grounds (a
    /// secret-shaped session id, say) reports [`MemoryError::Invalid`] rather
    /// than silently dropping it — the host cannot notice a missing turn.
    async fn insert_turn(&self, turn: &EpisodicTurn) -> Result<i64, MemoryError>;

    /// Every recorded turn for one session, oldest first.
    ///
    /// # Errors
    ///
    /// Backend failures; an unknown session yields an empty vector.
    async fn session_turns(&self, session_id: &str) -> Result<Vec<EpisodicTurn>, MemoryError>;

    /// The open segment for a session, when there is one.
    ///
    /// # Errors
    ///
    /// Backend failures only; no open segment yields `Ok(None)`.
    async fn open_segment(
        &self,
        session_id: &str,
    ) -> Result<Option<ConversationSegment>, MemoryError>;

    /// Start a new segment at `start_episodic_id`.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the engine row it creates; a params struct would be its only caller's"
    )]
    async fn create_segment(
        &self,
        segment_id: &str,
        session_id: &str,
        namespace: &str,
        start_episodic_id: i64,
        start_seq: Option<u32>,
        start_timestamp: f64,
        now: f64,
    ) -> Result<(), MemoryError>;

    /// Extend a segment to include one more turn.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn append_turn(
        &self,
        segment_id: &str,
        episodic_id: i64,
        seq: Option<u32>,
        timestamp: f64,
        now: f64,
    ) -> Result<(), MemoryError>;

    /// Mark a segment closed. Idempotent.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn close_segment(&self, segment_id: &str, now: f64) -> Result<(), MemoryError>;

    /// Attach a summary to a segment.
    ///
    /// Separate from [`Self::close_segment`] because the two happen at
    /// different times: a segment closes the moment the subject changes, and is
    /// summarised afterwards by a model call that may be slow, may fail, or may
    /// fall back to a composed summary. Folding them together would mean either
    /// holding the segment open across an inference call or losing the summary
    /// when one fails.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn set_segment_summary(
        &self,
        segment_id: &str,
        summary: &str,
        now: f64,
    ) -> Result<(), MemoryError>;

    /// Store a segment's embedding under `model_signature`, replacing any
    /// vector already held for that signature.
    ///
    /// The signature must be produced the same way the rest of the store
    /// produces it — see `docs/specs/2026-08-13-memory-module-port.md` §3 for
    /// why a mismatch here is silent.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    /// Record one extracted event against its segment.
    ///
    /// Keyed on `event_id`, so re-running extraction over the same segment
    /// replaces its own rows rather than duplicating them.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn insert_event(&self, event: &EpisodicEvent) -> Result<(), MemoryError>;

    async fn upsert_segment_embedding(
        &self,
        segment_id: &str,
        model_signature: &str,
        embedding: &[f32],
        created_at: f64,
    ) -> Result<(), MemoryError>;

    /// Closed segments that carry no summary yet, oldest first, capped at
    /// `limit`.
    ///
    /// The recovery half of the recap contract (oh#6186). When a summariser
    /// fails, the caller is expected to write **nothing** — the driver does not
    /// substitute a fallback (see
    /// [`MemoryTree::summarise`](super::content::MemoryTree::summarise)), and a
    /// caller that persisted one would flip the segment to summarised and lose
    /// the fact that it never was. That leaves the segment closed with no summary, which
    /// is the marker this selects on: no schema addition, and no state a driver
    /// has to start recording.
    ///
    /// A caller re-runs its own summariser over the returned segments' turns
    /// and writes through [`Self::set_segment_summary`] on success only. The
    /// summariser stays the caller's precisely because it is the same one that
    /// produced every other summary in the tree; a second one here would put
    /// two differently-prompted summaries in one tier.
    ///
    /// Ordered oldest-first, which a caller must not treat as a work queue that
    /// drains on its own: a segment nothing can ever summarise stays at the
    /// head. Bounding the retries per segment is the caller's job.
    ///
    /// Defaulted to empty so a driver that has no notion of segment lifecycle
    /// is not forced to grow one. Empty means "none pending", which for such a
    /// driver is true.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn segments_pending_summary(
        &self,
        _limit: u32,
    ) -> Result<Vec<ConversationSegment>, MemoryError> {
        Ok(Vec::new())
    }
}

/// Moving the whole episodic record between drivers.
///
/// Reached through
/// [`MemoryProvider::as_episodic_portability`](super::MemoryProvider::as_episodic_portability).
/// See `tinymemory_bus::provider::episodic_portability` for why this is a
/// family of its own and how turn ids survive a copy.
#[async_trait]
pub trait MemoryEpisodicPortability: Send + Sync {
    /// One page of `part`, starting at `cursor` (`None` for the first page).
    ///
    /// Each live record of the part appears once in a walk, in an order the
    /// driver keeps stable from page to page. Turns carry their ids, segments
    /// their whole current state. A page holds at most `limit` records and may
    /// hold fewer — a driver bounds a page by size as well — so only a missing
    /// [`EpisodicExportPage::next_cursor`] ends the walk.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a zero `limit` or a cursor this driver did
    /// not issue, otherwise backend failures.
    async fn export_episodic(
        &self,
        part: EpisodicPart,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<EpisodicExportPage, MemoryError>;

    /// Write `records` as given, and report what happened to each.
    ///
    /// Idempotent, so a copy that stopped part-way can be run again:
    ///
    /// - a turn is stored under its own id; one the driver already holds
    ///   exactly as given is skipped, and one that meets a *different* turn
    ///   under its id is stored under a new id, reported in
    ///   [`EpisodicImportOutcome::remapped`]. A turn without an id is refused;
    /// - a segment replaces the segment with its id, whole;
    /// - an event replaces the event with its id;
    /// - an embedding replaces the one for its segment and model signature.
    ///
    /// A record the driver refuses counts as failed, with a reason that names
    /// it and never its content, and the rest of the batch goes on.
    ///
    /// # Errors
    ///
    /// Failures that make the whole batch meaningless: the backend is down,
    /// or refuses the credential.
    async fn import_episodic(
        &self,
        records: EpisodicRecords,
    ) -> Result<EpisodicImportOutcome, MemoryError>;
}
