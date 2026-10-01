//! Tests for paging the episodic tables out and writing records back in.

use super::*;
use crate::store::events::{EventType, EVENTS_INIT_SQL};
use crate::store::fts5::{episodic_insert, episodic_search, EPISODIC_INIT_SQL};
use crate::store::segments::{segment_create, segment_get, SegmentStatus, SEGMENTS_INIT_SQL};

/// Where fresh ids start in these tests: far above any id they import.
const FLOOR: i64 = 1_000_000;

fn setup_db() -> Arc<Mutex<Connection>> {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(EPISODIC_INIT_SQL).unwrap();
    conn.execute_batch(SEGMENTS_INIT_SQL).unwrap();
    conn.execute_batch(EVENTS_INIT_SQL).unwrap();
    Arc::new(Mutex::new(conn))
}

fn entry(id: Option<i64>, session: &str, content: &str) -> EpisodicEntry {
    EpisodicEntry {
        id,
        session_id: session.to_string(),
        timestamp: 10.0,
        role: "user".to_string(),
        content: content.to_string(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 3,
    }
}

fn segment(id: &str, start: i64) -> ConversationSegment {
    ConversationSegment {
        segment_id: id.to_string(),
        session_id: "s1".to_string(),
        namespace: "global".to_string(),
        start_episodic_id: start,
        end_episodic_id: Some(start + 1),
        start_timestamp: 10.0,
        end_timestamp: Some(11.0),
        turn_count: 2,
        summary: Some("about the trip".to_string()),
        embedding: Some(vec![0.5, 0.25]),
        topic_keywords: None,
        status: SegmentStatus::Summarised,
        created_at: 10.0,
        updated_at: 11.0,
        start_seq: Some(0),
        end_seq: Some(1),
    }
}

fn event(id: &str) -> EventRecord {
    EventRecord {
        event_id: id.to_string(),
        segment_id: "seg-1".to_string(),
        session_id: "s1".to_string(),
        namespace: "global".to_string(),
        event_type: EventType::Decision,
        content: "we fly on monday".to_string(),
        subject: None,
        timestamp_ref: Some("monday".to_string()),
        confidence: 0.8,
        embedding: None,
        source_turn_ids: Some("[1,2]".to_string()),
        created_at: 12.0,
    }
}

#[test]
fn turns_page_in_id_order_from_past_the_cursor() {
    let conn = setup_db();
    for n in 0..5 {
        episodic_insert(&conn, &entry(None, "s1", &format!("turn {n}"))).unwrap();
    }
    let first = turns_after(&conn, None, 2).unwrap();
    assert_eq!(
        first.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    let rest = turns_after(&conn, Some(2), 10).unwrap();
    assert_eq!(rest.len(), 3);
    assert_eq!(rest[0].id, Some(3));
    assert!(turns_after(&conn, Some(5), 10).unwrap().is_empty());
}

#[test]
fn a_turn_keeps_its_id_and_is_searchable() {
    let conn = setup_db();
    let outcome = import_turn(&conn, &entry(Some(42), "s1", "flights to lisbon"), FLOOR).unwrap();
    assert_eq!(outcome, TurnImport::Imported);
    let turns = turns_after(&conn, None, 10).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].id, Some(42));
    assert_eq!(turns[0].cost_microdollars, 3);
    // The full-text index follows the table, imported rows included.
    let hits = episodic_search(&conn, "lisbon", 5).unwrap();
    assert_eq!(hits.len(), 1);
    // A later recorded turn is numbered past the imported one.
    let next = episodic_insert(&conn, &entry(None, "s1", "next")).unwrap();
    assert!(next > 42);
}

#[test]
fn the_same_turn_imported_twice_is_skipped() {
    let conn = setup_db();
    let turn = entry(Some(7), "s1", "hello");
    assert_eq!(
        import_turn(&conn, &turn, FLOOR).unwrap(),
        TurnImport::Imported
    );
    assert_eq!(
        import_turn(&conn, &turn, FLOOR).unwrap(),
        TurnImport::Skipped
    );
    assert_eq!(turns_after(&conn, None, 10).unwrap().len(), 1);
}

#[test]
fn a_turn_meeting_another_under_its_id_gets_a_new_one() {
    let conn = setup_db();
    let held = episodic_insert(&conn, &entry(None, "s1", "already here")).unwrap();
    let moved = entry(Some(held), "s2", "a different turn");
    let outcome = import_turn(&conn, &moved, FLOOR).unwrap();
    assert_eq!(
        outcome,
        TurnImport::Remapped(FLOOR),
        "a fresh id comes from above the floor"
    );
    let turns = turns_after(&conn, None, 10).unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].content, "already here");
    assert_eq!(turns[1].id, Some(FLOOR));
    assert_eq!(turns[1].session_id, "s2");

    // Run again, the moved turn is found where the first run put it.
    assert_eq!(
        import_turn(&conn, &moved, FLOOR).unwrap(),
        TurnImport::Present(FLOOR)
    );
    assert_eq!(turns_after(&conn, None, 10).unwrap().len(), 2);

    // A turn still to come keeps its own id: the fresh one did not take it.
    assert_eq!(
        import_turn(&conn, &entry(Some(held + 1), "s2", "next"), FLOOR).unwrap(),
        TurnImport::Imported
    );
}

#[test]
fn a_fresh_id_clears_a_table_already_above_the_floor() {
    let conn = setup_db();
    import_turn(&conn, &entry(Some(FLOOR + 5), "s1", "high"), FLOOR).unwrap();
    import_turn(&conn, &entry(Some(1), "s1", "low"), FLOOR).unwrap();
    let outcome = import_turn(&conn, &entry(Some(1), "s9", "clash"), FLOOR).unwrap();
    assert_eq!(outcome, TurnImport::Remapped(FLOOR + 6));
}

#[test]
fn an_imported_turn_is_sanitized_like_a_recorded_one() {
    let conn = setup_db();
    let secret = entry(
        Some(1),
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
        "x",
    );
    assert!(import_turn(&conn, &secret, FLOOR).is_err());
    assert!(import_turn(&conn, &entry(None, "s1", "no id"), FLOOR).is_err());
    assert!(turns_after(&conn, None, 10).unwrap().is_empty());
}

#[test]
fn a_segment_is_written_whole_and_read_back_in_id_order() {
    let conn = setup_db();
    assert!(import_segment(&conn, &segment("seg-b", 3)).unwrap());
    assert!(import_segment(&conn, &segment("seg-a", 1)).unwrap());
    let stored = segment_get(&conn, "seg-b").unwrap().unwrap();
    assert_eq!(stored.turn_count, 2);
    assert_eq!(stored.status, SegmentStatus::Summarised);
    assert_eq!(stored.summary.as_deref(), Some("about the trip"));
    assert_eq!(stored.embedding.as_deref(), Some([0.5, 0.25].as_slice()));
    assert_eq!(stored.end_seq, Some(1));

    let page = segments_after(&conn, None, 1).unwrap();
    assert_eq!(page[0].segment_id, "seg-a");
    let rest = segments_after(&conn, Some("seg-a"), 10).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].segment_id, "seg-b");
}

#[test]
fn a_segment_replaces_its_namesake_and_an_identical_one_is_skipped() {
    let conn = setup_db();
    segment_create(&conn, "seg-1", "s1", "global", 1, None, 10.0, 10.0).unwrap();
    {
        let conn = conn.lock();
        conn.execute(
            "UPDATE conversation_segments SET topic_keywords = 'trip' WHERE segment_id = 'seg-1'",
            [],
        )
        .unwrap();
    }
    let copy = segment("seg-1", 1);
    assert!(import_segment(&conn, &copy).unwrap());
    let stored = segment_get(&conn, "seg-1").unwrap().unwrap();
    assert_eq!(stored.turn_count, 2);
    assert_eq!(stored.status, SegmentStatus::Summarised);
    // What a copy does not carry stays as it was.
    assert_eq!(stored.topic_keywords.as_deref(), Some("trip"));
    assert!(!import_segment(&conn, &copy).unwrap());
}

#[test]
fn events_page_by_id_and_an_identical_one_is_skipped() {
    let conn = setup_db();
    assert!(import_event(&conn, &event("ev-2")).unwrap());
    assert!(import_event(&conn, &event("ev-1")).unwrap());
    assert!(!import_event(&conn, &event("ev-1")).unwrap());
    let mut changed = event("ev-1");
    changed.source_turn_ids = Some("[9]".to_string());
    assert!(import_event(&conn, &changed).unwrap());

    let page = events_after(&conn, None, 1).unwrap();
    assert_eq!(page[0].event_id, "ev-1");
    assert_eq!(page[0].source_turn_ids.as_deref(), Some("[9]"));
    let rest = events_after(&conn, Some("ev-1"), 10).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].event_id, "ev-2");
}

#[test]
fn segment_embeddings_page_by_segment_then_signature() {
    let conn = setup_db();
    // An embedding belongs to a segment the store holds; the table says so.
    import_segment(&conn, &segment("seg-1", 1)).unwrap();
    import_segment(&conn, &segment("seg-2", 3)).unwrap();
    for (segment_id, signature) in [("seg-2", "a"), ("seg-1", "b"), ("seg-1", "a")] {
        let stored = StoredSegmentEmbedding {
            segment_id: segment_id.to_string(),
            model_signature: signature.to_string(),
            vector: vec![1.0, 2.0],
            created_at: 5.0,
        };
        assert!(import_segment_embedding(&conn, &stored).unwrap());
        assert!(!import_segment_embedding(&conn, &stored).unwrap());
    }
    let first = segment_embeddings_after(&conn, None, 2).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|e| (e.segment_id.as_str(), e.model_signature.as_str()))
            .collect::<Vec<_>>(),
        vec![("seg-1", "a"), ("seg-1", "b")]
    );
    let rest = segment_embeddings_after(&conn, Some(("seg-1", "b")), 10).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].segment_id, "seg-2");
    assert_eq!(rest[0].vector, vec![1.0, 2.0]);
}

#[test]
fn an_embedding_for_a_segment_the_store_lacks_is_refused() {
    let conn = setup_db();
    let orphan = StoredSegmentEmbedding {
        segment_id: "missing".to_string(),
        model_signature: "a".to_string(),
        vector: vec![1.0],
        created_at: 1.0,
    };
    assert!(import_segment_embedding(&conn, &orphan).is_err());
}
