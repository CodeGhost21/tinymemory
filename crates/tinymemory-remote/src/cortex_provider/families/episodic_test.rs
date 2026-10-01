//! Episodic memory over the hosted wire: turns and segments as session-labelled
//! bookkeeping, read one session at a time.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{
    EpisodicEvent, EpisodicTurn, EventKind, MemoryCore, MemoryEpisodic, SegmentStatus,
};

use super::*;
use crate::cortex_labels;
use crate::cortex_provider::families::test_support::{hosted, requests};

fn turn(session: &str, role: &str, content: &str, timestamp: f64) -> EpisodicTurn {
    EpisodicTurn {
        id: None,
        session_id: session.to_string(),
        timestamp,
        role: role.to_string(),
        content: content.to_string(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: -5,
    }
}

#[test]
fn turn_ids_rise_even_within_one_microsecond() {
    let ids: Vec<i64> = (0..1000).map(|_| next_turn_id()).collect();
    assert!(ids.windows(2).all(|pair| pair[1] > pair[0]));
}

#[tokio::test]
async fn turns_get_rising_ids_and_read_back_oldest_first() {
    let (provider, _state) = hosted().await;
    let first = provider
        .insert_turn(&turn("session-1", "user", "hello", 100.0))
        .await
        .expect("turn");
    let second = provider
        .insert_turn(&turn("session-1", "assistant", "hi there", 100.001))
        .await
        .expect("turn");
    provider
        .insert_turn(&turn("session-2", "user", "elsewhere", 50.0))
        .await
        .expect("other session");
    assert!(second > first, "{first} then {second}");
    let turns = provider.session_turns("session-1").await.expect("turns");
    assert_eq!(
        turns
            .iter()
            .map(|t| (t.id, t.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(Some(first), "user"), (Some(second), "assistant")]
    );
    assert!(
        turns.iter().all(|t| t.cost_microdollars == 0),
        "negative cost is clamped"
    );
    assert!(provider
        .session_turns("session-3")
        .await
        .expect("none")
        .is_empty());
    for refused in [
        provider.insert_turn(&turn(" ", "user", "x", 1.0)).await,
        provider.insert_turn(&turn("session-1", "", "x", 1.0)).await,
    ] {
        assert!(
            matches!(refused, Err(MemoryError::Invalid(_))),
            "{refused:?}"
        );
    }
}

/// The calls a host makes on every turn read one session's records by label,
/// never the whole history.
#[tokio::test]
async fn a_sessions_reads_go_by_its_label() {
    let (provider, state) = hosted().await;
    provider
        .insert_turn(&turn("session-1", "user", "hello", 1.0))
        .await
        .expect("turn");
    provider
        .create_segment("seg-1", "session-1", "global", 1, Some(1), 1.0, 1.0)
        .await
        .expect("create");
    let before = requests(&state).len();
    provider.session_turns("session-1").await.expect("turns");
    provider.open_segment("session-1").await.expect("open");
    let label = cortex_labels::session("session-1");
    let reads: Vec<String> = requests(&state)[before..]
        .iter()
        .filter(|request| request.starts_with("GET /memory/events?"))
        .cloned()
        .collect();
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert!(
        reads.iter().all(
            |read| read.contains(&format!("labels={}", label.replace(':', "%3A")))
                || read.contains(&format!("labels={label}"))
        ),
        "{reads:?}"
    );
}

#[tokio::test]
async fn a_segment_opens_grows_closes_and_is_summarised() {
    let (provider, _state) = hosted().await;
    assert!(provider
        .open_segment("session-1")
        .await
        .expect("none yet")
        .is_none());
    provider
        .create_segment("seg-1", "session-1", "global", 1, Some(1), 100.0, 100.0)
        .await
        .expect("create");
    provider
        .append_turn("seg-1", 2, Some(2), 101.0, 101.0)
        .await
        .expect("append");
    let open = provider
        .open_segment("session-1")
        .await
        .expect("open")
        .expect("the open segment");
    assert_eq!(open.segment_id, "seg-1");
    assert_eq!(open.turn_count, 2);
    assert_eq!((open.start_seq, open.end_seq), (Some(1), Some(2)));
    assert_eq!(open.end_episodic_id, Some(2));
    assert_eq!(open.end_timestamp, Some(101.0));
    assert!(open.open);
    assert!(provider
        .open_segment("session-2")
        .await
        .expect("other")
        .is_none());

    provider.close_segment("seg-1", 102.0).await.expect("close");
    provider
        .close_segment("seg-1", 103.0)
        .await
        .expect("close is idempotent");
    assert!(provider
        .open_segment("session-1")
        .await
        .expect("closed")
        .is_none());
    provider
        .set_segment_summary("seg-1", "talked about lifetimes", 104.0)
        .await
        .expect("summary");
    let summarised = provider
        .segment("seg-1")
        .await
        .expect("read")
        .expect("present")
        .segment;
    assert_eq!(summarised.status, Some(SegmentStatus::Summarised));
    assert_eq!(
        summarised.summary.as_deref(),
        Some("talked about lifetimes")
    );
    provider
        .upsert_segment_embedding("seg-1", "cloud", &[0.1, 0.2], 105.0)
        .await
        .expect("embedding");
    provider
        .append_turn("seg-unknown", 9, None, 1.0, 1.0)
        .await
        .expect("an unknown segment is left alone");
    assert!(provider
        .segment("seg-unknown")
        .await
        .expect("read")
        .is_none());

    provider
        .create_segment("seg-2", "session-1", "global", 3, Some(3), 110.0, 110.0)
        .await
        .expect("create");
    let open = provider
        .open_segment("session-1")
        .await
        .expect("open")
        .expect("the new segment");
    assert_eq!(open.segment_id, "seg-2");
    assert_eq!(open.status, Some(SegmentStatus::Open));
    assert!(provider
        .segments_pending_summary(10)
        .await
        .expect("pending")
        .is_empty());
}

#[tokio::test]
async fn the_newest_of_two_open_segments_is_the_open_one() {
    let (provider, _state) = hosted().await;
    provider
        .create_segment("seg-late", "session-1", "global", 5, None, 50.0, 50.0)
        .await
        .expect("create");
    provider
        .create_segment("seg-early", "session-1", "global", 1, None, 10.0, 10.0)
        .await
        .expect("create");
    let open = provider
        .open_segment("session-1")
        .await
        .expect("open")
        .expect("one is open");
    assert_eq!(
        open.segment_id, "seg-late",
        "created later, written earlier"
    );
}

#[tokio::test]
async fn a_segment_needs_an_id_and_a_session() {
    let (provider, _state) = hosted().await;
    for (segment, session) in [(" ", "session-1"), ("seg-1", "")] {
        let refused = provider
            .create_segment(segment, session, "global", 1, None, 1.0, 1.0)
            .await;
        assert!(
            matches!(refused, Err(MemoryError::Invalid(_))),
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn an_episodic_event_is_replaced_by_its_id() {
    let (provider, _state) = hosted().await;
    let mut event = EpisodicEvent {
        event_id: "ev-1".to_string(),
        segment_id: "seg-1".to_string(),
        session_id: "session-1".to_string(),
        namespace: "global".to_string(),
        kind: EventKind::Decision,
        content: "chose hosted memory".to_string(),
        subject: None,
        timestamp_ref: None,
        confidence: 0.6,
        embedding: None,
        source_turn_ids: None,
        created_at: 1.0,
    };
    provider.insert_event(&event).await.expect("insert");
    event.content = "chose hosted memory, definitely".to_string();
    provider.insert_event(&event).await.expect("replace");
    let place = provider.episodic_place(EPISODIC_EVENTS).expect("place");
    let listed = Records::new(&provider.dialect)
        .live_all(&place)
        .await
        .expect("events");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].record.content.contains("definitely"));
    event.event_id = "  ".to_string();
    let refused = provider.insert_event(&event).await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn episodic_records_are_bookkeeping_not_memory() {
    let (provider, _state) = hosted().await;
    provider
        .insert_turn(&turn("session-1", "user", "hello", 1.0))
        .await
        .expect("turn");
    provider
        .create_segment("seg-1", "session-1", "global", 1, None, 1.0, 1.0)
        .await
        .expect("create");
    assert!(provider.namespaces().await.expect("namespaces").is_empty());
}

/// Makes the next turn id at least one past `id`, so a test elsewhere in the
/// families can know what [`next_turn_id`] hands out next.
pub(in crate::cortex_provider::families) fn turn_ids_continue_after(id: i64) {
    LAST_TURN_ID.fetch_max(id, std::sync::atomic::Ordering::SeqCst);
}
