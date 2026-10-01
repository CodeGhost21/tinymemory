//! Episodic memory: every turn of every session, grouped into segments the
//! host opens and closes as a conversation moves on.
//!
//! Turns, segments, extracted events and segment embeddings are inert records
//! in bookkeeping scopes of their own (see [`super::scopes`]), as JSON the
//! engine does not learn from — what a conversation says reaches the engine through
//! ingestion, once, not again through its bookkeeping. Turns and segments carry
//! their session's lookup label, so the calls made on every turn — recording
//! it, finding the open segment, extending it — read one session's records, or
//! one segment's, never the whole history.
//!
//! # Turn ids
//!
//! The contract asks for an `i64` the driver assigns, and the engine's ids are
//! strings. A turn's id is the microsecond it was recorded at, bumped past the
//! last one this process handed out, so ids rise within a process and do not
//! collide across devices short of two writes in the same microsecond.
//!
//! # Summaries
//!
//! A segment is summarised by the host's recap, which needs the embedded
//! engine's tree. Hosted memory has none, so nothing asks it to summarise and
//! [`MemoryEpisodic::segments_pending_summary`] keeps its default: none
//! pending, rather than a growing list of segments no retry can summarise.

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::{
    ConversationSegment, EpisodicEvent, EpisodicTurn, MemoryEpisodic, SegmentStatus,
};

use super::records::{Place, Record, Records, Version};
use super::scopes::{EPISODIC_EVENTS, SEGMENTS, SEGMENT_EMBEDDINGS, TURNS};
use crate::cortex_provider::CortexProvider;

/// The last turn id this process handed out.
static LAST_TURN_ID: AtomicI64 = AtomicI64::new(0);

/// Makes the next turn id at least one past `id`, so a test can know what
/// [`next_turn_id`] hands out next.
#[cfg(test)]
pub(super) fn turn_ids_continue_after(id: i64) {
    LAST_TURN_ID.fetch_max(id, Ordering::SeqCst);
}

/// A new turn id: the current microsecond, or one past the last id if that is
/// not already later.
pub(super) fn next_turn_id() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX)
        });
    let mut last = LAST_TURN_ID.load(Ordering::SeqCst);
    loop {
        let next = now.max(last.saturating_add(1));
        match LAST_TURN_ID.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(current) => last = current,
        }
    }
}

/// A segment as stored: the contract's shape, and when it was created, which
/// orders a session's segments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StoredSegment {
    #[serde(flatten)]
    pub(super) segment: ConversationSegment,
    pub(super) created_at: f64,
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(version: &Version) -> Option<T> {
    serde_json::from_str(&version.record.content).ok()
}

/// An inert record of `session`, under `key`.
pub(super) fn session_record(key: impl Into<String>, content: String, session: &str) -> Record {
    Record {
        session_id: Some(session.to_string()),
        ..Record::plain(key, content)
    }
}

impl CortexProvider {
    pub(super) fn episodic_place(&self, scope: &str) -> Result<Place, MemoryError> {
        Place::bookkeeping(&self.dialect, scope.to_string()).map_err(engine_error)
    }

    async fn segment(&self, segment_id: &str) -> Result<Option<StoredSegment>, MemoryError> {
        let place = self.episodic_place(SEGMENTS)?;
        Ok(Records::new(&self.dialect)
            .live(&place, segment_id)
            .await
            .map_err(engine_error)?
            .and_then(|version| parse(&version)))
    }

    async fn write_segment(&self, stored: &StoredSegment) -> Result<(), MemoryError> {
        let place = self.episodic_place(SEGMENTS)?;
        let record = session_record(
            stored.segment.segment_id.clone(),
            serde_json::to_string(stored)?,
            &stored.segment.session_id,
        );
        Records::new(&self.dialect)
            .put(&place, &record, None)
            .await
            .map_err(engine_error)
    }

    /// Reads a segment, changes it and writes it back. A segment that does
    /// not exist is left alone, as the embedded engine's update leaves it.
    async fn update_segment(
        &self,
        segment_id: &str,
        change: impl FnOnce(&mut ConversationSegment) -> bool + Send,
    ) -> Result<(), MemoryError> {
        let Some(mut stored) = self.segment(segment_id).await? else {
            return Ok(());
        };
        if change(&mut stored.segment) {
            self.write_segment(&stored).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl MemoryEpisodic for CortexProvider {
    async fn insert_turn(&self, turn: &EpisodicTurn) -> Result<i64, MemoryError> {
        if turn.session_id.trim().is_empty() || turn.role.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "a turn needs a session id and a role".to_string(),
            ));
        }
        let id = next_turn_id();
        let stored = EpisodicTurn {
            id: Some(id),
            cost_microdollars: turn.cost_microdollars.max(0),
            ..turn.clone()
        };
        let place = self.episodic_place(TURNS)?;
        let record = session_record(
            format!("turn:{id}"),
            serde_json::to_string(&stored)?,
            &turn.session_id,
        );
        Records::new(&self.dialect)
            .insert(&place, &record)
            .await
            .map_err(engine_error)?;
        Ok(id)
    }

    async fn session_turns(&self, session_id: &str) -> Result<Vec<EpisodicTurn>, MemoryError> {
        let place = self.episodic_place(TURNS)?;
        let mut turns: Vec<EpisodicTurn> = Records::new(&self.dialect)
            .of_session(&place, session_id)
            .await
            .map_err(engine_error)?
            .iter()
            .filter_map(parse::<EpisodicTurn>)
            .filter(|turn| turn.session_id == session_id)
            .collect();
        turns.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp).then(a.id.cmp(&b.id)));
        Ok(turns)
    }

    async fn open_segment(
        &self,
        session_id: &str,
    ) -> Result<Option<ConversationSegment>, MemoryError> {
        let place = self.episodic_place(SEGMENTS)?;
        Ok(Records::new(&self.dialect)
            .of_session(&place, session_id)
            .await
            .map_err(engine_error)?
            .iter()
            .filter_map(|version| {
                parse::<StoredSegment>(version).map(|stored| (version.order, stored))
            })
            .filter(|(_, stored)| {
                stored.segment.session_id == session_id
                    && stored.segment.status == Some(SegmentStatus::Open)
            })
            .max_by(|(a_order, a), (b_order, b)| {
                a.created_at
                    .total_cmp(&b.created_at)
                    .then(a_order.cmp(b_order))
            })
            .map(|(_, stored)| stored.segment))
    }

    async fn create_segment(
        &self,
        segment_id: &str,
        session_id: &str,
        namespace: &str,
        start_episodic_id: i64,
        start_seq: Option<u32>,
        start_timestamp: f64,
        now_seconds: f64,
    ) -> Result<(), MemoryError> {
        if segment_id.trim().is_empty() || session_id.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "a segment needs an id and a session id".to_string(),
            ));
        }
        self.write_segment(&StoredSegment {
            segment: ConversationSegment {
                segment_id: segment_id.to_string(),
                session_id: session_id.to_string(),
                namespace: namespace.to_string(),
                start_episodic_id,
                end_episodic_id: None,
                start_timestamp,
                end_timestamp: None,
                turn_count: 1,
                summary: None,
                embedding: None,
                open: true,
                status: Some(SegmentStatus::Open),
                start_seq,
                end_seq: None,
            },
            created_at: now_seconds,
        })
        .await
    }

    async fn append_turn(
        &self,
        segment_id: &str,
        episodic_id: i64,
        seq: Option<u32>,
        timestamp: f64,
        _now: f64,
    ) -> Result<(), MemoryError> {
        self.update_segment(segment_id, |segment| {
            segment.turn_count = segment.turn_count.saturating_add(1);
            segment.end_episodic_id = Some(episodic_id);
            segment.end_seq = seq;
            segment.end_timestamp = Some(timestamp);
            true
        })
        .await
    }

    async fn close_segment(&self, segment_id: &str, _now: f64) -> Result<(), MemoryError> {
        self.update_segment(segment_id, |segment| {
            if segment.status != Some(SegmentStatus::Open) {
                return false;
            }
            segment.status = Some(SegmentStatus::Closed);
            segment.open = false;
            true
        })
        .await
    }

    async fn set_segment_summary(
        &self,
        segment_id: &str,
        summary: &str,
        _now: f64,
    ) -> Result<(), MemoryError> {
        self.update_segment(segment_id, |segment| {
            segment.summary = Some(summary.to_string());
            segment.status = Some(SegmentStatus::Summarised);
            segment.open = false;
            true
        })
        .await
    }

    async fn insert_event(&self, event: &EpisodicEvent) -> Result<(), MemoryError> {
        if event.event_id.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "an episodic event needs an id".to_string(),
            ));
        }
        let place = self.episodic_place(EPISODIC_EVENTS)?;
        let record = session_record(
            event.event_id.clone(),
            serde_json::to_string(event)?,
            &event.session_id,
        );
        Records::new(&self.dialect)
            .put(&place, &record, None)
            .await
            .map_err(engine_error)
    }

    async fn upsert_segment_embedding(
        &self,
        segment_id: &str,
        model_signature: &str,
        embedding: &[f32],
        created_at: f64,
    ) -> Result<(), MemoryError> {
        let key = format!("{segment_id}/{model_signature}");
        let body = json!({
            "segment_id": segment_id,
            "model_signature": model_signature,
            "embedding": embedding,
            "created_at": created_at,
        });
        let place = self.episodic_place(SEGMENT_EMBEDDINGS)?;
        Records::new(&self.dialect)
            .put(&place, &Record::plain(key, body.to_string()), None)
            .await
            .map_err(engine_error)
    }
}

#[cfg(test)]
#[path = "episodic_test.rs"]
mod test;
