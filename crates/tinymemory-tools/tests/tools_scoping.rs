//! The security invariants: the namespace and reach are the host's, and a
//! model cannot read, write or forget outside its scope.
//!
//! The tree: `team:acme/agent:a` (the tools' place), its sibling
//! `team:acme/agent:b`, their shared parent `team:acme`, and the root.

use std::sync::Arc;

use serde_json::{Value, json};
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    Error, LearningKind, ListRequest, MemoryEngine, MemoryMeta, MetaFilter, Namespace, Reach,
    StoreItem,
};
use tinymemory_tools::{
    MEMORY_EXPLORE, MEMORY_FETCH, MEMORY_FORGET, MEMORY_GET, MEMORY_LIST, MEMORY_RECALL,
    MEMORY_STORE, MemoryTools, TOOL_NAMES,
};

#[allow(
    clippy::unwrap_used,
    reason = "a helper outside `#[test]` fails its test by panicking, as the tests do"
)]
fn ns(path: &str) -> Namespace {
    path.parse().unwrap()
}

struct World {
    engine: Arc<ReferenceEngine>,
    tools: MemoryTools,
    sibling_id: String,
    team_id: String,
}

/// An engine holding one secret at the sibling, one note at the team node,
/// and tools placed at `team:acme/agent:a`.
#[allow(
    clippy::unwrap_used,
    reason = "a helper outside `#[test]` fails its test by panicking, as the tests do"
)]
async fn world() -> World {
    let engine = Arc::new(ReferenceEngine::new());
    let put = |text: &str, at: &str| {
        let meta = MemoryMeta {
            namespace: ns(at),
            tags: vec!["shared-tag".into()],
            ..MemoryMeta::default()
        };
        StoreItem::learning(text, LearningKind::Fact, 0.9, meta)
    };
    let sibling = engine
        .store(put("sibling secret password", "team:acme/agent:b"))
        .await
        .unwrap();
    let team = engine
        .store(put("team note password", "team:acme"))
        .await
        .unwrap();
    let tools = MemoryTools::new(engine.clone()).placed_at(ns("team:acme/agent:a"));
    World {
        engine,
        tools,
        sibling_id: sibling.id.0,
        team_id: team.id.0,
    }
}

#[allow(
    clippy::unwrap_used,
    reason = "a helper outside `#[test]` fails its test by panicking, as the tests do"
)]
fn texts(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            item["text"]
                .as_str()
                .or(item["snippet"].as_str())
                .unwrap()
                .to_string()
        })
        .collect()
}

#[allow(
    clippy::unwrap_used,
    reason = "a helper outside `#[test]` fails its test by panicking, as the tests do"
)]
async fn held(engine: &ReferenceEngine) -> Vec<(String, Namespace)> {
    engine
        .list(ListRequest::new(MetaFilter::default(), 100))
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|hit| (hit.text, hit.meta.namespace))
        .collect()
}

#[tokio::test]
async fn a_namespace_or_reach_in_the_arguments_is_refused_by_every_tool() {
    let world = world().await;
    for name in TOOL_NAMES {
        for key in ["namespace", "reach"] {
            let top = json!({ key: "team:acme/agent:b" });
            let nested = json!({ "filter": { key: "team:acme/agent:b" } });
            for args in [top, nested] {
                match world.tools.call(name, args.clone()).await {
                    Err(Error::InvalidRequest(message)) => {
                        assert!(
                            message.contains("fixed by the host"),
                            "{name} {args}: {message}"
                        );
                    }
                    other => panic!("{name} {args}: expected a refusal, got {other:?}"),
                }
            }
        }
    }
    let nested_store = json!({ "learning": { "text": "x", "namespace": "root" } });
    assert!(matches!(
        world.tools.call(MEMORY_STORE, nested_store).await,
        Err(Error::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn store_lands_at_the_place() {
    let world = world().await;
    world
        .tools
        .call(
            MEMORY_STORE,
            json!({ "document": { "title": "mine", "text": "agent a doc" } }),
        )
        .await
        .unwrap();
    let placed: Vec<Namespace> = held(&world.engine)
        .await
        .into_iter()
        .filter(|(text, _)| text.contains("agent a doc"))
        .map(|(_, namespace)| namespace)
        .collect();
    assert_eq!(placed, [ns("team:acme/agent:a")]);
}

#[tokio::test]
async fn reads_never_return_the_siblings_item() {
    let world = world().await;
    let tools = &world.tools;

    let listed = tools
        .call(MEMORY_LIST, json!({ "limit": 50 }))
        .await
        .unwrap();
    assert_eq!(texts(&listed["items"]), ["team note password"]);

    for mode in ["keyword", "vector", "hybrid"] {
        let fetched = tools
            .call(
                MEMORY_FETCH,
                json!({ "query": "password secret", "mode": mode }),
            )
            .await
            .unwrap();
        assert!(
            !texts(&fetched["hits"])
                .iter()
                .any(|t| t.contains("sibling")),
            "{mode}: {fetched}"
        );
    }

    let recalled = tools
        .call(
            MEMORY_RECALL,
            json!({ "question": "what is the sibling secret password" }),
        )
        .await
        .unwrap();
    assert!(
        !recalled.to_string().contains("sibling secret"),
        "{recalled}"
    );
    assert_eq!(texts(&recalled["citations"]), ["team note password"]);

    let got = tools
        .call(
            MEMORY_GET,
            json!({ "ids": [world.sibling_id, world.team_id] }),
        )
        .await
        .unwrap();
    assert_eq!(texts(&got["items"]), ["team note password"]);
    assert_eq!(got["missing"], json!([world.sibling_id]));

    let explored = tools
        .call(MEMORY_EXPLORE, json!({ "facet": "tag" }))
        .await
        .unwrap();
    assert_eq!(
        explored["buckets"],
        json!([{ "value": "shared-tag", "count": 1 }])
    );
}

#[tokio::test]
async fn forgetting_the_siblings_id_is_skipped_and_the_item_survives() {
    let world = world().await;
    let result = world
        .tools
        .call(MEMORY_FORGET, json!({ "ids": [world.sibling_id] }))
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({ "forgotten": 0, "skipped": [world.sibling_id] })
    );
    assert_eq!(held(&world.engine).await.len(), 2);
}

#[tokio::test]
async fn forgetting_by_filter_is_confined_to_the_reach() {
    let world = world().await;
    let tools = MemoryTools::new(world.engine.clone())
        .placed_at(ns("team:acme/agent:a"))
        .reach(Reach::exact(ns("team:acme/agent:a")));
    tools
        .call(
            MEMORY_STORE,
            json!({ "learning": { "text": "mine to drop" }, "tags": ["shared-tag"] }),
        )
        .await
        .unwrap();
    let result = tools
        .call(
            MEMORY_FORGET,
            json!({ "filter": { "tags_any": ["shared-tag"] } }),
        )
        .await
        .unwrap();
    assert_eq!(result["forgotten"], json!(1));
    let mut left: Vec<String> = held(&world.engine)
        .await
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    left.sort();
    assert_eq!(left, ["sibling secret password", "team note password"]);
}

#[tokio::test]
async fn an_explicit_reach_replaces_the_placed_one() {
    let world = world().await;
    let team_wide = MemoryTools::new(world.engine.clone())
        .placed_at(ns("team:acme/agent:a"))
        .reach(Reach::subtree(ns("team:acme")));
    let listed = team_wide.call(MEMORY_LIST, json!({})).await.unwrap();
    assert_eq!(texts(&listed["items"]).len(), 2);
}

#[tokio::test]
async fn results_never_render_the_namespace() {
    let world = world().await;
    let listed = world.tools.call(MEMORY_LIST, json!({})).await.unwrap();
    assert!(!listed.to_string().contains("team:acme"), "{listed}");
}
