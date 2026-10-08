//! Erase: deepest scope first, so every event is deleted and every write
//! key released; kinds narrow it. Hosted, the whole tree is one `DELETE
//! /memory` and anything narrower goes scope by scope through the backend's
//! `memory/v1/erasures` passthrough (memory-api's synchronous scoped
//! erasure, retried while incomplete).

use tinymemory_api::{
    EraseRequest, ItemKind, LearningKind, ListRequest, MemoryMeta, MetaFilter, Namespace, Reach,
    StoreItem,
};

use std::sync::atomic::Ordering;

use super::*;
use crate::cortex::testing::{direct_double, direct_engine, hosted_double, hosted_engine};

fn learning(text: &str, namespace: &Namespace) -> StoreItem {
    StoreItem::learning(
        text,
        LearningKind::Fact,
        0.9,
        MemoryMeta {
            namespace: namespace.clone(),
            ..MemoryMeta::default()
        },
    )
}

fn node(path: &str) -> Namespace {
    path.parse().unwrap()
}

async fn listed(engine: &CortexEngine) -> Vec<String> {
    engine
        .list(ListRequest::new(MetaFilter::default(), 100))
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|hit| hit.text)
        .collect()
}

#[tokio::test]
async fn a_subtree_is_erased_deepest_first_and_its_items_store_anew() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let (flow, child, kept) = (
        node("ws:main/agent:flow"),
        node("ws:main/agent:flow/agent:step"),
        node("ws:main/agent:chat"),
    );
    let items = [
        learning("flow fact", &flow),
        learning("step fact", &child),
        learning("chat fact", &kept),
    ];
    for item in &items {
        engine.store(item.clone()).await.unwrap();
    }

    let report = engine
        .erase(EraseRequest::new(Reach::subtree(flow.clone())))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 2, "{report:?}");
    assert_eq!(report.receipts.len(), 2);
    let erased: Vec<String> = state
        .seen
        .lock()
        .unwrap()
        .erasures
        .iter()
        .map(|body| body["scope"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        erased,
        vec![
            "app:tinymemory/ws:main/agent:flow/agent:step/app:learnings".to_string(),
            "app:tinymemory/ws:main/agent:flow/app:learnings".to_string(),
        ],
        "deepest first"
    );
    assert_eq!(listed(&engine).await, vec!["chat fact".to_string()]);

    for item in &items[..2] {
        let again = engine.store(item.clone()).await.unwrap();
        assert!(!again.replayed, "an erased item's key was released");
    }
    assert_eq!(listed(&engine).await.len(), 3);
}

#[tokio::test]
async fn kinds_narrow_an_erasure() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    engine
        .store(StoreItem::document(
            "a document",
            MemoryMeta {
                namespace: at.clone(),
                ..MemoryMeta::default()
            },
        ))
        .await
        .unwrap();

    let mut req = EraseRequest::new(Reach::exact(at));
    req.kinds = vec![ItemKind::Learning];
    engine.erase(req).await.unwrap();
    assert_eq!(state.seen.lock().unwrap().erasures.len(), 1);
    assert_eq!(listed(&engine).await, vec!["a document".to_string()]);
}

#[tokio::test]
async fn an_erasure_with_nothing_registered_sends_nothing() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let report = engine
        .erase(EraseRequest::new(Reach::subtree(node("agent:nobody"))))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 0);
    assert!(state.seen.lock().unwrap().erasures.is_empty());
}

#[tokio::test]
async fn the_hosted_wire_erases_a_subtree_through_the_backend_passthrough() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let (gone, kept) = (node("ws:main/agent:flow"), node("ws:main/agent:chat"));
    let erased = learning("flow fact", &gone);
    engine.store(erased.clone()).await.unwrap();
    engine.store(learning("chat fact", &kept)).await.unwrap();

    let report = engine
        .erase(EraseRequest::new(Reach::subtree(gone)))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 1, "{report:?}");
    assert_eq!(state.count("POST /memory/v1/erasures"), 1);
    assert_eq!(state.count("DELETE /memory"), 0, "never the whole memory");
    let bodies = state.seen.lock().unwrap().erasures.clone();
    assert!(bodies[0].get("selector").is_none(), "{}", bodies[0]);
    assert_eq!(listed(&engine).await, vec!["chat fact".to_string()]);
    let again = engine.store(erased).await.unwrap();
    assert!(!again.replayed, "an erased item's key was released");
}

#[tokio::test]
async fn the_hosted_erasure_names_only_the_fields_memory_api_accepts() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    let body = state.seen.lock().unwrap().erasures[0].clone();
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["audit_note", "scope"], "{body}");
}

#[tokio::test]
async fn an_incomplete_hosted_erasure_is_retried() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    state.erasure_incomplete_for.store(1, Ordering::SeqCst);

    let report = engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 1);
    assert_eq!(state.count("POST /memory/v1/erasures"), 2);
    assert!(listed(&engine).await.is_empty());
}

#[tokio::test]
async fn a_hosted_erasure_that_stays_incomplete_is_unavailable() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    state.erasure_incomplete_for.store(10, Ordering::SeqCst);

    let failed = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(failed, Err(tinymemory_api::Error::Unavailable(_))),
        "{failed:?}"
    );
}

#[tokio::test]
async fn a_running_direct_erasure_is_polled_until_it_completes() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    // The POST and the first two polls answer `running`.
    state.erasure_running_for.store(3, Ordering::SeqCst);

    let report = engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    assert_eq!(report.receipts, vec!["erasure_1".to_string()]);
    assert_eq!(state.count("GET /v1/erasures/erasure_1"), 3);
}

#[tokio::test]
async fn a_direct_erasure_that_does_not_complete_is_an_error() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    state.erasure_running_for.store(1, Ordering::SeqCst);
    *state.erasure_ends.lock().unwrap() = Some("failed");

    let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(&refused, Err(tinymemory_api::Error::Engine(m)) if m.contains("failed")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_backend_without_the_scoped_erasure_route_is_unsupported() {
    let (endpoint, state) = hosted_double().await;
    state.scoped_erase_missing.store(true, Ordering::SeqCst);
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(refused, Err(tinymemory_api::Error::Unsupported(_))),
        "a caller falls back to forget: {refused:?}"
    );
    assert_eq!(listed(&engine).await, vec!["a fact".to_string()]);
}

fn whole_tree() -> EraseRequest {
    let mut whole = EraseRequest::new(Reach::subtree(Namespace::ROOT));
    whole.whole_tree = true;
    whole
}

#[tokio::test]
async fn the_hosted_wire_erases_the_whole_memory_in_one_request() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    engine
        .store(learning("a fact", &node("agent:a")))
        .await
        .unwrap();
    engine
        .store(learning("another fact", &node("agent:b")))
        .await
        .unwrap();
    assert_eq!(listed(&engine).await.len(), 2);
    let before = state.requests().len();

    let report = engine.erase(whole_tree()).await.unwrap();

    assert_eq!(report.erased_scopes, 2, "{report:?}");
    assert!(report.receipts.is_empty());
    assert_eq!(state.requests()[before..], ["DELETE /memory".to_string()]);
    assert_eq!(state.event_count(), 0);
    assert!(listed(&engine).await.is_empty());
}

#[tokio::test]
async fn a_narrower_hosted_erase_goes_scope_by_scope_not_whole_memory() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    engine
        .store(learning("a fact", &node("agent:a")))
        .await
        .unwrap();
    let mut learnings = whole_tree();
    learnings.kinds = vec![ItemKind::Learning];
    engine.erase(learnings).await.unwrap();
    assert_eq!(state.count("DELETE /memory"), 0);
    assert_eq!(state.count("POST /memory/v1/erasures"), 1);

    let missing_interlock = engine
        .erase(EraseRequest::new(Reach::subtree(Namespace::ROOT)))
        .await;
    assert!(
        matches!(
            missing_interlock,
            Err(tinymemory_api::Error::InvalidRequest(_))
        ),
        "{missing_interlock:?}"
    );
}

#[tokio::test]
async fn a_backend_without_the_erase_route_is_unsupported() {
    let (endpoint, state) = hosted_double().await;
    state
        .erase_all_missing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let engine = hosted_engine(&endpoint);
    let refused = engine.erase(whole_tree()).await;
    assert!(
        matches!(refused, Err(tinymemory_api::Error::Unsupported(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_hosted_outage_on_the_erase_is_unavailable() {
    let (endpoint, state) = hosted_double().await;
    state
        .fail_all
        .lock()
        .unwrap()
        .replace((503, "UPSTREAM_UNAVAILABLE"));
    let engine = hosted_engine(&endpoint);
    let failed = engine.erase(whole_tree()).await;
    assert!(
        matches!(failed, Err(tinymemory_api::Error::Unavailable(_))),
        "{failed:?}"
    );
}

/// Backend contract (tinyhumansai/backend#1409): `DELETE /memory` answers
/// `{success:true,data:{erased:true,scopes:n}}`, authenticated like the other
/// memory routes, and fails as `{success:false,error,errorCode}`.
#[tokio::test]
async fn the_hosted_erase_reads_the_backend_envelope() {
    let (endpoint, state) = hosted_double().await;
    *state.erase_all_answer.lock().unwrap() =
        Some(serde_json::json!({ "erased": true, "scopes": 3 }));
    let engine = hosted_engine(&endpoint);
    let report = engine.erase(whole_tree()).await.unwrap();
    assert_eq!(report.erased_scopes, 3, "{report:?}");
    assert!(
        state
            .seen
            .lock()
            .unwrap()
            .auth
            .iter()
            .all(|a| a.starts_with("Bearer ")),
        "authenticated like the other memory routes"
    );

    // An unauthenticated caller gets the error envelope with its code.
    *state.accept_token.lock().unwrap() = Some("other".to_string());
    let denied = engine.erase(whole_tree()).await.unwrap_err();
    assert!(
        matches!(
            denied,
            tinymemory_api::Error::Unauthorized(_) | tinymemory_api::Error::Unavailable(_)
        ),
        "{denied:?}"
    );
}

#[tokio::test]
async fn a_hosted_erase_answer_without_erased_true_is_an_engine_error() {
    for data in [
        serde_json::json!({ "scopes": 2 }),
        serde_json::json!({ "erased": false, "scopes": 2 }),
        serde_json::json!({ "erased": "true", "scopes": 2 }),
    ] {
        let (endpoint, state) = hosted_double().await;
        *state.erase_all_answer.lock().unwrap() = Some(data.clone());
        let failed = hosted_engine(&endpoint).erase(whole_tree()).await;
        assert!(failed.is_err(), "{data}: {failed:?}");
    }
}

#[tokio::test]
async fn a_hosted_erase_answer_with_a_missing_or_malformed_count_is_an_error() {
    for data in [
        serde_json::json!({ "erased": true }),
        serde_json::json!({ "erased": true, "scopes": "2" }),
        serde_json::json!({ "erased": true, "scopes": -1 }),
        serde_json::json!({ "erased": true, "scopes": null }),
    ] {
        let (endpoint, state) = hosted_double().await;
        *state.erase_all_answer.lock().unwrap() = Some(data.clone());
        let failed = hosted_engine(&endpoint).erase(whole_tree()).await;
        assert!(failed.is_err(), "{data}: {failed:?}");
    }
}

#[tokio::test]
async fn the_gate_answers_before_a_missing_erase_route() {
    let (endpoint, state) = hosted_double().await;
    state
        .erase_all_missing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    *state.accept_token.lock().unwrap() = Some("other".to_string());
    let refused = hosted_engine(&endpoint).erase(whole_tree()).await;
    assert!(
        !matches!(refused, Err(tinymemory_api::Error::Unsupported(_))),
        "an unauthenticated caller is told so, not that the route is missing: {refused:?}"
    );
}

#[test]
fn the_double_erases_every_scope_of_its_log() {
    let mut log = crate::cortex::testing::CortexLog::default();
    for (n, scope) in ["a", "a/b", "c"].into_iter().enumerate() {
        let event = serde_json::json!({ "id": format!("e{n}"), "scope": scope });
        log.events.push(event);
    }
    assert_eq!(log.erase_everything(), 3);
    assert!(log.events.is_empty());
    assert_eq!(log.erasures, 3, "one whole-scope erasure per scope");
}

#[tokio::test]
async fn the_whole_tree_needs_its_interlock() {
    let (endpoint, _) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let refused = engine
        .erase(EraseRequest::new(Reach::subtree(Namespace::ROOT)))
        .await;
    assert!(
        matches!(refused, Err(tinymemory_api::Error::InvalidRequest(_))),
        "{refused:?}"
    );
    let mut whole = EraseRequest::new(Reach::subtree(Namespace::ROOT));
    whole.whole_tree = true;
    engine.erase(whole).await.unwrap();
}

#[tokio::test]
async fn the_double_drops_beliefs_built_from_erased_events() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    let scope = "app:tinymemory/agent:assistant/app:learnings";
    {
        let mut log = state.log.lock().unwrap();
        let source = log.events[0]["id"].clone();
        log.beliefs
            .push(serde_json::json!({ "scope": scope, "source": source, "text": "built" }));
    }
    engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    assert!(state.log.lock().unwrap().beliefs.is_empty());
    assert_eq!(state.log.lock().unwrap().forgotten.len(), 1);
}

#[test]
fn the_double_refuses_an_erasure_without_a_scope() {
    let mut log = crate::cortex::testing::CortexLog::default();
    let (status, _) = log.erase(&serde_json::json!({ "confirm_all": true }));
    assert_eq!(status, 422);
}

#[tokio::test]
async fn a_direct_erasure_that_stays_running_times_out_as_unavailable() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    // Running for far longer than the test's erasure deadline.
    state.erasure_running_for.store(100_000, Ordering::SeqCst);

    let started = std::time::Instant::now();
    let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(&refused, Err(tinymemory_api::Error::Unavailable(m)) if m.contains("pending")),
        "{refused:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the deadline bounds the whole poll loop"
    );
    assert!(state.count("GET /v1/erasures/erasure_1") >= 1);
}

#[tokio::test]
async fn a_polled_erasure_without_a_status_is_not_completed() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    state.erasure_running_for.store(1, Ordering::SeqCst);
    state.erasure_poll_omits_status.store(true, Ordering::SeqCst);

    let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(&refused, Err(tinymemory_api::Error::Engine(m)) if m.contains("status")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_direct_erasure_that_keeps_a_non_terminal_status_is_not_done() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    // Settles as `cancelled`: neither pending nor completed.
    state.erasure_running_for.store(1, Ordering::SeqCst);
    *state.erasure_ends.lock().unwrap() = Some("cancelled");

    let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
    assert!(
        matches!(&refused, Err(tinymemory_api::Error::Engine(m)) if m.contains("cancelled")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_hosted_erasure_reports_the_scopes_the_backend_erased() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    // The scope emptied between the listing and the erasure.
    *state.scoped_erase_answer.lock().unwrap() =
        Some(serde_json::json!({ "erased": true, "scopes": 0, "erasure_ids": [] }));

    let report = engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 0, "{report:?}");
    assert!(report.receipts.is_empty());
}

#[tokio::test]
async fn a_malformed_hosted_erasure_answer_is_an_error() {
    for answer in [
        serde_json::json!({ "erased": true, "scopes": 1, "erasure_ids": [123] }),
        serde_json::json!({ "erased": true, "scopes": 1, "erasure_ids": "x" }),
        serde_json::json!({ "erased": true, "erasure_ids": [] }),
    ] {
        let (endpoint, state) = hosted_double().await;
        let engine = hosted_engine(&endpoint);
        let at = node("agent:assistant");
        engine.store(learning("a fact", &at)).await.unwrap();
        *state.scoped_erase_answer.lock().unwrap() = Some(answer.clone());
        let refused = engine.erase(EraseRequest::new(Reach::exact(at))).await;
        assert!(
            matches!(refused, Err(tinymemory_api::Error::Engine(_))),
            "{answer}: {refused:?}"
        );
    }
}
