//! `memory_docs`: documents, learnings and `global` rows.
//!
//! A row's section comes from its logical namespace: `logical_namespace` when
//! that column exists and is set, else `namespace` with the sanitisation
//! (`:` stored as `_`) undone for the known v1 section prefixes. Then:
//!
//! - `learning:<class>` (or bare `learning`) → a learning;
//! - `global` → a learning;
//! - `event:<ns>` → skipped: raw event payloads the v1 engine kept as
//!   bookkeeping, whose meaning already lives in the episodic log and the
//!   learnings distilled from them;
//! - anything else (`document:*`, `source:*`, `conversation:*`, custom
//!   `Memory::store` namespaces) → a document.
//!
//! Both sections scan the whole table by `document_id` and skip the rows that
//! belong to the other one.

use rusqlite::params;
use serde_json::Value;
use tinymemory_api::{DocumentBody, LearningKind, StoreItem};

use super::{Mark, Scanned, import_meta, push_unique, sql_limit};
use crate::convert;
use crate::error::Result;
use crate::workspace::LegacyWorkspace;

/// v1 section prefixes whose `:` separator the sanitiser turned into `_`.
const SECTION_PREFIXES: [&str; 9] = [
    "conversation",
    "document",
    "learning",
    "entity",
    "profile",
    "tool",
    "source",
    "custom",
    "event",
];

/// One `memory_docs` row.
#[derive(Debug, Clone)]
struct DocRow {
    document_id: String,
    namespace: String,
    logical_namespace: Option<String>,
    title: String,
    content: String,
    tags_json: String,
    metadata_json: String,
    updated_at: Option<f64>,
}

/// Which section a row belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowClass {
    Document,
    Learning(Option<String>),
    Global,
    Event,
}

/// The documents section.
pub(super) fn documents(
    ws: &LegacyWorkspace,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Scanned>> {
    Ok(rows(ws, after, limit)?
        .into_iter()
        .map(|row| {
            let logical = logical_namespace(&row);
            let item = match classify(&logical) {
                RowClass::Document => document(ws, &row, logical),
                _ => None,
            };
            Scanned {
                mark: Mark::Document(row.document_id),
                item,
            }
        })
        .collect())
}

/// The learnings section.
pub(super) fn learnings(
    ws: &LegacyWorkspace,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Scanned>> {
    Ok(rows(ws, after, limit)?
        .into_iter()
        .map(|row| {
            let item = match classify(&logical_namespace(&row)) {
                RowClass::Learning(class) => learning(ws, &row, class),
                RowClass::Global => global(ws, &row),
                RowClass::Document | RowClass::Event => None,
            };
            Scanned {
                mark: Mark::Learning(row.document_id),
                item,
            }
        })
        .collect())
}

fn rows(ws: &LegacyWorkspace, after: Option<&str>, limit: usize) -> Result<Vec<DocRow>> {
    let logical = if ws.schema.logical_namespace {
        "logical_namespace"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT document_id, namespace, {logical}, title, content, tags_json, metadata_json, \
         updated_at FROM memory_docs WHERE (?1 IS NULL OR document_id > ?1) \
         ORDER BY document_id LIMIT ?2"
    );
    let mut stmt = ws.memory.prepare(&sql)?;
    let rows = stmt.query_map(params![after, sql_limit(limit)], |row| {
        Ok(DocRow {
            document_id: row.get(0)?,
            namespace: row.get(1)?,
            logical_namespace: row.get(2)?,
            title: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            content: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            tags_json: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            metadata_json: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            updated_at: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn logical_namespace(row: &DocRow) -> String {
    match &row.logical_namespace {
        Some(logical) if !logical.trim().is_empty() => logical.clone(),
        _ => restore_namespace(&row.namespace),
    }
}

/// Undoes the v1 sanitiser for a known section prefix: `learning_style` →
/// `learning:style`. Only the first separator can be restored; the rest of
/// the name is kept as stored.
fn restore_namespace(namespace: &str) -> String {
    if namespace.contains(':') {
        return namespace.to_string();
    }
    for prefix in SECTION_PREFIXES {
        if let Some(rest) = namespace
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('_'))
        {
            return format!("{prefix}:{rest}");
        }
    }
    namespace.to_string()
}

fn classify(logical: &str) -> RowClass {
    if logical == "global" {
        return RowClass::Global;
    }
    if logical == "learning" {
        return RowClass::Learning(None);
    }
    if let Some(class) = logical.strip_prefix("learning:") {
        let class = class.trim();
        return RowClass::Learning((!class.is_empty()).then(|| class.to_string()));
    }
    if logical == "event" || logical.starts_with("event:") {
        return RowClass::Event;
    }
    RowClass::Document
}

fn document(ws: &LegacyWorkspace, row: &DocRow, logical: String) -> Option<StoreItem> {
    if row.content.trim().is_empty() {
        return None;
    }
    let mut meta = import_meta(ws, format!("memory_docs:{}", row.document_id));
    let mut tags = convert::string_array(&row.tags_json);
    push_unique(&mut tags, format!("ns:{logical}"));
    meta.tags = tags;
    meta.observed_at = row.updated_at.and_then(convert::from_unix_seconds);
    let metadata = convert::object(&row.metadata_json);
    meta.url = metadata
        .as_ref()
        .and_then(|map| convert::string_field(map, "url"));
    let mime = metadata
        .as_ref()
        .and_then(|map| convert::string_field(map, "mime"));
    let title = row.title.trim();
    Some(StoreItem::Document {
        title: (!title.is_empty()).then(|| title.to_string()),
        body: DocumentBody::Text(row.content.clone()),
        mime,
        meta,
    })
}

/// A `learning:<class>` row. Its content is a JSON `LearningCandidate`; one
/// that does not parse (or lacks `key`/`value`) is kept as an
/// [`LearningKind::Other`] learning of its raw text at the default
/// confidence.
fn learning(ws: &LegacyWorkspace, row: &DocRow, class: Option<String>) -> Option<StoreItem> {
    let mut meta = import_meta(ws, format!("memory_docs:{}", row.document_id));
    let updated = row.updated_at.and_then(convert::from_unix_seconds);
    let candidate = convert::object(&row.content);
    let parsed = candidate.as_ref().and_then(|map| {
        let key = convert::string_field(map, "key")?;
        let value = map.get("value").filter(|value| !value.is_null())?;
        Some((map, key, convert::value_text(value)))
    });
    let Some((map, key, value)) = parsed else {
        if row.content.trim().is_empty() {
            return None;
        }
        meta.tags = class.into_iter().collect();
        meta.observed_at = updated;
        return Some(StoreItem::Learning {
            text: row.content.clone(),
            kind: LearningKind::Other,
            confidence: convert::DEFAULT_CONFIDENCE,
            evidence: None,
            meta,
        });
    };
    let class = convert::string_field(map, "class").or(class);
    let kind = class
        .as_deref()
        .map_or(LearningKind::Other, convert::learning_kind);
    meta.tags = class.into_iter().collect();
    meta.observed_at = map
        .get("observed_at")
        .and_then(Value::as_f64)
        .and_then(convert::from_unix_seconds)
        .or(updated);
    Some(StoreItem::Learning {
        text: format!("{key}: {value}"),
        kind,
        confidence: convert::confidence(map.get("initial_confidence").and_then(Value::as_f64)),
        evidence: map
            .get("evidence")
            .filter(|evidence| !evidence.is_null())
            .map(Value::to_string),
        meta,
    })
}

/// A `global` row: free text the v1 host stored as always-relevant, kept as a
/// [`LearningKind::Fact`] at the default confidence.
fn global(ws: &LegacyWorkspace, row: &DocRow) -> Option<StoreItem> {
    if row.content.trim().is_empty() {
        return None;
    }
    let mut meta = import_meta(ws, format!("memory_docs:{}", row.document_id));
    meta.tags = vec!["global".to_string()];
    meta.observed_at = row.updated_at.and_then(convert::from_unix_seconds);
    Some(StoreItem::Learning {
        text: row.content.clone(),
        kind: LearningKind::Fact,
        confidence: convert::DEFAULT_CONFIDENCE,
        evidence: None,
        meta,
    })
}

#[cfg(test)]
#[path = "memory_docs_tests.rs"]
mod tests;
