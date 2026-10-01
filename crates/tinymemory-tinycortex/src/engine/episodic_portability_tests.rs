//! Unit tests for the episodic export's paging rules and conversions.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{ConversationSegment, EpisodicPart, SegmentStatus};
use tinymemory_core::store::segments::SegmentStatus as Engine;

use super::{cursor, fit, key_of, segment_to_engine, PAGE_BYTES};

fn segment(
    open: bool,
    summary: Option<&str>,
    status: Option<SegmentStatus>,
) -> ConversationSegment {
    ConversationSegment {
        segment_id: "seg-1".to_string(),
        session_id: "s1".to_string(),
        namespace: "global".to_string(),
        start_episodic_id: 1,
        end_episodic_id: None,
        start_timestamp: 5.0,
        end_timestamp: None,
        turn_count: 1,
        summary: summary.map(str::to_string),
        embedding: None,
        open,
        status,
        start_seq: None,
        end_seq: None,
    }
}

#[test]
fn a_cursor_names_its_part_and_is_refused_for_another() {
    let issued = cursor(EpisodicPart::Turns, "42");
    assert_eq!(
        key_of(EpisodicPart::Turns, Some(&issued)).expect("own cursor"),
        Some("42".to_string())
    );
    assert_eq!(key_of(EpisodicPart::Turns, None).expect("first page"), None);
    assert!(matches!(
        key_of(EpisodicPart::Segments, Some(&issued)),
        Err(MemoryError::Invalid(_))
    ));
    assert!(matches!(
        key_of(EpisodicPart::Turns, Some("turnsX")),
        Err(MemoryError::Invalid(_))
    ));
}

#[test]
fn a_page_stops_at_its_byte_budget_but_always_moves_one_record() {
    let (kept, trimmed) = fit(vec![1usize, 2, 3], |_| PAGE_BYTES / 2);
    assert_eq!(kept, vec![1, 2]);
    assert!(trimmed);

    let (kept, trimmed) = fit(vec![1usize, 2], |_| PAGE_BYTES * 3);
    assert_eq!(kept, vec![1], "an oversized record still moves, alone");
    assert!(trimmed);

    let (kept, trimmed) = fit(vec![1usize, 2, 3], |_| 10);
    assert_eq!(kept.len(), 3);
    assert!(!trimmed);
}

#[test]
fn a_segment_takes_its_status_or_the_one_its_fields_imply() {
    assert_eq!(
        segment_to_engine(segment(false, None, Some(SegmentStatus::Closed))).status,
        Engine::Closed
    );
    assert_eq!(
        segment_to_engine(segment(true, None, None)).status,
        Engine::Open
    );
    assert_eq!(
        segment_to_engine(segment(false, Some("recap"), None)).status,
        Engine::Summarised
    );
    assert_eq!(
        segment_to_engine(segment(false, None, None)).status,
        Engine::Closed
    );
}

#[test]
fn a_segment_is_dated_by_its_turns() {
    let mut grown = segment(false, None, Some(SegmentStatus::Closed));
    grown.end_timestamp = Some(9.0);
    let row = segment_to_engine(grown);
    assert_eq!(row.created_at, 5.0);
    assert_eq!(row.updated_at, 9.0);
    let row = segment_to_engine(segment(true, None, None));
    assert_eq!(row.updated_at, 5.0);
}
