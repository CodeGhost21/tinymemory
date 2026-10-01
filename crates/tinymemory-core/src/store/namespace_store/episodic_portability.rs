//! Paging the episodic tables out, and writing records back in, for a copy
//! of the episodic record between stores.
//!
//! Every listing walks one table in primary-key order from just past the
//! caller's last key, so a page is one indexed range scan and a write that
//! lands mid-walk never shifts a later page. Every write keeps what a record
//! already says when it says the same thing, so a copy that is run again
//! changes nothing.

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

use super::events::{row_to_event, EventRecord};
use super::fts5::{stored_text, EpisodicEntry};
use super::segments::{decode_embedding_row, row_to_segment, vec_to_bytes, ConversationSegment};

/// The columns [`row_to_segment`] reads, in its order.
const SEGMENT_COLUMNS: &str = "segment_id, session_id, namespace, start_episodic_id, \
     end_episodic_id, start_timestamp, end_timestamp, turn_count, summary, embedding, \
     topic_keywords, status, created_at, updated_at, start_seq, end_seq";

/// The columns [`row_to_event`] reads, in its order.
const EVENT_COLUMNS: &str = "event_id, segment_id, session_id, namespace, event_type, content, \
     subject, timestamp_ref, confidence, embedding, source_turn_ids, created_at";

/// SQLite's own ceiling on a `LIMIT`, reached by a caller asking for more
/// rows than a page can hold.
fn page_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// Turns with an id above `after`, lowest id first.
pub fn turns_after(
    conn: &Arc<Mutex<Connection>>,
    after: Option<i64>,
    limit: usize,
) -> anyhow::Result<Vec<EpisodicEntry>> {
    let conn = conn.lock();
    let mut stmt = conn.prepare(
        "SELECT id, session_id, timestamp, role, content, lesson, tool_calls_json, cost_microdollars
         FROM episodic_log
         WHERE id > ?1
         ORDER BY id ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(
            params![after.unwrap_or(i64::MIN), page_limit(limit)],
            |row| {
                Ok(EpisodicEntry {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    timestamp: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    lesson: row.get(5)?,
                    tool_calls_json: row.get(6)?,
                    cost_microdollars: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A turn's stored columns past its id: session, timestamp, role, content,
/// lesson, tool calls, cost.
type TurnRow = (
    String,
    f64,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

/// What happened to one imported turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnImport {
    /// Stored under the id it carried.
    Imported,
    /// The store already held it under that id, exactly.
    Skipped,
    /// The store already held it exactly, under this other id — a turn an
    /// earlier copy had to move.
    Present(i64),
    /// Another turn held its id, so it was stored under this one.
    Remapped(i64),
}

/// Writes `entry` under its own id when that id is free.
///
/// In order: a turn the store holds exactly under its id is skipped; one it
/// holds exactly under another id is reported there, so a copy that is run
/// again finds what an earlier run moved instead of writing it twice; a free
/// id is kept; and a turn that meets a different one under its id is stored
/// under a fresh id no lower than `fresh_floor`.
///
/// A caller passes the current time in microseconds as `fresh_floor`. Every
/// exported turn was recorded in the past, so its id is either a small row id
/// or a past microsecond, and an id taken from above the present cannot
/// collide with a turn still to come in the same copy — which an id taken from
/// just past the table's highest would, moving every later turn in turn.
///
/// The text is sanitized as [`super::fts5::episodic_insert`] sanitizes it, and
/// "holds exactly" compares the stored row with the sanitized text, so a turn
/// copied out of this store and back in is recognised as itself.
///
/// # Errors
///
/// An entry with no id, a secret-shaped session id or role, or a database
/// failure.
pub fn import_turn(
    conn: &Arc<Mutex<Connection>>,
    entry: &EpisodicEntry,
    fresh_floor: i64,
) -> anyhow::Result<TurnImport> {
    let Some(id) = entry.id else {
        anyhow::bail!("an imported turn needs the id it was exported with");
    };
    let text = stored_text(entry)?;
    let cost = i64::try_from(entry.cost_microdollars).unwrap_or(i64::MAX);
    let wanted: TurnRow = (
        entry.session_id.clone(),
        entry.timestamp,
        entry.role.clone(),
        text.content.clone(),
        text.lesson.clone(),
        text.tool_calls_json.clone(),
        cost,
    );
    let conn = conn.lock();
    let held: Option<TurnRow> = conn
        .query_row(
            "SELECT session_id, timestamp, role, content, lesson, tool_calls_json, cost_microdollars
             FROM episodic_log WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    if held.as_ref() == Some(&wanted) {
        return Ok(TurnImport::Skipped);
    }
    let elsewhere: Option<i64> = conn
        .query_row(
            "SELECT id FROM episodic_log
             WHERE session_id = ?1 AND timestamp = ?2 AND role = ?3 AND content = ?4
               AND lesson IS ?5 AND tool_calls_json IS ?6 AND cost_microdollars = ?7
             ORDER BY id ASC LIMIT 1",
            params![wanted.0, wanted.1, wanted.2, wanted.3, wanted.4, wanted.5, wanted.6],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(other) = elsewhere {
        return Ok(TurnImport::Present(other));
    }
    let target = if held.is_none() {
        id
    } else {
        let highest: Option<i64> =
            conn.query_row("SELECT MAX(id) FROM episodic_log", [], |row| row.get(0))?;
        fresh_floor.max(highest.unwrap_or(0).saturating_add(1))
    };
    conn.execute(
        "INSERT INTO episodic_log
         (id, session_id, timestamp, role, content, lesson, tool_calls_json, cost_microdollars)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![target, wanted.0, wanted.1, wanted.2, wanted.3, wanted.4, wanted.5, wanted.6],
    )?;
    Ok(if target == id {
        TurnImport::Imported
    } else {
        TurnImport::Remapped(target)
    })
}

/// Segments with an id after `after`, by id.
pub fn segments_after(
    conn: &Arc<Mutex<Connection>>,
    after: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<ConversationSegment>> {
    let conn = conn.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {SEGMENT_COLUMNS} FROM conversation_segments
         WHERE ?1 IS NULL OR segment_id > ?1
         ORDER BY segment_id ASC
         LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![after, page_limit(limit)], row_to_segment)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Writes `segment` whole, replacing the segment with its id. Answers whether
/// anything changed: `false` when the store already held it exactly.
///
/// `topic_keywords` is not carried by a copy, so a replaced row keeps its own
/// and a new one starts without.
pub fn import_segment(
    conn: &Arc<Mutex<Connection>>,
    segment: &ConversationSegment,
) -> anyhow::Result<bool> {
    let conn = conn.lock();
    let held = conn
        .query_row(
            &format!("SELECT {SEGMENT_COLUMNS} FROM conversation_segments WHERE segment_id = ?1"),
            params![segment.segment_id],
            row_to_segment,
        )
        .optional()?;
    if let Some(held) = &held {
        if same_segment(held, segment) {
            return Ok(false);
        }
    }
    let embedding = segment.embedding.as_deref().map(vec_to_bytes);
    conn.execute(
        "INSERT INTO conversation_segments
         (segment_id, session_id, namespace, start_episodic_id, end_episodic_id,
          start_timestamp, end_timestamp, turn_count, summary, embedding,
          status, created_at, updated_at, start_seq, end_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(segment_id) DO UPDATE SET
            session_id = excluded.session_id,
            namespace = excluded.namespace,
            start_episodic_id = excluded.start_episodic_id,
            end_episodic_id = excluded.end_episodic_id,
            start_timestamp = excluded.start_timestamp,
            end_timestamp = excluded.end_timestamp,
            turn_count = excluded.turn_count,
            summary = excluded.summary,
            embedding = excluded.embedding,
            status = excluded.status,
            updated_at = excluded.updated_at,
            start_seq = excluded.start_seq,
            end_seq = excluded.end_seq",
        params![
            segment.segment_id,
            segment.session_id,
            segment.namespace,
            segment.start_episodic_id,
            segment.end_episodic_id,
            segment.start_timestamp,
            segment.end_timestamp,
            segment.turn_count,
            segment.summary,
            embedding,
            segment.status.as_str(),
            segment.created_at,
            segment.updated_at,
            segment.start_seq,
            segment.end_seq,
        ],
    )?;
    Ok(true)
}

/// Whether two segments say the same thing about everything a copy carries.
fn same_segment(a: &ConversationSegment, b: &ConversationSegment) -> bool {
    a.session_id == b.session_id
        && a.namespace == b.namespace
        && a.start_episodic_id == b.start_episodic_id
        && a.end_episodic_id == b.end_episodic_id
        && a.start_timestamp == b.start_timestamp
        && a.end_timestamp == b.end_timestamp
        && a.turn_count == b.turn_count
        && a.summary == b.summary
        && a.embedding == b.embedding
        && a.status == b.status
        && a.start_seq == b.start_seq
        && a.end_seq == b.end_seq
}

/// Events with an id after `after`, by id.
pub fn events_after(
    conn: &Arc<Mutex<Connection>>,
    after: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<EventRecord>> {
    let conn = conn.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM event_log
         WHERE ?1 IS NULL OR event_id > ?1
         ORDER BY event_id ASC
         LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![after, page_limit(limit)], row_to_event)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Writes `event`, replacing the event with its id. Answers whether anything
/// changed: `false` when the store already held it exactly.
pub fn import_event(conn: &Arc<Mutex<Connection>>, event: &EventRecord) -> anyhow::Result<bool> {
    let held = {
        let conn = conn.lock();
        conn.query_row(
            &format!("SELECT {EVENT_COLUMNS} FROM event_log WHERE event_id = ?1"),
            params![event.event_id],
            row_to_event,
        )
        .optional()?
    };
    if let Some(held) = &held {
        if same_event(held, event) {
            return Ok(false);
        }
    }
    super::events::event_insert(conn, event)?;
    Ok(true)
}

/// Whether two events say the same thing.
fn same_event(a: &EventRecord, b: &EventRecord) -> bool {
    a.segment_id == b.segment_id
        && a.session_id == b.session_id
        && a.namespace == b.namespace
        && a.event_type == b.event_type
        && a.content == b.content
        && a.subject == b.subject
        && a.timestamp_ref == b.timestamp_ref
        && a.confidence == b.confidence
        && a.embedding == b.embedding
        && a.source_turn_ids == b.source_turn_ids
        && a.created_at == b.created_at
}

/// One stored segment embedding.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSegmentEmbedding {
    /// The segment it embeds.
    pub segment_id: String,
    /// The embedding space it was computed in.
    pub model_signature: String,
    /// The vector.
    pub vector: Vec<f32>,
    /// When it was computed, seconds since the epoch.
    pub created_at: f64,
}

/// Segment embeddings after `(segment_id, model_signature)`, in key order.
///
/// # Errors
///
/// A database failure, or a stored vector whose length disagrees with its
/// recorded dimension.
pub fn segment_embeddings_after(
    conn: &Arc<Mutex<Connection>>,
    after: Option<(&str, &str)>,
    limit: usize,
) -> anyhow::Result<Vec<StoredSegmentEmbedding>> {
    let (after_segment, after_signature) = after.unzip();
    let conn = conn.lock();
    let mut stmt = conn.prepare(
        "SELECT segment_id, model_signature, vector, dim, created_at
         FROM segment_embeddings
         WHERE ?1 IS NULL OR (segment_id, model_signature) > (?1, ?2)
         ORDER BY segment_id ASC, model_signature ASC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(
            params![after_segment, after_signature, page_limit(limit)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f64>(4)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(segment_id, model_signature, bytes, dim, created_at)| {
            Ok(StoredSegmentEmbedding {
                vector: decode_embedding_row(&bytes, dim)?.unwrap_or_default(),
                segment_id,
                model_signature,
                created_at,
            })
        })
        .collect()
}

/// Writes `embedding`, replacing the one for its segment and signature.
/// Answers whether anything changed.
pub fn import_segment_embedding(
    conn: &Arc<Mutex<Connection>>,
    embedding: &StoredSegmentEmbedding,
) -> anyhow::Result<bool> {
    let held = {
        let conn = conn.lock();
        conn.query_row(
            "SELECT vector, dim, created_at FROM segment_embeddings
             WHERE segment_id = ?1 AND model_signature = ?2",
            params![embedding.segment_id, embedding.model_signature],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            },
        )
        .optional()?
    };
    if let Some((bytes, dim, created_at)) = held {
        if created_at == embedding.created_at
            && decode_embedding_row(&bytes, dim)?.as_deref() == Some(embedding.vector.as_slice())
        {
            return Ok(false);
        }
    }
    super::segments::segment_embedding_upsert(
        conn,
        &embedding.segment_id,
        &embedding.model_signature,
        &embedding.vector,
        embedding.created_at,
    )?;
    Ok(true)
}

#[cfg(test)]
#[path = "episodic_portability_tests.rs"]
mod tests;
