//! Builds v1 TinyCortex workspaces in temporary directories, using the
//! verbatim v1 DDL.

use std::path::Path;

use rusqlite::{Connection, params};
use tempfile::TempDir;

/// The current v1 `memory.db` schema, verbatim.
pub(crate) const MEMORY_DDL: &str = "
CREATE TABLE memory_docs (
  document_id TEXT PRIMARY KEY, namespace TEXT NOT NULL, key TEXT NOT NULL, title TEXT NOT NULL,
  content TEXT NOT NULL, source_type TEXT NOT NULL, priority TEXT NOT NULL, tags_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL, category TEXT NOT NULL, session_id TEXT, created_at REAL NOT NULL,
  updated_at REAL NOT NULL, markdown_rel_path TEXT NOT NULL, taint TEXT NOT NULL DEFAULT 'internal',
  logical_namespace TEXT, UNIQUE(namespace, key));
CREATE TABLE kv_global (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at REAL NOT NULL);
CREATE TABLE kv_namespace (namespace TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY(namespace, key));
CREATE TABLE episodic_log (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, timestamp REAL NOT NULL,
  role TEXT NOT NULL, content TEXT NOT NULL, lesson TEXT, tool_calls_json TEXT, cost_microdollars INTEGER DEFAULT 0);
CREATE TABLE user_profile (facet_id TEXT PRIMARY KEY, facet_type TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 0.5, evidence_count INTEGER NOT NULL DEFAULT 1, source_segment_ids TEXT,
  first_seen_at REAL NOT NULL, last_seen_at REAL NOT NULL, state TEXT NOT NULL DEFAULT 'active',
  stability REAL NOT NULL DEFAULT 0.0, user_state TEXT NOT NULL DEFAULT 'auto', evidence_refs_json TEXT,
  class TEXT, cue_families_json TEXT, UNIQUE(facet_type, key));
";

/// The v1 `event_log` table, as the engine created it.
pub(crate) const EVENTS_DDL: &str = "
CREATE TABLE event_log (event_id TEXT PRIMARY KEY, segment_id TEXT NOT NULL, session_id TEXT NOT NULL,
  namespace TEXT NOT NULL DEFAULT 'global', event_type TEXT NOT NULL, content TEXT NOT NULL, subject TEXT,
  timestamp_ref TEXT, confidence REAL NOT NULL, embedding BLOB, source_turn_ids TEXT, created_at REAL NOT NULL);
";

/// An early v1 `memory.db`: no `taint`/`logical_namespace`, no
/// `tool_calls_json`, and no profile columns from `state` on.
pub(crate) const OLD_MEMORY_DDL: &str = "
CREATE TABLE memory_docs (
  document_id TEXT PRIMARY KEY, namespace TEXT NOT NULL, key TEXT NOT NULL, title TEXT NOT NULL,
  content TEXT NOT NULL, source_type TEXT NOT NULL, priority TEXT NOT NULL, tags_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL, category TEXT NOT NULL, session_id TEXT, created_at REAL NOT NULL,
  updated_at REAL NOT NULL, markdown_rel_path TEXT NOT NULL, UNIQUE(namespace, key));
CREATE TABLE episodic_log (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, timestamp REAL NOT NULL,
  role TEXT NOT NULL, content TEXT NOT NULL, lesson TEXT);
CREATE TABLE user_profile (facet_id TEXT PRIMARY KEY, facet_type TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 0.5, evidence_count INTEGER NOT NULL DEFAULT 1, source_segment_ids TEXT,
  first_seen_at REAL NOT NULL, last_seen_at REAL NOT NULL);
";

/// The v1 `memory_tree/chunks.db` schema, verbatim, plus the migrated
/// `content_path` column.
pub(crate) const CHUNKS_DDL: &str = "
CREATE TABLE mem_tree_chunks (id TEXT PRIMARY KEY, source_kind TEXT NOT NULL, source_id TEXT NOT NULL, path_scope TEXT,
  source_ref TEXT, owner TEXT NOT NULL, timestamp_ms INTEGER NOT NULL, time_range_start_ms INTEGER NOT NULL,
  time_range_end_ms INTEGER NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]', content TEXT NOT NULL,
  token_count INTEGER NOT NULL, seq_in_source INTEGER NOT NULL, created_at_ms INTEGER NOT NULL);
ALTER TABLE mem_tree_chunks ADD COLUMN content_path TEXT;
";

/// A workspace directory with a `memory.db` built from `ddl`.
pub(crate) fn workspace(ddl: &str) -> (TempDir, Connection) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("memory")).expect("memory dir");
    let conn = Connection::open(dir.path().join("memory/memory.db")).expect("memory.db");
    conn.execute_batch(ddl).expect("memory ddl");
    (dir, conn)
}

/// Inserts a `memory_docs` row in the current schema.
#[allow(clippy::too_many_arguments, reason = "mirrors the table's columns")]
pub(crate) fn doc(
    conn: &Connection,
    id: &str,
    namespace: &str,
    logical: Option<&str>,
    title: &str,
    content: &str,
    tags: &str,
    metadata: &str,
    updated_at: f64,
) {
    conn.execute(
        "INSERT INTO memory_docs (document_id, namespace, key, title, content, source_type,
           priority, tags_json, metadata_json, category, created_at, updated_at,
           markdown_rel_path, logical_namespace)
         VALUES (?1, ?2, ?1, ?3, ?4, 'chat', 'normal', ?5, ?6, 'core', ?7, ?7, '', ?8)",
        params![
            id, namespace, title, content, tags, metadata, updated_at, logical
        ],
    )
    .expect("insert doc");
}

/// Inserts an `episodic_log` turn in the current schema.
pub(crate) fn turn(
    conn: &Connection,
    session: &str,
    timestamp: f64,
    role: &str,
    content: &str,
    tool_calls: Option<&str>,
) {
    conn.execute(
        "INSERT INTO episodic_log (session_id, timestamp, role, content, tool_calls_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session, timestamp, role, content, tool_calls],
    )
    .expect("insert turn");
}

/// Inserts a `user_profile` facet in the current schema.
#[allow(clippy::too_many_arguments, reason = "mirrors the table's columns")]
pub(crate) fn facet(
    conn: &Connection,
    id: &str,
    facet_type: &str,
    key: &str,
    value: &str,
    confidence: f64,
    last_seen_at: f64,
    state: &str,
    user_state: &str,
    class: Option<&str>,
) {
    conn.execute(
        "INSERT INTO user_profile (facet_id, facet_type, key, value, confidence, first_seen_at,
           last_seen_at, state, user_state, class, evidence_refs_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, '[\"seg-1\"]')",
        params![
            id,
            facet_type,
            key,
            value,
            confidence,
            last_seen_at,
            state,
            user_state,
            class
        ],
    )
    .expect("insert facet");
}

/// Creates `memory_tree/chunks.db` under `root`.
pub(crate) fn chunk_store(root: &Path) -> Connection {
    std::fs::create_dir_all(root.join("memory_tree/content")).expect("tree dir");
    let conn = Connection::open(root.join("memory_tree/chunks.db")).expect("chunks.db");
    conn.execute_batch(CHUNKS_DDL).expect("chunks ddl");
    conn
}

/// Inserts a chunk.
#[allow(clippy::too_many_arguments, reason = "mirrors the table's columns")]
pub(crate) fn chunk(
    conn: &Connection,
    id: &str,
    kind: &str,
    source: &str,
    seq: i64,
    timestamp_ms: i64,
    preview: &str,
    tags: &str,
    content_path: Option<&str>,
) {
    conn.execute(
        "INSERT INTO mem_tree_chunks (id, source_kind, source_id, owner, timestamp_ms,
           time_range_start_ms, time_range_end_ms, tags_json, content, token_count,
           seq_in_source, created_at_ms, content_path)
         VALUES (?1, ?2, ?3, 'me', ?4, ?4, ?4, ?5, ?6, 1, ?7, ?4, ?8)",
        params![
            id,
            kind,
            source,
            timestamp_ms,
            tags,
            preview,
            seq,
            content_path
        ],
    )
    .expect("insert chunk");
}
