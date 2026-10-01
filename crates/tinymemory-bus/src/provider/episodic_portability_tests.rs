//! Wire shape of the episodic-portability family.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::{
    EpisodicExportPage, EpisodicImportOutcome, EpisodicPart, EpisodicRecords, SegmentEmbedding,
    TurnIdRemap,
};
use crate::provider::episodic::EpisodicTurn;

fn turn(id: i64) -> EpisodicTurn {
    EpisodicTurn {
        id: Some(id),
        session_id: "sess-1".to_string(),
        timestamp: 1.5,
        role: "user".to_string(),
        content: "hello".to_string(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 0,
    }
}

/// Every part's wire string is the serde form, so a page and a request name a
/// part the same way.
#[test]
fn each_part_serialises_as_its_wire_string() {
    for part in EpisodicPart::ALL {
        let json = serde_json::to_string(&part).expect("serialises");
        assert_eq!(json, format!("\"{}\"", part.as_str()));
        let back: EpisodicPart = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, part);
    }
}

/// Turns come first: segments and events name turns by id, so a copy that
/// imported them before the turns could not rewrite a remapped id.
#[test]
fn turns_are_imported_first() {
    assert_eq!(EpisodicPart::ALL[0], EpisodicPart::Turns);
}

/// Records carry their part as a tag beside them, and decode back to it.
#[test]
fn records_are_tagged_with_their_part() {
    let records = EpisodicRecords::Turns(vec![turn(7)]);
    let json = serde_json::to_value(&records).expect("serialises");
    assert_eq!(json["part"], "turns");
    assert_eq!(json["records"][0]["id"], 7);
    let back: EpisodicRecords = serde_json::from_value(json).expect("deserialises");
    assert_eq!(back, records);
    assert_eq!(back.part(), EpisodicPart::Turns);
    assert_eq!(back.len(), 1);
    assert!(!back.is_empty());
}

/// An empty set of each part reports that part and no records.
#[test]
fn empty_records_keep_their_part() {
    for part in EpisodicPart::ALL {
        let records = EpisodicRecords::empty(part);
        assert_eq!(records.part(), part);
        assert!(records.is_empty());
    }
}

/// A page round-trips, and a last page decodes without a cursor field.
#[test]
fn a_page_round_trips_and_the_cursor_is_optional() {
    let page = EpisodicExportPage {
        records: EpisodicRecords::SegmentEmbeddings(vec![SegmentEmbedding {
            segment_id: "seg-1".to_string(),
            model_signature: "sig".to_string(),
            embedding: vec![0.5, 0.25],
            created_at: 3.0,
        }]),
        next_cursor: Some("c".to_string()),
    };
    let json = serde_json::to_string(&page).expect("serialises");
    let back: EpisodicExportPage = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(back, page);

    let last: EpisodicExportPage =
        serde_json::from_str(r#"{"records":{"part":"events","records":[]}}"#)
            .expect("decodes without a cursor");
    assert_eq!(last.next_cursor, None);
    assert_eq!(last.records.part(), EpisodicPart::Events);
}

/// An outcome without errors or remaps decodes, so a driver that never
/// remaps need not send the field.
#[test]
fn an_outcome_decodes_without_the_optional_lists() {
    let outcome: EpisodicImportOutcome =
        serde_json::from_str(r#"{"imported":2,"skipped":1,"failed":0}"#).expect("decodes");
    assert_eq!(outcome.imported, 2);
    assert!(outcome.errors.is_empty());
    assert!(outcome.remapped.is_empty());

    let remapped = EpisodicImportOutcome {
        remapped: vec![TurnIdRemap { from: 1, to: 9 }],
        ..EpisodicImportOutcome::default()
    };
    let json = serde_json::to_string(&remapped).expect("serialises");
    let back: EpisodicImportOutcome = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(back, remapped);
}
