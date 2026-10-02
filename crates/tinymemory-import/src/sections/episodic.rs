//! `episodic_log`: one conversation per thread.
//!
//! Threads are walked by `session_id`; each thread's turns are read in
//! `(timestamp, id)` order. Blank turns are dropped (a v2 conversation turn
//! must have text), and a thread with no remaining turns is skipped. The
//! `lesson` and `cost_microdollars` columns are not imported: lessons were
//! distilled into the learning rows the learnings section imports.

use rusqlite::params;
use tinymemory_api::{StoreItem, Turn, TurnRange};

use super::{Mark, Scanned, import_meta, sql_limit};
use crate::convert;
use crate::error::Result;
use crate::workspace::LegacyWorkspace;

/// The next page of threads after `after`.
pub(super) fn page(
    ws: &LegacyWorkspace,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Scanned>> {
    let mut stmt = ws.memory.prepare(
        "SELECT DISTINCT session_id FROM episodic_log WHERE (?1 IS NULL OR session_id > ?1) \
         ORDER BY session_id LIMIT ?2",
    )?;
    let sessions = stmt
        .query_map(params![after, sql_limit(limit)], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    sessions
        .into_iter()
        .map(|session| {
            Ok(Scanned {
                item: conversation(ws, &session)?,
                mark: Mark::Conversation(session),
            })
        })
        .collect()
}

fn conversation(ws: &LegacyWorkspace, session: &str) -> Result<Option<StoreItem>> {
    let tool_calls = if ws.schema.tool_calls_json {
        "tool_calls_json"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT role, content, timestamp, {tool_calls} FROM episodic_log \
         WHERE session_id = ?1 ORDER BY timestamp, id"
    );
    let mut stmt = ws.memory.prepare(&sql)?;
    let rows = stmt.query_map([session], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, Option<f64>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut turns = Vec::new();
    for row in rows {
        let (role, text, timestamp, calls) = row?;
        if text.trim().is_empty() {
            continue;
        }
        turns.push(Turn {
            role: convert::role(&role),
            text,
            at: timestamp.and_then(convert::from_unix_seconds),
            tool_calls: calls.as_deref().map(convert::tool_calls).unwrap_or_default(),
        });
    }
    let Some(last) = turns.len().checked_sub(1) else {
        return Ok(None);
    };
    let mut meta = import_meta(ws, format!("episodic_log:{session}"));
    meta.thread_id = Some(session.to_string());
    meta.turns = Some(TurnRange {
        first: 0,
        last: u32::try_from(last).unwrap_or(u32::MAX),
    });
    meta.observed_at = turns.iter().rev().find_map(|turn| turn.at);
    Ok(Some(StoreItem::Conversation { turns, meta }))
}
