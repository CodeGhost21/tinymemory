//! Consolidation requests and receipts.

use super::*;
use crate::namespace::Namespace;

#[test]
fn a_beliefs_request_needs_a_limit_and_a_real_query() {
    let request = BeliefsRequest::new(Reach::subtree(Namespace::ROOT), 5);
    request.validate().unwrap();
    request.clone().query("refunds").validate().unwrap();
    assert!(matches!(
        BeliefsRequest::new(Reach::default(), 0).validate(),
        Err(Error::InvalidRequest(_))
    ));
    assert!(matches!(
        request.query("  ").validate(),
        Err(Error::InvalidRequest(_))
    ));
}

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
