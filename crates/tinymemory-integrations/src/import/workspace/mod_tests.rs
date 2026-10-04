//! Tests for schema probing.

use super::*;

fn memory(sql: &str) -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(sql).expect("schema");
    conn
}

const MINIMAL: &str = "
    CREATE TABLE memory_docs (document_id TEXT, namespace TEXT, title TEXT, content TEXT,
        tags_json TEXT, metadata_json TEXT, updated_at REAL);
    CREATE TABLE episodic_log (id INTEGER, session_id TEXT, timestamp REAL, role TEXT,
        content TEXT);
    CREATE TABLE user_profile (facet_id TEXT, facet_type TEXT, key TEXT, value TEXT,
        confidence REAL, last_seen_at REAL);
";

#[test]
fn a_minimal_store_has_no_optional_columns() {
    let schema = MemorySchema::probe(&memory(MINIMAL))
        .expect("probes")
        .expect("is legacy");
    assert_eq!(schema, MemorySchema::default());
}

#[test]
fn detects_optional_columns() {
    let conn = memory(MINIMAL);
    conn.execute_batch(
        "ALTER TABLE memory_docs ADD COLUMN logical_namespace TEXT;
         ALTER TABLE episodic_log ADD COLUMN tool_calls_json TEXT;
         ALTER TABLE user_profile ADD COLUMN state TEXT;
         ALTER TABLE user_profile ADD COLUMN user_state TEXT;
         ALTER TABLE user_profile ADD COLUMN class TEXT;
         ALTER TABLE user_profile ADD COLUMN evidence_refs_json TEXT;",
    )
    .expect("alter");
    let schema = MemorySchema::probe(&conn).expect("probes").expect("legacy");
    assert!(schema.logical_namespace);
    assert!(schema.tool_calls_json);
    assert!(schema.profile_state);
    assert!(schema.profile_user_state);
    assert!(schema.profile_class);
    assert!(schema.profile_evidence);
}

#[test]
fn names_a_missing_table() {
    let reason = MemorySchema::probe(&memory("CREATE TABLE memory_docs (x TEXT);"))
        .expect("probes")
        .expect_err("not legacy");
    assert!(reason.contains("episodic_log"), "{reason}");
}

#[test]
fn names_a_missing_column() {
    let conn = memory(&MINIMAL.replace("value TEXT,", ""));
    let reason = MemorySchema::probe(&conn)
        .expect("probes")
        .expect_err("not legacy");
    assert_eq!(reason, "column user_profile.value is missing");
}
