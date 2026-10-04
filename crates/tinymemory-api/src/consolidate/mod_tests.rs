//! Consolidation requests and receipts.

use super::*;
use crate::namespace::Namespace;

#[test]
fn an_unnamed_kind_list_admits_every_kind() {
    let request = ConsolidateRequest::new(Reach::subtree(Namespace::ROOT));
    assert_eq!(request.admitted_kinds(), ItemKind::ALL.to_vec());
    let narrowed = request.kinds([ItemKind::Conversation]);
    assert_eq!(narrowed.admitted_kinds(), vec![ItemKind::Conversation]);
    narrowed.validate().unwrap();
}

#[test]
fn rejects_a_kind_named_twice() {
    let request =
        ConsolidateRequest::new(Reach::default()).kinds([ItemKind::Document, ItemKind::Document]);
    assert!(matches!(request.validate(), Err(Error::InvalidRequest(_))));
}

#[test]
fn round_trips_through_json() {
    let request = ConsolidateRequest::new(Reach::subtree(Namespace::source("pdf")))
        .kinds([ItemKind::Document]);
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["reach"]["at"], "source:pdf");
    assert_eq!(json["kinds"], serde_json::json!(["document"]));
    let back: ConsolidateRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, request);

    let receipt = ConsolidateReceipt::scheduled();
    assert_eq!(
        serde_json::to_value(&receipt).unwrap(),
        serde_json::json!({ "status": "scheduled", "scopes": 0 })
    );
    assert_eq!(Consolidation::default(), Consolidation::None);
}
