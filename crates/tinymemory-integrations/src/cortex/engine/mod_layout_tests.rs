//! Layout v3: an engine with a scope root writes, lists and registers below
//! it, and never reads the legacy tree (nor the legacy engine its tree).

use std::collections::BTreeSet;

use super::*;
use crate::cortex::testing::{
    direct_double, direct_engine, hosted_double, hosted_engine, sample_items,
};
use tinymemory_api::{MetaFilter, Namespace, Reach};

/// The sample items placed the way a host lays them out: the document in a
/// source, the chat in a workspace, the learning at the root, plus a
/// workflow's learning in its service node.
fn placed() -> Vec<StoreItem> {
    let at = |item: StoreItem, namespace: &str| {
        let mut item = item;
        item.meta_mut().namespace = namespace.parse().unwrap();
        item
    };
    let mut items = sample_items();
    let learning = items.pop().unwrap();
    let conversation = items.pop().unwrap();
    let document = items.pop().unwrap();
    let mut flow = learning.clone();
    if let StoreItem::Learning { text, .. } = &mut flow {
        *text = "newsletter item 5531 already sent".to_string();
    }
    vec![
        at(document, "source:gmail"),
        at(conversation, "ws:main"),
        at(learning, ""),
        at(flow, "ws:main/service:newsletter"),
    ]
}

fn scopes_written(state: &crate::cortex::testing::Shared) -> BTreeSet<String> {
    state
        .log
        .lock()
        .unwrap()
        .events
        .iter()
        .map(|event| event["scope"].as_str().unwrap().to_string())
        .collect()
}

async fn listed(engine: &CortexEngine, filter: MetaFilter) -> usize {
    engine
        .list(ListRequest::new(filter, 50))
        .await
        .unwrap()
        .items
        .len()
}

#[tokio::test]
async fn a_v3_engine_keeps_every_kind_under_a_leaf_below_its_root() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint)
        .with_scope_root("user:42", Some("user:42"))
        .unwrap();
    engine.store_many(placed()).await.unwrap();
    engine.store_many(placed()).await.unwrap();

    let expected: BTreeSet<String> = [
        "user:42/app:brain/source:gmail",
        "user:42/ws:main/app:conversations",
        "user:42/app:learnings",
        "user:42/ws:main/app:flows/service:newsletter/app:learnings",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(scopes_written(&state), expected);

    // Registered once, with the person as owner, before the first write.
    let registrations = state.seen.lock().unwrap().registrations.clone();
    assert_eq!(registrations.len(), 1, "{registrations:?}");
    assert_eq!(registrations[0]["path"], "user:42");
    assert_eq!(registrations[0]["members"][0]["actor"], "user:42");
    assert_eq!(registrations[0]["members"][0]["role"], "owner");
    let requests = state.requests();
    let first_write = requests.iter().position(|r| r.contains("/v1/experience"));
    let register = requests
        .iter()
        .position(|r| r.starts_with("POST /v1/scopes"));
    assert!(register < first_write, "{requests:?}");
    assert_eq!(state.count("POST /v1/scopes"), 1, "{requests:?}");

    // Read back through discovery; the workflow stays in its sandbox.
    assert_eq!(listed(&engine, MetaFilter::default()).await, 3);
    let flow: Namespace = "ws:main/service:newsletter".parse().unwrap();
    let in_flow = MetaFilter {
        reach: Some(Reach::exact(flow)),
        ..MetaFilter::default()
    };
    assert_eq!(listed(&engine, in_flow).await, 1);
}

#[tokio::test]
async fn switching_layout_moves_nothing_and_strands_nothing() {
    let (endpoint, state) = direct_double().await;
    let legacy = direct_engine(&endpoint);
    legacy.store_many(sample_items()).await.unwrap();
    let v3 = direct_engine(&endpoint)
        .with_scope_root("user:42", None)
        .unwrap();
    assert_eq!(
        listed(&v3, MetaFilter::default()).await,
        0,
        "v3 reads only its root"
    );
    v3.store_many(placed()).await.unwrap();
    assert_eq!(
        listed(&legacy, MetaFilter::default()).await,
        3,
        "the legacy tree is untouched, and read again on switching back"
    );
    assert!(
        scopes_written(&state)
            .iter()
            .any(|s| s.starts_with("app:tinymemory/"))
    );
    assert_eq!(
        state.seen.lock().unwrap().registrations.len(),
        0,
        "no owner, no registration"
    );
}

#[tokio::test]
async fn a_hosted_v3_engine_leaves_tenancy_to_the_backend() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint)
        .with_scope_root("user:6512ab0f", Some("user:6512ab0f"))
        .unwrap();
    engine.store_many(placed()).await.unwrap();
    assert!(
        state
            .requests()
            .iter()
            .all(|r| !r.contains("POST /v1/scopes"))
    );
    assert!(
        scopes_written(&state)
            .iter()
            .all(|scope| scope.starts_with("user:6512ab0f/")),
        "{:?}",
        scopes_written(&state)
    );
    assert_eq!(listed(&engine, MetaFilter::default()).await, 3);
}

#[test]
fn a_scope_root_is_checked_when_set() {
    let engine = direct_engine("http://127.0.0.1:9");
    assert!(matches!(
        engine.clone().with_scope_root("kb:policies", None),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        engine.with_scope_root("user:42", Some("  ")),
        Err(Error::Config(_))
    ));
}

#[tokio::test]
async fn a_failed_registration_never_fails_a_write_and_is_tried_again() {
    let (endpoint, state) = direct_double().await;
    *state.fail_registration.lock().unwrap() = Some((403, "POLICY_DENIED"));
    let engine = direct_engine(&endpoint)
        .with_scope_root("user:42", Some("user:42"))
        .unwrap();
    engine.store_many(placed()).await.unwrap();
    assert_eq!(state.event_count(), 6, "the writes went on");
    engine.store_many(placed()).await.unwrap();
    assert_eq!(
        state.count("POST /v1/scopes"),
        2,
        "tried again on the next write"
    );

    *state.fail_registration.lock().unwrap() = None;
    engine.store_many(placed()).await.unwrap();
    engine.store_many(placed()).await.unwrap();
    assert_eq!(state.count("POST /v1/scopes"), 3, "done once it succeeds");
    assert_eq!(state.seen.lock().unwrap().registrations.len(), 1);
}
