//! [`MigrateStep::Episodic`](super::MigrateStep::Episodic): the episodic
//! record, a part at a time, turns first.
//!
//! A target keeps a turn's id unless a different turn already holds it, and
//! reports the turns it had to move. Segments and events name turns by id, so
//! every reference to a moved turn is rewritten before they are imported —
//! one lookup per reference, never chained, because a moved turn's new id can
//! be another turn's old one.

use std::collections::{HashMap, HashSet};

use crate::capabilities::Capability;
use crate::provider::{EpisodicPart, EpisodicRecords, MemoryProvider};

use super::{unserved, CopyProgress, MigrateStep, StepReport};

/// How many records one episodic export page asks for.
const EPISODIC_PAGE: usize = 500;

/// A hard stop on pages per part, as [`super::copy`] has.
const MAX_PAGES: usize = 100_000;

/// Rewrites a turn id that moved; leaves the rest alone.
fn moved(map: &HashMap<i64, i64>, id: i64) -> i64 {
    map.get(&id).copied().unwrap_or(id)
}

/// Rewrites the turn ids in an event's `source_turn_ids`, which the caller
/// encodes. The two encodings in use — a JSON array of ids and a
/// comma-separated list — are rewritten in their own form; anything else is
/// left as it is.
pub(super) fn remap_turn_ids(encoded: &str, map: &HashMap<i64, i64>) -> String {
    if map.is_empty() {
        return encoded.to_string();
    }
    if let Ok(ids) = serde_json::from_str::<Vec<i64>>(encoded) {
        let ids: Vec<i64> = ids.into_iter().map(|id| moved(map, id)).collect();
        return serde_json::to_string(&ids).unwrap_or_else(|_| encoded.to_string());
    }
    let parts: Option<Vec<i64>> = encoded
        .split(',')
        .map(|part| part.trim().parse::<i64>().ok())
        .collect();
    match parts {
        Some(ids) if !encoded.trim().is_empty() => ids
            .into_iter()
            .map(|id| moved(map, id).to_string())
            .collect::<Vec<_>>()
            .join(","),
        _ => encoded.to_string(),
    }
}

/// `records` with every reference to a moved turn rewritten.
pub(super) fn remap(records: EpisodicRecords, map: &HashMap<i64, i64>) -> EpisodicRecords {
    if map.is_empty() {
        return records;
    }
    match records {
        EpisodicRecords::Segments(mut segments) => {
            for segment in &mut segments {
                segment.start_episodic_id = moved(map, segment.start_episodic_id);
                segment.end_episodic_id = segment.end_episodic_id.map(|id| moved(map, id));
            }
            EpisodicRecords::Segments(segments)
        }
        EpisodicRecords::Events(mut events) => {
            for event in &mut events {
                event.source_turn_ids = event
                    .source_turn_ids
                    .as_deref()
                    .map(|encoded| remap_turn_ids(encoded, map));
            }
            EpisodicRecords::Events(events)
        }
        other => other,
    }
}

/// Copies the episodic record from `from` into `to`.
pub(super) async fn copy(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    progress: &mut impl FnMut(CopyProgress),
) -> anyhow::Result<StepReport> {
    let step = MigrateStep::Episodic;
    let (Some(source), Some(target)) =
        (from.as_episodic_portability(), to.as_episodic_portability())
    else {
        let side = if from.as_episodic_portability().is_none() {
            "source"
        } else {
            "target"
        };
        return Ok(StepReport::skipped(
            step,
            unserved(side, Capability::EpisodicPortability),
        ));
    };
    let mut report = StepReport::new(step);
    let mut moved_turns: HashMap<i64, i64> = HashMap::new();
    for part in EpisodicPart::ALL {
        let mut cursor: Option<String> = None;
        let mut seen: HashSet<String> = HashSet::new();
        let mut pages = 0usize;
        loop {
            anyhow::ensure!(
                pages < MAX_PAGES,
                "the source's {part} export did not terminate after {MAX_PAGES} pages"
            );
            let page = source
                .export_episodic(part, cursor.as_deref(), EPISODIC_PAGE)
                .await?;
            pages += 1;
            anyhow::ensure!(
                page.records.part() == part,
                "the source answered a {part} page with {} records",
                page.records.part()
            );
            if !page.records.is_empty() {
                report.read += page.records.len();
                let outcome = target
                    .import_episodic(remap(page.records, &moved_turns))
                    .await?;
                report.written += usize::try_from(outcome.imported).unwrap_or(usize::MAX);
                report.unchanged += usize::try_from(outcome.skipped).unwrap_or(usize::MAX);
                report.failed += usize::try_from(outcome.failed).unwrap_or(usize::MAX);
                for error in outcome.errors {
                    report.note(error);
                }
                for pair in outcome.remapped {
                    moved_turns.insert(pair.from, pair.to);
                }
            }
            progress(CopyProgress {
                step,
                read: report.read,
                written: report.written,
            });
            match page.next_cursor {
                Some(next) => {
                    anyhow::ensure!(
                        cursor.as_deref() != Some(next.as_str()) && seen.insert(next.clone()),
                        "the source's {part} export cursor repeated after {pages} pages; \
                         refusing to loop"
                    );
                    cursor = Some(next);
                }
                None => break,
            }
        }
    }
    Ok(report)
}
