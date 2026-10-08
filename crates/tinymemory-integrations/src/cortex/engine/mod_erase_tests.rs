//! Erase: deepest scope first, so every event is deleted and every write
//! key released; kinds narrow it. The hosted wire erases through the
//! backend's `memory/v1/erasures` passthrough and polls a running erasure
//! until it settles.

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
async fn the_hosted_wire_erases_through_the_backend_passthrough() {
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
    let bodies = state.seen.lock().unwrap().erasures.clone();
    assert_eq!(bodies[0]["confirm_all"], true);
    assert!(bodies[0].get("selector").is_none(), "{}", bodies[0]);
    assert_eq!(listed(&engine).await, vec!["chat fact".to_string()]);
    let again = engine.store(erased).await.unwrap();
    assert!(!again.replayed, "an erased item's key was released");
}

#[tokio::test]
async fn a_running_hosted_erasure_is_polled_until_it_completes() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let at = node("agent:assistant");
    engine.store(learning("a fact", &at)).await.unwrap();
    // The POST and the first two polls answer `running`.
    state.erasure_running_for.store(3, Ordering::SeqCst);

    let report = engine
        .erase(EraseRequest::new(Reach::exact(at)))
        .await
        .unwrap();
    assert_eq!(report.receipts, vec!["erasure_1".to_string()]);
    assert_eq!(state.count("GET /memory/v1/erasures/erasure_1"), 3);
}

#[tokio::test]
async fn an_erasure_that_does_not_complete_is_an_error() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
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
