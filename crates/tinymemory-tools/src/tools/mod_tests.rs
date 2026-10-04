//! `MemoryTools`: dispatch, the read-only refusal, and the scope builders.

use super::*;
use serde_json::json;
use tinymemory_api::conformance::ReferenceEngine;

fn tools() -> MemoryTools {
    MemoryTools::new(Arc::new(ReferenceEngine::new()))
}

#[test]
fn new_writes_to_the_root_reads_everything_and_allows_writes() {
    assert_eq!(tools().scope(), &ToolScope::default());
    assert_eq!(ToolScope::default().place, Namespace::ROOT);
    assert!(ToolScope::default().reach.is_none());
    assert!(ToolScope::default().writes);
}

#[test]
fn placing_sets_the_reach_to_the_place_and_its_ancestors() {
    let place = Namespace::agent("a");
    let placed = tools().placed_at(place.clone());
    assert_eq!(placed.scope().place, place);
    assert_eq!(placed.scope().reach, Some(Reach::of(place.clone())));

    let narrowed = tools()
        .placed_at(place.clone())
        .reach(Reach::exact(place.clone()));
    assert_eq!(narrowed.scope().reach, Some(Reach::exact(place.clone())));

    let reset = tools()
        .reach(Reach::subtree(Namespace::ROOT))
        .placed_at(place.clone());
    assert_eq!(reset.scope().reach, Some(Reach::of(place)));
}

#[test]
fn placing_keeps_read_only() {
    let tools = tools().read_only().placed_at(Namespace::agent("a"));
    assert!(!tools.scope().writes);
}

#[test]
fn with_scope_uses_the_scope_given() {
    let scope = ToolScope {
        writes: false,
        ..ToolScope::at(Namespace::agent("a"))
    };
    let tools = MemoryTools::with_scope(Arc::new(ReferenceEngine::new()), scope.clone());
    assert_eq!(tools.scope(), &scope);
}

#[test]
fn debug_names_the_engine_and_scope() {
    let text = format!("{:?}", tools());
    assert!(
        text.contains("reference") && text.contains("scope"),
        "{text}"
    );
}

#[tokio::test]
async fn an_unknown_tool_is_an_invalid_request() {
    let error = tools()
        .call("memory_delete_all", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::InvalidRequest(message) if message.contains("memory_delete_all"))
    );
}

#[tokio::test]
async fn write_tools_are_unsupported_when_read_only() {
    let tools = tools().read_only();
    for name in WRITE_TOOL_NAMES {
        let error = tools
            .call(name, json!({ "learning": { "text": "x" } }))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Unsupported(_)), "{name}: {error:?}");
    }
}

#[tokio::test]
async fn every_tool_dispatches() {
    let tools = tools();
    let stored = tools
        .call(
            MEMORY_STORE,
            json!({ "learning": { "text": "rust is fast" } }),
        )
        .await
        .unwrap();
    let id = stored["id"].clone();
    let calls = [
        (MEMORY_RECALL, json!({ "question": "rust" })),
        (MEMORY_FETCH, json!({ "query": "rust" })),
        (MEMORY_LIST, Value::Null),
        (MEMORY_GET, json!({ "ids": [id] })),
        (MEMORY_EXPLORE, json!({ "facet": "kind" })),
        (MEMORY_FORGET, json!({ "ids": [id] })),
    ];
    for (name, args) in calls {
        assert!(tools.call(name, args).await.is_ok(), "{name}");
    }
}

#[test]
fn the_call_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    let tools = tools();
    assert_send(tools.call(MEMORY_LIST, Value::Null));
}
