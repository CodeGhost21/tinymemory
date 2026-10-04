//! Belief builds: one `v1/beliefs/build` per held scope on Direct, none on
//! the hosted wire.

use super::*;
use crate::cortex::testing::{both, direct_double, direct_engine, sample_items};
use tinymemory_api::{
    ConsolidateRequest, ItemKind, MemoryEngine, MemoryMeta, Namespace, Reach, StoreItem,
};

fn at(namespace: Namespace) -> MemoryMeta {
    MemoryMeta {
        namespace,
        ..MemoryMeta::default()
    }
}

#[test]
fn reads_the_job_handle_by_any_known_field() {
    assert_eq!(job_id(&json!({ "job_id": "j1" })).as_deref(), Some("j1"));
    assert_eq!(job_id(&json!({ "build_id": 7 })).as_deref(), Some("7"));
    assert_eq!(
        job_id(&json!({ "id": "x", "build_id": "b" })).as_deref(),
        Some("b")
    );
    assert_eq!(job_id(&json!({ "job_id": "" })), None);
    assert_eq!(job_id(&json!({ "status": "queued" })), None);
}

#[tokio::test]
async fn builds_every_held_scope_in_reach_and_nothing_else() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    for item in [
        StoreItem::document("Refunds take five days.", at(Namespace::source("pdf"))),
        StoreItem::document("Deploys run on Fridays.", at(Namespace::source("notion"))),
        StoreItem::document("Agent scratch note.", at(Namespace::agent("support"))),
    ] {
        engine.store(item).await.unwrap();
    }

    let receipt = engine
        .consolidate(ConsolidateRequest::new(Reach::exact(Namespace::source(
            "pdf",
        ))))
        .await
        .unwrap();
    assert_eq!(receipt.status, ConsolidateStatus::Started);
    assert_eq!(receipt.scopes, 1, "only the pdf documents scope is held");
    assert_eq!(receipt.jobs, ["build-1"]);
    let builds = state.seen.lock().unwrap().builds.clone();
    assert_eq!(
        builds,
        [json!({ "scope": "app:tinymemory/source:pdf/app:documents" })]
    );

    let whole = engine
        .consolidate(
            ConsolidateRequest::new(Reach::subtree(Namespace::ROOT)).kinds([ItemKind::Document]),
        )
        .await
        .unwrap();
    assert_eq!(whole.scopes, 3, "every document scope below the root");
    assert_eq!(state.seen.lock().unwrap().builds.len(), 4);
}

#[tokio::test]
async fn an_empty_reach_builds_nothing() {
    let (endpoint, state) = direct_double().await;
    let receipt = direct_engine(&endpoint)
        .consolidate(ConsolidateRequest::new(Reach::exact(Namespace::agent(
            "nobody",
        ))))
        .await
        .unwrap();
    assert_eq!(receipt.scopes, 0);
    assert!(receipt.jobs.is_empty());
    assert!(state.seen.lock().unwrap().builds.is_empty());
}

#[tokio::test]
async fn the_hosted_wire_acknowledges_a_schedule_without_a_request() {
    for (engine, state) in both().await {
        for item in sample_items() {
            engine.store(item).await.unwrap();
        }
        let before = state.requests().len();
        let receipt = engine
            .consolidate(ConsolidateRequest::new(Reach::subtree(Namespace::ROOT)))
            .await
            .unwrap();
        match engine.wire() {
            CortexWire::TinyHumans => {
                assert_eq!(receipt, ConsolidateReceipt::scheduled());
                assert_eq!(state.requests().len(), before, "no request is sent");
            }
            CortexWire::Direct => {
                assert_eq!(receipt.status, ConsolidateStatus::Started);
                assert!(state.count("POST /v1/beliefs/build") > 0);
            }
        }
    }
}

#[tokio::test]
async fn a_malformed_request_is_refused_before_any_request() {
    let (endpoint, state) = direct_double().await;
    let error = direct_engine(&endpoint)
        .consolidate(
            ConsolidateRequest::new(Reach::default())
                .kinds([ItemKind::Learning, ItemKind::Learning]),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, crate::cortex::Error::InvalidRequest(_)),
        "{error:?}"
    );
    assert!(state.requests().is_empty());
}
