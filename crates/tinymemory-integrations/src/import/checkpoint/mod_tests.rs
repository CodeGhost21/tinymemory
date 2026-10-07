//! Checkpoint encoding tests.

use super::*;
use crate::import::error::Error;

#[test]
fn a_default_checkpoint_is_the_start() {
    assert!(Checkpoint::default().is_start());
    let moved = Checkpoint {
        profile: Some("f1".into()),
        ..Checkpoint::default()
    };
    assert!(!moved.is_start());
}

#[test]
fn round_trips_through_json() {
    let checkpoint = Checkpoint {
        documents: Some("doc-9".into()),
        chunks: Some(ChunkCursor {
            source_kind: "chat".into(),
            source_id: "slack:general".into(),
        }),
        conversations: Some("thread-2".into()),
        learnings: None,
        profile: Some("facet-1".into()),
        events: Some("evt-3".into()),
        lessons: Some(42),
        graph_global: Some(7),
        graph_namespace: None,
        files: Some("goals".into()),
    };
    let json = checkpoint.to_json().expect("encodes");
    assert_eq!(Checkpoint::from_json(&json).expect("decodes"), checkpoint);
}

#[test]
fn an_empty_object_decodes_to_the_start() {
    assert!(Checkpoint::from_json("{}").expect("decodes").is_start());
}

#[test]
fn rejects_json_that_is_not_a_checkpoint() {
    let err = Checkpoint::from_json("[1, 2]").expect_err("not a checkpoint");
    assert!(matches!(err, Error::Json(_)), "{err:?}");
}
