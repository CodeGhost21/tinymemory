//! The `org:` root transition: the hosted wire sends paths relative to the
//! tenant root, a direct engine roots at `org:<id>`, and while a retired
//! `user:<id>` root is named every read covers it too (merged by item id),
//! every forget and erasure removes from it too, and no write goes there.

use std::collections::BTreeSet;

use tinymemory_api::{
    EraseRequest, ForgetTarget, GetRequest, LearningKind, ListRequest, MemoryMeta, MetaFilter,
    Namespace, Reach,
};

use super::*;
use crate::cortex::testing::{direct_double, direct_engine, hosted_double, hosted_engine};

fn learning(text: &str, namespace: &str) -> StoreItem {
    StoreItem::learning(
        text,
        LearningKind::Fact,
        0.9,
        MemoryMeta {
            namespace: namespace.parse().unwrap(),
            ..MemoryMeta::default()
        },
    )
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

async fn texts(engine: &CortexEngine) -> Vec<String> {
    let mut texts: Vec<String> = engine
        .list(ListRequest::new(MetaFilter::default(), 100))
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|hit| hit.text)
        .collect();
    texts.sort();
    texts
}

/// The engine before the transition: v3 below `user:42`.
fn retired(endpoint: &str) -> CortexEngine {
    direct_engine(endpoint)
        .with_scope_root("user:42", Some("user:42"))
        .unwrap()
}

/// The engine during it: `org:42`, still reading `user:42`.
fn transitional(endpoint: &str) -> CortexEngine {
    direct_engine(endpoint)
        .with_scope_root("org:42", Some("user:42"))
        .unwrap()
        .with_retired_root("user:42")
        .unwrap()
}

#[tokio::test]
async fn a_hosted_tenant_engine_sends_no_root_segment() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint).with_tenant_root().unwrap();
    engine
        .store_many(vec![
            learning("prefers dark mode", ""),
            learning("ships on fridays", "ws:main"),
        ])
        .await
        .unwrap();
    let expected: BTreeSet<String> = ["app:learnings", "ws:main/app:learnings"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(scopes_written(&state), expected);
    // The whole tenant is listed without a prefix, which the backend bounds
    // to the caller; an empty `prefix=` it would refuse.
    assert_eq!(
        texts(&engine).await,
        ["prefers dark mode", "ships on fridays"]
    );
    assert!(
        state.requests().iter().all(|r| !r.contains("prefix=&")),
        "{:?}",
        state.requests()
    );
}

#[tokio::test]
async fn a_hosted_tenant_engine_reads_the_retired_user_segment_too() {
    let (endpoint, state) = hosted_double().await;
    let old = hosted_engine(&endpoint)
        .with_scope_root("user:6512ab0f", None)
        .unwrap();
    old.store(learning("written before", "ws:main"))
        .await
        .unwrap();
    let new = hosted_engine(&endpoint)
        .with_tenant_root()
        .unwrap()
        .with_retired_root("user:6512ab0f")
        .unwrap();
    new.store(learning("written after", "ws:main"))
        .await
        .unwrap();
    assert!(scopes_written(&state).contains("ws:main/app:learnings"));
    assert_eq!(texts(&new).await, ["written after", "written before"]);
    let at_main = MetaFilter {
        reach: Some(Reach::exact("ws:main".parse().unwrap())),
        ..MetaFilter::default()
    };
    let exact = new.list(ListRequest::new(at_main, 10)).await.unwrap();
    assert_eq!(exact.items.len(), 2, "an exact reach reads both roots");

    // Without the retired root, the old path is no longer read at the
    // person's own nodes.
    let off = hosted_engine(&endpoint).with_tenant_root().unwrap();
    let at_main = MetaFilter {
        reach: Some(Reach::exact("ws:main".parse().unwrap())),
        ..MetaFilter::default()
    };
    let exact = off.list(ListRequest::new(at_main, 10)).await.unwrap();
    assert_eq!(exact.items.len(), 1);
    assert_eq!(exact.items[0].text, "written after");
}

#[test]
fn a_tenant_root_is_hosted_only_and_a_retired_root_needs_v3() {
    assert!(matches!(
        direct_engine("http://127.0.0.1:9").with_tenant_root(),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        direct_engine("http://127.0.0.1:9").with_retired_root("user:42"),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        transitional("http://127.0.0.1:9").with_retired_root("org:42"),
        Err(Error::Config(_))
    ));
}

#[tokio::test]
async fn reads_merge_both_roots_by_item_id_and_writes_go_only_to_the_new_one() {
    let (endpoint, state) = direct_double().await;
    let shared = learning("held in both", "ws:main");
    retired(&endpoint)
        .store_many(vec![shared.clone(), learning("only old", "")])
        .await
        .unwrap();
    let engine = transitional(&endpoint);
    engine
        .store_many(vec![shared.clone(), learning("only new", "")])
        .await
        .unwrap();

    let written = scopes_written(&state);
    assert!(
        written.contains("org:42/ws:main/app:learnings"),
        "{written:?}"
    );
    assert!(written.contains("org:42/app:learnings"), "{written:?}");
    assert_eq!(
        texts(&engine).await,
        ["held in both", "only new", "only old"],
        "one item per id"
    );
    // A get by id finds an item held only below the retired root.
    let old_id = learning("only old", "").fingerprint();
    let got = engine
        .get(GetRequest {
            ids: vec![old_id.into()],
            reach: None,
        })
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].text, "only old");

    // The new root's owner is the person's actor, registered on `org:42`.
    let registrations = state.seen.lock().unwrap().registrations.clone();
    assert!(
        registrations
            .iter()
            .any(|r| r["path"] == "org:42" && r["members"][0]["actor"] == "user:42"),
        "{registrations:?}"
    );
}

#[tokio::test]
async fn a_forget_by_id_and_by_filter_removes_from_both_roots() {
    let (endpoint, _state) = direct_double().await;
    let shared = learning("held in both", "ws:main");
    retired(&endpoint)
        .store_many(vec![shared.clone(), learning("old only", "ws:main")])
        .await
        .unwrap();
    let engine = transitional(&endpoint);
    engine.store(shared.clone()).await.unwrap();

    let report = engine
        .forget(ForgetTarget::Ids(vec![shared.fingerprint().into()]))
        .await
        .unwrap();
    assert_eq!(report.forgotten, 1);
    assert_eq!(texts(&engine).await, ["old only"]);
    assert_eq!(
        texts(&retired(&endpoint)).await,
        ["old only"],
        "gone from the retired root too"
    );

    let at_main = MetaFilter {
        reach: Some(Reach::exact("ws:main".parse().unwrap())),
        ..MetaFilter::default()
    };
    engine.forget(ForgetTarget::Filter(at_main)).await.unwrap();
    assert!(texts(&engine).await.is_empty());
    assert!(texts(&retired(&endpoint)).await.is_empty());
}

#[tokio::test]
async fn an_erasure_removes_both_roots() {
    let (endpoint, state) = direct_double().await;
    retired(&endpoint)
        .store(learning("old", "ws:main"))
        .await
        .unwrap();
    let engine = transitional(&endpoint);
    engine.store(learning("new", "ws:main")).await.unwrap();
    let report = engine
        .erase(EraseRequest::new(Reach::subtree(
            "ws:main".parse::<Namespace>().unwrap(),
        )))
        .await
        .unwrap();
    assert_eq!(report.erased_scopes, 2);
    let erased: Vec<String> = state
        .seen
        .lock()
        .unwrap()
        .erasures
        .iter()
        .map(|e| e.to_string())
        .collect();
    assert!(
        erased.iter().any(|e| e.contains("org:42/ws:main")),
        "{erased:?}"
    );
    assert!(
        erased.iter().any(|e| e.contains("user:42/ws:main")),
        "{erased:?}"
    );
    assert!(texts(&engine).await.is_empty());
}
