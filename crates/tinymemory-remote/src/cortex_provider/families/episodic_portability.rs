//! The episodic record, paged out of its bookkeeping scopes and written back
//! in, for a copy between drivers.
//!
//! # Export
//!
//! Each part lives in one bookkeeping scope (see [`super::scopes`]). A page
//! folds that scope to its live records, orders them by key — turns by id —
//! and starts just past the cursor's key. Folding reads the whole scope, so a
//! page costs what the scope costs to list; a copy asks for large pages. A
//! page also stops at [`PAGE_BYTES`] of payload.
//!
//! # Import
//!
//! The backend has no bulk route, so an import appends one record per item
//! without waiting, then waits once for the last — the log is ordered, so
//! that one becoming listable implies the rest are. Before writing, it looks
//! up what the scope already holds under the batch's keys, forty keys a
//! request, and skips a record the scope holds exactly; the versions a record
//! replaces are retired after the wait, as [`Records::put`] retires them. A
//! backend that says it cannot serve right now gets the same pauses a record
//! import gets, and then fails the batch.
//!
//! A turn keeps its id unless a different turn holds it. Then the turn's
//! session is searched for an identical turn an earlier copy moved, and only
//! if there is none does it take a fresh id, from [`next_turn_id`] — above
//! every turn already recorded, and never one a turn still to come in the same
//! batch carries. Later batches look up what earlier ones wrote, which is
//! readable by then: each batch waits for its last write.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::{
    ConversationSegment, EpisodicEvent, EpisodicExportPage, EpisodicImportOutcome, EpisodicPart,
    EpisodicRecords, EpisodicTurn, MemoryEpisodicPortability, SegmentEmbedding, TurnIdRemap,
};

use super::episodic::{next_turn_id, parse, session_record, StoredSegment};
use super::records::{newest_live, Place, Record, Records, Version, SUPERSEDED};
use super::scopes::{EPISODIC_EVENTS, SEGMENTS, SEGMENT_EMBEDDINGS, TURNS};
use crate::cortex::AppendedEvent;
use crate::cortex_provider::CortexProvider;
use crate::hosted::error_code;

/// Most payload one export page carries, in bytes.
const PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Most refusal reasons one import outcome keeps.
const MAX_ERRORS: usize = 20;

/// A segment embedding as its bookkeeping record holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredEmbedding {
    segment_id: String,
    model_signature: String,
    embedding: Vec<f32>,
    created_at: f64,
}

fn embedding_key(segment_id: &str, model_signature: &str) -> String {
    format!("{segment_id}/{model_signature}")
}

fn turn_key(id: i64) -> String {
    format!("turn:{id}")
}

fn invalid_cursor(part: EpisodicPart) -> MemoryError {
    MemoryError::Invalid(format!("not a cursor this driver issued for {part}"))
}

/// The key a cursor this driver issued for `part` names.
fn key_of(part: EpisodicPart, cursor: Option<&str>) -> Result<Option<&str>, MemoryError> {
    cursor
        .map(|cursor| {
            cursor
                .strip_prefix(part.as_str())
                .and_then(|rest| rest.strip_prefix(':'))
                .ok_or_else(|| invalid_cursor(part))
        })
        .transpose()
}

/// The records of one page: those after `after` in `ordered`, at most
/// `limit` of them and at most [`PAGE_BYTES`] of payload, always at least one
/// when there is one; and the key to resume from, when anything is left.
fn page<T>(
    ordered: Vec<(String, T)>,
    limit: usize,
    size: impl Fn(&T) -> usize,
) -> (Vec<T>, Option<String>) {
    let mut taken = Vec::new();
    let mut total = 0usize;
    let mut last_key = None;
    let mut rest = ordered.into_iter().peekable();
    while let Some((key, record)) = rest.next_if(|_| taken.len() < limit) {
        total = total.saturating_add(size(&record));
        if !taken.is_empty() && total > PAGE_BYTES {
            // Put nothing back: the cursor below resumes at this record.
            return (taken, last_key);
        }
        taken.push(record);
        last_key = Some(key);
    }
    let next = rest.peek().is_some().then_some(last_key).flatten();
    (taken, next)
}

fn refuse(outcome: &mut EpisodicImportOutcome, reason: String) {
    outcome.failed += 1;
    if outcome.errors.len() < MAX_ERRORS {
        outcome.errors.push(reason);
    }
}

/// Why one record was not written.
enum Fault {
    /// The backend refused this record; the rest of the batch can go on.
    Refused(String),
    /// A failure that makes the whole batch meaningless.
    Batch(MemoryError),
}

/// One keyed record an import writes.
struct Write {
    /// What a refusal names: the part and the key, never the content.
    label: String,
    record: Record,
}

impl CortexProvider {
    /// Appends one record, pausing between attempts while the backend says it
    /// cannot serve right now — the patience a record import has.
    async fn append_record_patiently(
        &self,
        place: &Place,
        record: &Record,
    ) -> Result<Option<AppendedEvent>, Fault> {
        let records = Records::new(&self.dialect);
        let mut pauses = self.import_patience.iter();
        loop {
            let error = match records.append(place, record, None, false).await {
                Ok(appended) => return Ok(appended),
                Err(error) => engine_error(error),
            };
            match &error {
                MemoryError::Invalid(_) | MemoryError::NotFound(_) => {
                    let code = error_code(&error).unwrap_or("refused");
                    return Err(Fault::Refused(format!(
                        "the memory backend refused it ({code})"
                    )));
                }
                MemoryError::Unavailable(_)
                | MemoryError::Timeout(_)
                | MemoryError::Unreachable(_) => match pauses.next() {
                    Some(pause) => tokio::time::sleep(*pause).await,
                    None => return Err(Fault::Batch(error)),
                },
                _ => return Err(Fault::Batch(error)),
            }
        }
    }

    /// The live records of `scope`.
    async fn live_records(&self, scope: &str) -> Result<Vec<Version>, MemoryError> {
        let place = self.episodic_place(scope)?;
        Records::new(&self.dialect)
            .live_all(&place)
            .await
            .map_err(engine_error)
    }

    /// Writes `writes` to `place`, skipping each one `same` says the scope
    /// already holds, then waits once and retires what was replaced.
    async fn write_keyed(
        &self,
        place: &Place,
        writes: Vec<Write>,
        same: impl Fn(&Version, &Record) -> bool,
        outcome: &mut EpisodicImportOutcome,
    ) -> Result<(), MemoryError> {
        let records = Records::new(&self.dialect);
        let keys: Vec<&str> = writes.iter().map(|w| w.record.key.as_str()).collect();
        let held = records.versions(place, &keys).await.map_err(engine_error)?;
        let mut last = None;
        let mut replaced = Vec::new();
        for write in writes {
            let versions = held.get(&write.record.key).cloned().unwrap_or_default();
            if newest_live(&versions).is_some_and(|version| same(version, &write.record)) {
                outcome.skipped += 1;
                continue;
            }
            match self.append_record_patiently(place, &write.record).await {
                Ok(appended) => {
                    outcome.imported += 1;
                    last = appended.or(last);
                    replaced.extend(versions);
                }
                Err(Fault::Refused(why)) => refuse(outcome, format!("{}: {why}", write.label)),
                Err(Fault::Batch(error)) => return Err(error),
            }
        }
        if let Some(event) = last {
            records.wait(place, &event).await.map_err(engine_error)?;
        }
        records.retire(place, &replaced, SUPERSEDED).await;
        Ok(())
    }

    async fn import_turns(
        &self,
        turns: Vec<EpisodicTurn>,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let place = self.episodic_place(TURNS)?;
        let records = Records::new(&self.dialect);
        let mut outcome = EpisodicImportOutcome::default();
        let keys: Vec<String> = turns.iter().filter_map(|t| t.id).map(turn_key).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let held = records
            .versions(&place, &key_refs)
            .await
            .map_err(engine_error)?;
        // A session's turns, read only when one of its turns meets another
        // under its id.
        let mut sessions: HashMap<String, Vec<EpisodicTurn>> = HashMap::new();
        // The lookup above was made before this batch wrote anything, so the
        // batch keeps its own account: every id it carries is spoken for — a
        // fresh id must not land on a turn still to come in it — and every
        // turn it writes is what a later turn under that id meets.
        let mut reserved: HashSet<i64> = turns.iter().filter_map(|t| t.id).collect();
        let mut written: HashMap<i64, EpisodicTurn> = HashMap::new();
        let mut last = None;
        for turn in turns {
            let Some(id) = turn.id else {
                refuse(
                    &mut outcome,
                    "turn without an id: an imported turn needs the id it was exported with"
                        .to_string(),
                );
                continue;
            };
            if turn.session_id.trim().is_empty() || turn.role.trim().is_empty() {
                refuse(
                    &mut outcome,
                    format!("turn {id}: it needs a session id and a role"),
                );
                continue;
            }
            let wanted = EpisodicTurn {
                cost_microdollars: turn.cost_microdollars.max(0),
                ..turn
            };
            let current = written.get(&id).cloned().or_else(|| {
                held.get(&turn_key(id))
                    .and_then(|versions| newest_live(versions))
                    .and_then(parse::<EpisodicTurn>)
            });
            let target = match current {
                Some(existing) if existing == wanted => {
                    outcome.skipped += 1;
                    continue;
                }
                None => id,
                Some(_) => {
                    if !sessions.contains_key(&wanted.session_id) {
                        let read: Vec<EpisodicTurn> = records
                            .of_session(&place, &wanted.session_id)
                            .await
                            .map_err(engine_error)?
                            .iter()
                            .filter_map(parse::<EpisodicTurn>)
                            .collect();
                        sessions.insert(wanted.session_id.clone(), read);
                    }
                    let session = sessions
                        .get(&wanted.session_id)
                        .map_or(&[][..], Vec::as_slice);
                    let moved = session.iter().find(|held| {
                        held.id != Some(id)
                            && EpisodicTurn {
                                id: Some(id),
                                ..(*held).clone()
                            } == wanted
                    });
                    if let Some(at) = moved.and_then(|held| held.id) {
                        outcome.skipped += 1;
                        outcome.remapped.push(TurnIdRemap { from: id, to: at });
                        continue;
                    }
                    let mut fresh = next_turn_id();
                    while reserved.contains(&fresh) {
                        fresh = next_turn_id();
                    }
                    fresh
                }
            };
            let stored = EpisodicTurn {
                id: Some(target),
                ..wanted
            };
            let record = session_record(
                turn_key(target),
                serde_json::to_string(&stored)?,
                &stored.session_id,
            );
            match self.append_record_patiently(&place, &record).await {
                Ok(appended) => {
                    outcome.imported += 1;
                    if target != id {
                        outcome.remapped.push(TurnIdRemap {
                            from: id,
                            to: target,
                        });
                    }
                    reserved.insert(target);
                    written.insert(target, stored);
                    last = appended.or(last);
                }
                Err(Fault::Refused(why)) => refuse(&mut outcome, format!("turn {id}: {why}")),
                Err(Fault::Batch(error)) => return Err(error),
            }
        }
        if let Some(event) = last {
            records.wait(&place, &event).await.map_err(engine_error)?;
        }
        Ok(outcome)
    }

    async fn import_segments(
        &self,
        segments: Vec<ConversationSegment>,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let place = self.episodic_place(SEGMENTS)?;
        let keys: Vec<&str> = segments.iter().map(|s| s.segment_id.as_str()).collect();
        let held = Records::new(&self.dialect)
            .versions(&place, &keys)
            .await
            .map_err(engine_error)?;
        let mut outcome = EpisodicImportOutcome::default();
        let mut writes = Vec::new();
        for segment in segments {
            if segment.segment_id.trim().is_empty() || segment.session_id.trim().is_empty() {
                refuse(
                    &mut outcome,
                    "segment: it needs an id and a session id".to_string(),
                );
                continue;
            }
            // A segment already here keeps the creation time that orders it
            // among its session's segments; a new one was created when it
            // started.
            let created_at = held
                .get(&segment.segment_id)
                .and_then(|versions| newest_live(versions))
                .and_then(parse::<StoredSegment>)
                .map_or(segment.start_timestamp, |stored| stored.created_at);
            let stored = StoredSegment {
                segment,
                created_at,
            };
            writes.push(Write {
                label: format!("segment {}", stored.segment.segment_id),
                record: session_record(
                    stored.segment.segment_id.clone(),
                    serde_json::to_string(&stored)?,
                    &stored.segment.session_id,
                ),
            });
        }
        self.write_keyed(
            &place,
            writes,
            |version, record| {
                let held = parse::<StoredSegment>(version).map(|s| s.segment);
                let wanted = serde_json::from_str::<StoredSegment>(&record.content)
                    .ok()
                    .map(|s| s.segment);
                held.is_some() && held == wanted
            },
            &mut outcome,
        )
        .await?;
        Ok(outcome)
    }

    async fn import_events(
        &self,
        events: Vec<EpisodicEvent>,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let place = self.episodic_place(EPISODIC_EVENTS)?;
        let mut outcome = EpisodicImportOutcome::default();
        let mut writes = Vec::new();
        for event in events {
            if event.event_id.trim().is_empty() {
                refuse(&mut outcome, "event: it needs an id".to_string());
                continue;
            }
            writes.push(Write {
                label: format!("event {}", event.event_id),
                record: session_record(
                    event.event_id.clone(),
                    serde_json::to_string(&event)?,
                    &event.session_id,
                ),
            });
        }
        self.write_keyed(
            &place,
            writes,
            |version, record| {
                let held = parse::<EpisodicEvent>(version);
                held.is_some() && held == serde_json::from_str(&record.content).ok()
            },
            &mut outcome,
        )
        .await?;
        Ok(outcome)
    }

    async fn import_segment_embeddings(
        &self,
        embeddings: Vec<SegmentEmbedding>,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let place = self.episodic_place(SEGMENT_EMBEDDINGS)?;
        let mut outcome = EpisodicImportOutcome::default();
        let mut writes = Vec::new();
        for embedding in embeddings {
            let stored = StoredEmbedding {
                segment_id: embedding.segment_id,
                model_signature: embedding.model_signature,
                embedding: embedding.embedding,
                created_at: embedding.created_at,
            };
            let key = embedding_key(&stored.segment_id, &stored.model_signature);
            writes.push(Write {
                label: format!("embedding {key}"),
                record: Record::plain(key, serde_json::to_string(&stored)?),
            });
        }
        self.write_keyed(
            &place,
            writes,
            |version, record| {
                let held = parse::<StoredEmbedding>(version);
                held.is_some() && held == serde_json::from_str(&record.content).ok()
            },
            &mut outcome,
        )
        .await?;
        Ok(outcome)
    }
}

#[async_trait]
impl MemoryEpisodicPortability for CortexProvider {
    async fn export_episodic(
        &self,
        part: EpisodicPart,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<EpisodicExportPage, MemoryError> {
        if limit == 0 {
            return Err(MemoryError::Invalid(
                "episodic export page limit must be greater than zero".to_string(),
            ));
        }
        let after = key_of(part, cursor)?;
        let (records, next) = match part {
            EpisodicPart::Turns => {
                let after = after
                    .map(|key| key.parse::<i64>().map_err(|_| invalid_cursor(part)))
                    .transpose()?;
                let mut turns: Vec<EpisodicTurn> = self
                    .live_records(TURNS)
                    .await?
                    .iter()
                    .filter_map(parse::<EpisodicTurn>)
                    .filter(|turn| turn.id.is_some_and(|id| after.is_none_or(|a| id > a)))
                    .collect();
                turns.sort_by_key(|turn| turn.id);
                let ordered = turns
                    .into_iter()
                    .map(|turn| (turn.id.unwrap_or_default().to_string(), turn))
                    .collect();
                let (turns, next) = page(ordered, limit, |turn: &EpisodicTurn| {
                    turn.content.len()
                        + turn.lesson.as_ref().map_or(0, String::len)
                        + turn.tool_calls_json.as_ref().map_or(0, String::len)
                });
                (EpisodicRecords::Turns(turns), next)
            }
            EpisodicPart::Segments => {
                let ordered = self
                    .live_records(SEGMENTS)
                    .await?
                    .iter()
                    .filter_map(parse::<StoredSegment>)
                    .map(|stored| (stored.segment.segment_id.clone(), stored.segment))
                    .filter(|(key, _)| after.is_none_or(|a| key.as_str() > a))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect();
                let (segments, next) = page(ordered, limit, |segment: &ConversationSegment| {
                    segment.summary.as_ref().map_or(0, String::len)
                        + segment.embedding.as_ref().map_or(0, |v| v.len() * 16)
                });
                (EpisodicRecords::Segments(segments), next)
            }
            EpisodicPart::Events => {
                let ordered = self
                    .live_records(EPISODIC_EVENTS)
                    .await?
                    .iter()
                    .filter_map(parse::<EpisodicEvent>)
                    .map(|event| (event.event_id.clone(), event))
                    .filter(|(key, _)| after.is_none_or(|a| key.as_str() > a))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect();
                let (events, next) = page(ordered, limit, |event: &EpisodicEvent| {
                    event.content.len() + event.embedding.as_ref().map_or(0, |v| v.len() * 16)
                });
                (EpisodicRecords::Events(events), next)
            }
            EpisodicPart::SegmentEmbeddings => {
                let after: Option<(String, String)> = after
                    .map(|key| serde_json::from_str(key).map_err(|_| invalid_cursor(part)))
                    .transpose()?;
                let ordered = self
                    .live_records(SEGMENT_EMBEDDINGS)
                    .await?
                    .iter()
                    .filter_map(parse::<StoredEmbedding>)
                    .map(|stored| {
                        (
                            (stored.segment_id.clone(), stored.model_signature.clone()),
                            stored,
                        )
                    })
                    .filter(|(key, _)| after.as_ref().is_none_or(|a| key > a))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, stored)| Ok((serde_json::to_string(&key)?, stored)))
                    .collect::<Result<Vec<_>, MemoryError>>()?;
                let (embeddings, next) = page(ordered, limit, |stored: &StoredEmbedding| {
                    stored.embedding.len() * 16
                });
                (
                    EpisodicRecords::SegmentEmbeddings(
                        embeddings
                            .into_iter()
                            .map(|stored| SegmentEmbedding {
                                segment_id: stored.segment_id,
                                model_signature: stored.model_signature,
                                embedding: stored.embedding,
                                created_at: stored.created_at,
                            })
                            .collect(),
                    ),
                    next,
                )
            }
        };
        Ok(EpisodicExportPage {
            records,
            next_cursor: next.map(|key| format!("{}:{key}", part.as_str())),
        })
    }

    async fn import_episodic(
        &self,
        records: EpisodicRecords,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        Ok(match records {
            EpisodicRecords::Turns(turns) => self.import_turns(turns).await?,
            EpisodicRecords::Segments(segments) => self.import_segments(segments).await?,
            EpisodicRecords::Events(events) => self.import_events(events).await?,
            EpisodicRecords::SegmentEmbeddings(embeddings) => {
                self.import_segment_embeddings(embeddings).await?
            }
        })
    }
}

#[cfg(test)]
#[path = "episodic_portability_tests.rs"]
mod test;
