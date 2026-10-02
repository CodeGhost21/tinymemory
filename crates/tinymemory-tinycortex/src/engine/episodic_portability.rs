//! The episodic record, paged out of the engine's tables and written back in.
//!
//! Each part is read in primary-key order from just past the cursor, so a
//! page is one range scan. A page also stops at [`PAGE_BYTES`] of payload,
//! whatever `limit` says: it crosses the module bus as one frame, and a page
//! of long assistant turns would otherwise outgrow it.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{
    ConversationSegment, EpisodicEvent, EpisodicExportPage, EpisodicImportOutcome, EpisodicPart,
    EpisodicRecords, EpisodicTurn, EventKind, MemoryEpisodicPortability, SegmentEmbedding,
    SegmentStatus, TurnIdRemap,
};
use tinymemory_core::store::episodic_portability as store;
use tinymemory_core::store::events::{EventRecord, EventType};
use tinymemory_core::store::fts5::EpisodicEntry;

use super::{episodic_to_contract, event_kind_to_engine, segment_to_contract, TinycortexProvider};

/// Most payload one page carries, in bytes.
const PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Most refusal reasons one import outcome keeps.
const MAX_ERRORS: usize = 20;

/// Where the next page of `part` starts, as an opaque cursor.
fn cursor(part: EpisodicPart, key: &str) -> String {
    format!("{}:{key}", part.as_str())
}

/// The key a cursor this driver issued for `part` names.
fn key_of(part: EpisodicPart, cursor: Option<&str>) -> Result<Option<String>, MemoryError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    cursor
        .strip_prefix(part.as_str())
        .and_then(|rest| rest.strip_prefix(':'))
        .map(|key| Some(key.to_string()))
        .ok_or_else(|| invalid_cursor(part))
}

fn invalid_cursor(part: EpisodicPart) -> MemoryError {
    MemoryError::Invalid(format!("not a cursor this driver issued for {part}"))
}

/// The leading records of `records` that fit in [`PAGE_BYTES`] — always at
/// least one, so a single oversized record still moves — and whether any were
/// left out.
fn fit<T>(mut records: Vec<T>, size: impl Fn(&T) -> usize) -> (Vec<T>, bool) {
    let mut total = 0usize;
    let mut keep = 0usize;
    for record in &records {
        total = total.saturating_add(size(record));
        if keep > 0 && total > PAGE_BYTES {
            break;
        }
        keep += 1;
    }
    let trimmed = keep < records.len();
    records.truncate(keep);
    (records, trimmed)
}

fn optional_len(text: Option<&String>) -> usize {
    text.map_or(0, String::len)
}

fn turn_size(turn: &EpisodicEntry) -> usize {
    turn.content.len()
        + optional_len(turn.lesson.as_ref())
        + optional_len(turn.tool_calls_json.as_ref())
        + turn.session_id.len()
}

fn segment_size(segment: &tinymemory_core::store::segments::ConversationSegment) -> usize {
    optional_len(segment.summary.as_ref())
        + segment.embedding.as_ref().map_or(0, |v| v.len() * 16)
        + segment.segment_id.len()
        + segment.session_id.len()
}

fn event_size(event: &EventRecord) -> usize {
    event.content.len()
        + event.embedding.as_ref().map_or(0, |v| v.len() * 16)
        + optional_len(event.source_turn_ids.as_ref())
}

/// Engine event type -> contract event kind.
fn event_kind_from_engine(kind: &EventType) -> EventKind {
    match kind {
        EventType::Fact => EventKind::Fact,
        EventType::Decision => EventKind::Decision,
        EventType::Commitment => EventKind::Commitment,
        EventType::Preference => EventKind::Preference,
        EventType::Question => EventKind::Question,
        EventType::Foresight => EventKind::Foresight,
    }
}

fn event_to_contract(event: EventRecord) -> EpisodicEvent {
    EpisodicEvent {
        kind: event_kind_from_engine(&event.event_type),
        event_id: event.event_id,
        segment_id: event.segment_id,
        session_id: event.session_id,
        namespace: event.namespace,
        content: event.content,
        subject: event.subject,
        timestamp_ref: event.timestamp_ref,
        confidence: event.confidence,
        embedding: event.embedding,
        source_turn_ids: event.source_turn_ids,
        created_at: event.created_at,
    }
}

fn event_to_engine(event: EpisodicEvent) -> EventRecord {
    EventRecord {
        event_type: event_kind_to_engine(event.kind),
        event_id: event.event_id,
        segment_id: event.segment_id,
        session_id: event.session_id,
        namespace: event.namespace,
        content: event.content,
        subject: event.subject,
        timestamp_ref: event.timestamp_ref,
        confidence: event.confidence,
        embedding: event.embedding,
        source_turn_ids: event.source_turn_ids,
        created_at: event.created_at,
    }
}

fn turn_to_engine(turn: EpisodicTurn) -> EpisodicEntry {
    EpisodicEntry {
        id: turn.id,
        session_id: turn.session_id,
        timestamp: turn.timestamp,
        role: turn.role,
        content: turn.content,
        lesson: turn.lesson,
        tool_calls_json: turn.tool_calls_json,
        cost_microdollars: u64::try_from(turn.cost_microdollars).unwrap_or(0),
    }
}

/// Contract segment -> engine row. The contract carries no creation time, so
/// a segment is taken to have been created when it started and updated when
/// it last grew. A segment from a driver that predates `status` is read from
/// what it does say: open, summarised when it has a summary, else closed.
fn segment_to_engine(
    segment: ConversationSegment,
) -> tinymemory_core::store::segments::ConversationSegment {
    use tinymemory_core::store::segments::SegmentStatus as Engine;
    let status = match segment.status {
        Some(SegmentStatus::Open) => Engine::Open,
        Some(SegmentStatus::Closed) => Engine::Closed,
        Some(SegmentStatus::Summarised) => Engine::Summarised,
        None if segment.open => Engine::Open,
        None if segment.summary.is_some() => Engine::Summarised,
        None => Engine::Closed,
    };
    tinymemory_core::store::segments::ConversationSegment {
        created_at: segment.start_timestamp,
        updated_at: segment.end_timestamp.unwrap_or(segment.start_timestamp),
        segment_id: segment.segment_id,
        session_id: segment.session_id,
        namespace: segment.namespace,
        start_episodic_id: segment.start_episodic_id,
        end_episodic_id: segment.end_episodic_id,
        start_timestamp: segment.start_timestamp,
        end_timestamp: segment.end_timestamp,
        turn_count: segment.turn_count,
        summary: segment.summary,
        embedding: segment.embedding,
        topic_keywords: None,
        status,
        start_seq: segment.start_seq,
        end_seq: segment.end_seq,
    }
}

/// The lowest id a moved turn may take: the present, in microseconds. See
/// [`store::import_turn`] for why from above the present.
fn fresh_floor() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX)
        })
}

/// Counts what one record's write did into `outcome`.
fn tally(outcome: &mut EpisodicImportOutcome, written: anyhow::Result<bool>, record: &str) {
    match written {
        Ok(true) => outcome.imported += 1,
        Ok(false) => outcome.skipped += 1,
        Err(error) => refuse(outcome, format!("{record}: {error}")),
    }
}

fn refuse(outcome: &mut EpisodicImportOutcome, reason: String) {
    outcome.failed += 1;
    if outcome.errors.len() < MAX_ERRORS {
        outcome.errors.push(reason);
    }
}

#[async_trait]
impl MemoryEpisodicPortability for TinycortexProvider {
    async fn export_episodic(
        &self,
        part: EpisodicPart,
        cursor_in: Option<&str>,
        limit: usize,
    ) -> Result<EpisodicExportPage, MemoryError> {
        if limit == 0 {
            return Err(MemoryError::Invalid(
                "episodic export page limit must be greater than zero".to_string(),
            ));
        }
        let after = key_of(part, cursor_in)?;
        let conn = self.client.profile_conn();
        let page =
            tokio::task::spawn_blocking(move || -> Result<EpisodicExportPage, MemoryError> {
                let read = |error: anyhow::Error| Self::other("export episodic", error);
                match part {
                    EpisodicPart::Turns => {
                        let after = after
                            .map(|key| key.parse::<i64>().map_err(|_| invalid_cursor(part)))
                            .transpose()?;
                        let rows = store::turns_after(&conn, after, limit).map_err(read)?;
                        let full = rows.len() == limit;
                        let (rows, trimmed) = fit(rows, turn_size);
                        let next = (full || trimmed)
                            .then(|| rows.last().and_then(|row| row.id))
                            .flatten()
                            .map(|id| cursor(part, &id.to_string()));
                        Ok(EpisodicExportPage {
                            records: EpisodicRecords::Turns(
                                rows.into_iter().map(episodic_to_contract).collect(),
                            ),
                            next_cursor: next,
                        })
                    }
                    EpisodicPart::Segments => {
                        let rows =
                            store::segments_after(&conn, after.as_deref(), limit).map_err(read)?;
                        let full = rows.len() == limit;
                        let (rows, trimmed) = fit(rows, segment_size);
                        let next = (full || trimmed)
                            .then(|| rows.last().map(|row| cursor(part, &row.segment_id)))
                            .flatten();
                        Ok(EpisodicExportPage {
                            records: EpisodicRecords::Segments(
                                rows.into_iter().map(segment_to_contract).collect(),
                            ),
                            next_cursor: next,
                        })
                    }
                    EpisodicPart::Events => {
                        let rows =
                            store::events_after(&conn, after.as_deref(), limit).map_err(read)?;
                        let full = rows.len() == limit;
                        let (rows, trimmed) = fit(rows, event_size);
                        let next = (full || trimmed)
                            .then(|| rows.last().map(|row| cursor(part, &row.event_id)))
                            .flatten();
                        Ok(EpisodicExportPage {
                            records: EpisodicRecords::Events(
                                rows.into_iter().map(event_to_contract).collect(),
                            ),
                            next_cursor: next,
                        })
                    }
                    EpisodicPart::SegmentEmbeddings => {
                        let after: Option<(String, String)> = after
                            .map(|key| serde_json::from_str(&key).map_err(|_| invalid_cursor(part)))
                            .transpose()?;
                        let rows = store::segment_embeddings_after(
                            &conn,
                            after.as_ref().map(|(s, m)| (s.as_str(), m.as_str())),
                            limit,
                        )
                        .map_err(read)?;
                        let full = rows.len() == limit;
                        let (rows, trimmed) = fit(rows, |row| row.vector.len() * 16);
                        let next = match rows.last() {
                            Some(row) if full || trimmed => Some(cursor(
                                part,
                                &serde_json::to_string(&(&row.segment_id, &row.model_signature))?,
                            )),
                            _ => None,
                        };
                        Ok(EpisodicExportPage {
                            records: EpisodicRecords::SegmentEmbeddings(
                                rows.into_iter()
                                    .map(|row| SegmentEmbedding {
                                        segment_id: row.segment_id,
                                        model_signature: row.model_signature,
                                        embedding: row.vector,
                                        created_at: row.created_at,
                                    })
                                    .collect(),
                            ),
                            next_cursor: next,
                        })
                    }
                }
            })
            .await
            .map_err(|error| Self::other("join export episodic", error))??;
        log::debug!(
            "[tinycortex] episodic export page part={part} records={} more={}",
            page.records.len(),
            page.next_cursor.is_some()
        );
        Ok(page)
    }

    async fn import_episodic(
        &self,
        records: EpisodicRecords,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let part = records.part();
        let conn = self.client.profile_conn();
        let outcome = tokio::task::spawn_blocking(move || {
            let mut outcome = EpisodicImportOutcome::default();
            match records {
                EpisodicRecords::Turns(turns) => {
                    let floor = fresh_floor();
                    for turn in turns {
                        let from = turn.id;
                        match store::import_turn(&conn, &turn_to_engine(turn), floor) {
                            Ok(store::TurnImport::Imported) => outcome.imported += 1,
                            Ok(store::TurnImport::Skipped) => outcome.skipped += 1,
                            Ok(store::TurnImport::Present(at)) => {
                                outcome.skipped += 1;
                                if let Some(from) = from {
                                    outcome.remapped.push(TurnIdRemap { from, to: at });
                                }
                            }
                            Ok(store::TurnImport::Remapped(to)) => {
                                outcome.imported += 1;
                                if let Some(from) = from {
                                    outcome.remapped.push(TurnIdRemap { from, to });
                                }
                            }
                            Err(error) => refuse(
                                &mut outcome,
                                format!("turn {}: {error}", from.unwrap_or_default()),
                            ),
                        }
                    }
                }
                EpisodicRecords::Segments(segments) => {
                    for segment in segments {
                        let id = segment.segment_id.clone();
                        let written = store::import_segment(&conn, &segment_to_engine(segment));
                        tally(&mut outcome, written, &format!("segment {id}"));
                    }
                }
                EpisodicRecords::Events(events) => {
                    for event in events {
                        let id = event.event_id.clone();
                        let written = store::import_event(&conn, &event_to_engine(event));
                        tally(&mut outcome, written, &format!("event {id}"));
                    }
                }
                EpisodicRecords::SegmentEmbeddings(embeddings) => {
                    for embedding in embeddings {
                        let record = format!(
                            "embedding {}/{}",
                            embedding.segment_id, embedding.model_signature
                        );
                        let written = store::import_segment_embedding(
                            &conn,
                            &store::StoredSegmentEmbedding {
                                segment_id: embedding.segment_id,
                                model_signature: embedding.model_signature,
                                vector: embedding.embedding,
                                created_at: embedding.created_at,
                            },
                        );
                        tally(&mut outcome, written, &record);
                    }
                }
            }
            outcome
        })
        .await
        .map_err(|error| Self::other("join import episodic", error))?;
        log::debug!(
            "[tinycortex] episodic import part={part} imported={} skipped={} failed={} remapped={}",
            outcome.imported,
            outcome.skipped,
            outcome.failed,
            outcome.remapped.len()
        );
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "episodic_portability_tests.rs"]
mod test;
