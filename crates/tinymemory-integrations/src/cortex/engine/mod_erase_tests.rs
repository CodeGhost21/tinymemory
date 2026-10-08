//! Erase on the Direct wire: deepest scope first, so every event is deleted
//! and every write key released; kinds narrow it. The hosted wire erases
//! only the whole tree, in one `DELETE /memory`, and refuses anything
//! narrower without a request.

use tinymemory_api::{
    EraseRequest, ItemKind, LearningKind, ListRequest, MemoryMeta, MetaFilter, Namespace, Reach,
    StoreItem,
};

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
async fn the_hosted_wire_refuses_to_erase_without_a_request() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let refused = engine
        .erase(EraseRequest::new(Reach::subtree(node("agent:x"))))
        .await;
    assert!(
        matches!(refused, Err(tinymemory_api::Error::Unsupported(_))),
        "{refused:?}"
    );
    assert!(state.seen.lock().unwrap().requests.is_empty());
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
async fn the_hosted_wire_refuses_a_narrower_erase_without_a_request() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let mut learnings = whole_tree();
    learnings.kinds = vec![ItemKind::Learning];
    let mut exact = EraseRequest::new(Reach::exact(Namespace::ROOT));
    exact.whole_tree = true;
    for req in [learnings, exact] {
        let refused = engine.erase(req).await;
        assert!(
            matches!(refused, Err(tinymemory_api::Error::Unsupported(_))),
            "{refused:?}"
        );
    }
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
    assert!(state.seen.lock().unwrap().requests.is_empty());
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
async fn a_hosted_erase_without_its_confirmation_is_an_engine_error() {
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
