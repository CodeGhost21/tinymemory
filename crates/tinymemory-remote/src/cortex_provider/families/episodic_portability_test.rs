//! The episodic record moving between hosted accounts: every part pages out
//! in key order and back in, ids survive, and a second pass writes nothing.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{
    EpisodicEvent, EpisodicExportPage, EpisodicPart, EpisodicRecords, EpisodicTurn, EventKind,
    MemoryEpisodic, MemoryEpisodicPortability, SegmentStatus,
};

use super::*;
use crate::cortex_provider::families::test_support::hosted;

fn turn(id: Option<i64>, session: &str, content: &str, timestamp: f64) -> EpisodicTurn {
    EpisodicTurn {
        id,
        session_id: session.to_string(),
        timestamp,
        role: "user".to_string(),
        content: content.to_string(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 0,
    }
}

/// Every page of `part`, walked to the end with pages of `limit`.
async fn walk(
    provider: &CortexProvider,
    part: EpisodicPart,
    limit: usize,
) -> Vec<EpisodicExportPage> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = provider
            .export_episodic(part, cursor.as_deref(), limit)
            .await
            .expect("export page");
        assert_eq!(page.records.part(), part);
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
    }
}

/// A source account holding two turns, a summarised segment over them, an
/// event and an embedding.
async fn source() -> (CortexProvider, Vec<i64>) {
    let (provider, _state) = hosted().await;
    let mut ids = Vec::new();
    for (n, content) in ["plan the trip", "book flights"].iter().enumerate() {
        ids.push(
            provider
                .insert_turn(&turn(None, "session-1", content, 10.0 + n as f64))
                .await
                .expect("turn"),
        );
    }
    provider
        .create_segment("seg-1", "session-1", "global", ids[0], Some(0), 10.0, 10.0)
        .await
        .expect("segment");
    provider
        .append_turn("seg-1", ids[1], Some(1), 11.0, 11.0)
        .await
        .expect("append");
    provider
        .set_segment_summary("seg-1", "planning a trip", 12.0)
        .await
        .expect("summary");
    provider
        .insert_event(&EpisodicEvent {
            event_id: "ev-1".into(),
            segment_id: "seg-1".into(),
            session_id: "session-1".into(),
            namespace: "global".into(),
            kind: EventKind::Decision,
            content: "flights get booked".into(),
            subject: None,
            timestamp_ref: None,
            confidence: 0.9,
            embedding: None,
            source_turn_ids: Some(format!("[{},{}]", ids[0], ids[1])),
            created_at: 12.0,
        })
        .await
        .expect("event");
    provider
        .upsert_segment_embedding("seg-1", "sig-a", &[0.5, 0.25], 12.0)
        .await
        .expect("embedding");
    (provider, ids)
}

#[tokio::test]
async fn the_record_moves_whole_and_a_second_pass_writes_nothing() {
    let (from, ids) = source().await;
    let (to, _state) = hosted().await;
    for pass in 0..2 {
        for part in EpisodicPart::ALL {
            for page in walk(&from, part, 1).await {
                if page.records.is_empty() {
                    continue;
                }
                let outcome = to.import_episodic(page.records).await.expect("import");
                assert_eq!(outcome.failed, 0, "{:?}", outcome.errors);
                assert!(outcome.remapped.is_empty(), "{:?}", outcome.remapped);
                if pass == 1 {
                    assert_eq!(outcome.imported, 0, "the second pass wrote {part}");
                }
            }
        }
    }

    let turns = to.session_turns("session-1").await.expect("turns");
    assert_eq!(
        turns.iter().map(|t| t.id).collect::<Vec<_>>(),
        ids.iter().copied().map(Some).collect::<Vec<_>>(),
        "a turn keeps its id"
    );
    for part in EpisodicPart::ALL {
        let mut exported: Vec<EpisodicRecords> = walk(&from, part, 10)
            .await
            .into_iter()
            .map(|page| page.records)
            .collect();
        let copied: Vec<EpisodicRecords> = walk(&to, part, 10)
            .await
            .into_iter()
            .map(|page| page.records)
            .collect();
        exported.retain(|records| !records.is_empty());
        assert_eq!(
            copied
                .into_iter()
                .filter(|r| !r.is_empty())
                .collect::<Vec<_>>(),
            exported,
            "{part} differs after the copy"
        );
    }
    let EpisodicRecords::Segments(segments) = walk(&to, EpisodicPart::Segments, 10)
        .await
        .remove(0)
        .records
    else {
        panic!("asked for segments");
    };
    assert_eq!(segments[0].status, Some(SegmentStatus::Summarised));
    assert_eq!(segments[0].turn_count, 2);
}

#[tokio::test]
async fn pages_resume_past_their_cursor_in_id_order() {
    let (from, ids) = source().await;
    let pages = walk(&from, EpisodicPart::Turns, 1).await;
    let walked: Vec<Option<i64>> = pages
        .iter()
        .flat_map(|page| match &page.records {
            EpisodicRecords::Turns(turns) => turns.iter().map(|t| t.id).collect::<Vec<_>>(),
            _ => panic!("asked for turns"),
        })
        .collect();
    assert_eq!(walked, ids.into_iter().map(Some).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_turn_meeting_another_under_its_id_moves_once() {
    let (to, _state) = hosted().await;
    // The target already holds a different turn under id 7.
    let outcome = to
        .import_episodic(EpisodicRecords::Turns(vec![turn(
            Some(7),
            "session-0",
            "an older conversation",
            1.0,
        )]))
        .await
        .expect("seed");
    assert_eq!(outcome.imported, 1);

    let incoming = EpisodicRecords::Turns(vec![turn(Some(7), "session-1", "hello", 5.0)]);
    let first = to.import_episodic(incoming.clone()).await.expect("import");
    assert_eq!(first.imported, 1);
    assert_eq!(first.remapped.len(), 1);
    let moved = first.remapped[0];
    assert_eq!(moved.from, 7);
    assert!(
        moved.to > 7,
        "a fresh id comes from above every recorded turn"
    );

    let again = to.import_episodic(incoming).await.expect("again");
    assert_eq!((again.imported, again.skipped), (0, 1));
    assert_eq!(
        again.remapped,
        vec![moved],
        "found where the first pass put it"
    );

    let turns = to.session_turns("session-1").await.expect("turns");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].id, Some(moved.to));
    let older = to.session_turns("session-0").await.expect("turns");
    assert_eq!(older[0].content, "an older conversation", "left as it was");
}

#[tokio::test]
async fn refusals_name_the_record_and_bad_cursors_are_invalid() {
    let (provider, _state) = hosted().await;
    let outcome = provider
        .import_episodic(EpisodicRecords::Turns(vec![
            turn(None, "session-1", "no id", 1.0),
            turn(Some(3), " ", "no session", 1.0),
            turn(Some(4), "session-1", "fine", 1.0),
        ]))
        .await
        .expect("import");
    assert_eq!((outcome.imported, outcome.failed), (1, 2));
    assert!(outcome.errors.iter().all(|e| !e.contains("no session")));

    for (part, cursor) in [
        (EpisodicPart::Segments, "turns:4"),
        (EpisodicPart::Turns, "turns:x"),
        (EpisodicPart::SegmentEmbeddings, "segment_embeddings:[1]"),
    ] {
        let result = provider.export_episodic(part, Some(cursor), 10).await;
        assert!(matches!(result, Err(MemoryError::Invalid(_))), "{cursor}");
    }
    assert!(matches!(
        provider.export_episodic(EpisodicPart::Turns, None, 0).await,
        Err(MemoryError::Invalid(_))
    ));
}

#[test]
fn a_page_stops_at_its_limit_or_its_byte_budget() {
    let ordered = |n: usize| (0..n).map(|i| (i.to_string(), i)).collect::<Vec<_>>();
    let (taken, next) = page(ordered(3), 2, |_| 1);
    assert_eq!((taken, next.as_deref()), (vec![0, 1], Some("1")));
    let (taken, next) = page(ordered(2), 2, |_| 1);
    assert_eq!((taken, next), (vec![0, 1], None));
    let (taken, next) = page(ordered(3), 10, |_| PAGE_BYTES);
    assert_eq!((taken, next.as_deref()), (vec![0], Some("0")));
    let (taken, next) = page(Vec::<(String, usize)>::new(), 10, |_| 1);
    assert!(taken.is_empty() && next.is_none());
}
