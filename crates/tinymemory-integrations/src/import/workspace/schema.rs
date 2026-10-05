//! Probing which tables and columns a legacy database has.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{is_not_a_database, open_read_only};
use crate::import::error::Result;

/// Tables a v1 `memory.db` always has.
const REQUIRED_TABLES: [&str; 3] = ["memory_docs", "episodic_log", "user_profile"];

/// Columns the importer reads that every v1 release wrote.
const REQUIRED_COLUMNS: [(&str, &[&str]); 3] = [
    (
        "memory_docs",
        &[
            "document_id",
            "namespace",
            "title",
            "content",
            "tags_json",
            "metadata_json",
            "updated_at",
        ],
    ),
    (
        "episodic_log",
        &["id", "session_id", "timestamp", "role", "content"],
    ),
    (
        "user_profile",
        &[
            "facet_id",
            "facet_type",
            "key",
            "value",
            "confidence",
            "last_seen_at",
        ],
    ),
];

/// Columns `mem_tree_chunks` must have for the chunk section to run.
const CHUNK_COLUMNS: [&str; 7] = [
    "id",
    "source_kind",
    "source_id",
    "timestamp_ms",
    "tags_json",
    "content",
    "seq_in_source",
];

/// Which optional columns `memory.db` has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MemorySchema {
    /// `memory_docs.logical_namespace`.
    pub(crate) logical_namespace: bool,
    /// `memory_docs.taint`.
    pub(crate) taint: bool,
    /// `episodic_log.tool_calls_json`.
    pub(crate) tool_calls_json: bool,
    /// `user_profile.state`.
    pub(crate) profile_state: bool,
    /// `user_profile.user_state`.
    pub(crate) profile_user_state: bool,
    /// `user_profile.class`.
    pub(crate) profile_class: bool,
    /// `user_profile.evidence_refs_json`.
    pub(crate) profile_evidence: bool,
}

impl MemorySchema {
    /// Probes `memory.db`. The outer error is SQLite failing; the inner one
    /// is the reason the database is not a v1 store.
    pub(crate) fn probe(conn: &Connection) -> rusqlite::Result<std::result::Result<Self, String>> {
        let tables = tables(conn)?;
        if let Some(missing) = REQUIRED_TABLES.iter().find(|t| !tables.contains(**t)) {
            return Ok(Err(format!("table {missing} is missing")));
        }
        for (table, required) in REQUIRED_COLUMNS {
            let present = columns(conn, table)?;
            if let Some(missing) = required.iter().find(|c| !present.contains(**c)) {
                return Ok(Err(format!("column {table}.{missing} is missing")));
            }
        }
        let docs = columns(conn, "memory_docs")?;
        let episodic = columns(conn, "episodic_log")?;
        let profile = columns(conn, "user_profile")?;
        Ok(Ok(Self {
            logical_namespace: docs.contains("logical_namespace"),
            taint: docs.contains("taint"),
            tool_calls_json: episodic.contains("tool_calls_json"),
            profile_state: profile.contains("state"),
            profile_user_state: profile.contains("user_state"),
            profile_class: profile.contains("class"),
            profile_evidence: profile.contains("evidence_refs_json"),
        }))
    }
}

/// `memory_tree/chunks.db`, opened read-only.
#[derive(Debug)]
pub(crate) struct ChunkStore {
    /// The open database.
    pub(crate) conn: Connection,
    /// `memory_tree/content`, where full chunk bodies live.
    pub(crate) content_dir: PathBuf,
    /// Whether `mem_tree_chunks.content_path` exists.
    pub(crate) content_path: bool,
}

impl ChunkStore {
    /// Opens the chunk store under `root`, or `None` when it is absent or
    /// unusable.
    pub(crate) fn open(root: &Path) -> Result<Option<Self>> {
        let tree = root.join("memory_tree");
        let db = tree.join("chunks.db");
        if !db.is_file() {
            return Ok(None);
        }
        let conn = open_read_only(&db)?;
        let present = match tables(&conn) {
            Ok(tables) if tables.contains("mem_tree_chunks") => columns(&conn, "mem_tree_chunks")?,
            Ok(_) => return Ok(None),
            Err(err) if is_not_a_database(&err) => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        if CHUNK_COLUMNS.iter().any(|c| !present.contains(*c)) {
            return Ok(None);
        }
        Ok(Some(Self {
            content_path: present.contains("content_path"),
            content_dir: tree.join("content"),
            conn,
        }))
    }
}

fn tables(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(0))?;
    names.collect()
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let names = stmt.query_map([table], |row| row.get::<_, String>(0))?;
    names.collect()
}
