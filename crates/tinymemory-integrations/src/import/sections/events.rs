//! `event_log`: one learning per extracted event.
//!
//! The v1 engine extracted typed atomic events (fact, decision, commitment,
//! preference, question, foresight) from closed conversation segments: what
//! the user said they decided, promised or prefer, distilled. Each becomes a
//! learning, walked by `event_id`. A blank event is skipped. A store from
//! before the table existed (or with a partial one) has no events section.
//!
//! | `event_type` | Kind |
//! | --- | --- |
//! | `fact`, `decision` | `Fact` |
//! | `preference` | `Preference` |
//! | `commitment`, `question`, `foresight`, unknown | `Other` |

use rusqlite::params;
use tinymemory_api::{LearningKind, StoreItem};

use super::{Mark, Scanned, count_of, has_text, import_meta, push_unique, sql_limit};
use crate::import::convert;
use crate::import::error::Result;
use crate::import::workspace::LegacyWorkspace;

/// One `event_log` row.
#[derive(Debug)]
struct EventRow {
    event_id: String,
    session_id: Option<String>,
    event_type: String,
    content: String,
    subject: Option<String>,
    confidence: Option<f64>,
    created_at: Option<f64>,
}

/// The next page of events after `after`.
pub(super) fn page(
    ws: &LegacyWorkspace,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Scanned>> {
    let Some(memory) = ws.memory.as_ref().filter(|_| ws.schema.event_log) else {
        return Ok(Vec::new());
    };
    let mut stmt = memory.prepare(
        "SELECT event_id, session_id, event_type, content, subject, confidence, created_at \
         FROM event_log WHERE (?1 IS NULL OR event_id > ?1) ORDER BY event_id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after, sql_limit(limit)], |row| {
        Ok(EventRow {
            event_id: row.get(0)?,
            session_id: row.get(1)?,
            event_type: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            content: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            subject: row.get(4)?,
            confidence: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.map(|row| {
        let row = row?;
        Ok(Scanned {
            item: event(ws, &row),
            mark: Mark::Event(row.event_id),
        })
    })
    .collect()
}

/// Events with text.
pub(super) fn count(ws: &LegacyWorkspace) -> Result<u64> {
    let Some(memory) = ws.memory.as_ref().filter(|_| ws.schema.event_log) else {
        return Ok(0);
    };
    let sql = format!(
        "SELECT COUNT(*) FROM event_log WHERE {}",
        has_text("content")
    );
    Ok(count_of(memory.query_row(&sql, [], |row| row.get(0))?))
}

/// The learning kind an `event_type` maps to.
fn kind(event_type: &str) -> LearningKind {
    match event_type.trim().to_ascii_lowercase().as_str() {
        "fact" | "decision" => LearningKind::Fact,
        "preference" => LearningKind::Preference,
        _ => LearningKind::Other,
    }
}

fn event(ws: &LegacyWorkspace, row: &EventRow) -> Option<StoreItem> {
    if row.content.trim().is_empty() {
        return None;
    }
    let mut meta = import_meta(ws, format!("event_log:{}", row.event_id));
    let event_type = row.event_type.trim().to_ascii_lowercase();
    if !event_type.is_empty() {
        push_unique(&mut meta.tags, format!("event:{event_type}"));
    }
    if let Some(subject) = row.subject.as_deref().map(str::trim)
        && !subject.is_empty()
    {
        push_unique(&mut meta.tags, format!("subject:{subject}"));
    }
    meta.thread_id = row
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    meta.observed_at = row.created_at.and_then(convert::from_unix_seconds);
    Some(StoreItem::Learning {
        text: row.content.clone(),
        kind: kind(&row.event_type),
        confidence: convert::confidence(row.confidence),
        evidence: None,
        meta,
    })
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
