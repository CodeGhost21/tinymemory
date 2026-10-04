//! Every memory tool round-trips through `MemoryTools::call` against the
//! reference engine, and read-only tools neither list nor run the writes.

use std::sync::Arc;

use serde_json::{Value, json};
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, Error, FetchMode, FetchPage, FetchRequest, ForgetReport,
    ForgetTarget, ListPage, ListRequest, MemoryEngine, RecallAnswer, RecallRequest, Result,
    StoreItem, StoreReceipt, async_trait,
};
use tinymemory_tools::{
    MEMORY_EXPLORE, MEMORY_FETCH, MEMORY_FORGET, MEMORY_GET, MEMORY_LIST, MEMORY_RECALL,
    MEMORY_STORE, MemoryTools, TOOL_NAMES, WRITE_TOOL_NAMES,
};

fn tools() -> MemoryTools {
    MemoryTools::new(Arc::new(ReferenceEngine::new()))
}

async fn store(tools: &MemoryTools, args: Value) -> String {
    let receipt = tools.call(MEMORY_STORE, args).await.unwrap();
    receipt["id"].as_str().unwrap().to_string()
}

fn texts(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn store_is_idempotent_and_reports_replays() {
    let tools = tools();
    let args = json!({ "learning": { "text": "the user prefers tabs" } });
    let first = tools.call(MEMORY_STORE, args.clone()).await.unwrap();
    let again = tools.call(MEMORY_STORE, args).await.unwrap();
    assert_eq!(first["replayed"], json!(false));
    assert_eq!(again["replayed"], json!(true));
    assert_eq!(first["id"], again["id"]);
}

#[tokio::test]
async fn recall_answers_with_citations() {
    let tools = tools();
    store(
        &tools,
        json!({ "learning": { "text": "rust ownership moves values" }, "tags": ["rust"] }),
    )
    .await;
    let answer = tools
        .call(
            MEMORY_RECALL,
            json!({ "question": "rust ownership", "limit": 3,
                                     "instructions": "be brief" }),
        )
        .await
        .unwrap();
    assert!(answer["answer"].as_str().unwrap().contains("ownership"));
    assert_eq!(answer["citations"][0]["kind"], json!("learning"));
    assert_eq!(answer["citations"][0]["meta"]["tags"], json!(["rust"]));
}

#[tokio::test]
async fn fetch_pages_with_a_cursor() {
    let tools = tools();
    for n in 0..3 {
        store(
            &tools,
            json!({ "document": { "text": format!("rust note {n}") } }),
        )
        .await;
    }
    let first = tools
        .call(
            MEMORY_FETCH,
            json!({ "query": "rust", "mode": "keyword", "limit": 2 }),
        )
        .await
        .unwrap();
    assert_eq!(first["hits"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].clone();
    assert!(cursor.is_string());
    let rest = tools
        .call(
            MEMORY_FETCH,
            json!({ "query": "rust", "mode": "keyword", "limit": 2, "cursor": cursor }),
        )
        .await
        .unwrap();
    assert_eq!(rest["hits"].as_array().unwrap().len(), 1);
    assert!(rest.get("next_cursor").is_none());
}

#[tokio::test]
async fn list_narrows_by_the_model_filter() {
    let tools = tools();
    store(
        &tools,
        json!({ "learning": { "text": "a learning" }, "tags": ["keep"] }),
    )
    .await;
    store(&tools, json!({ "document": { "text": "a document" } })).await;
    let learnings = tools
        .call(MEMORY_LIST, json!({ "filter": { "kinds": ["learning"] } }))
        .await
        .unwrap();
    assert_eq!(texts(&learnings["items"]), ["a learning"]);
    let tagged = tools
        .call(
            MEMORY_LIST,
            json!({ "filter": { "tags_any": ["keep"], "sources": ["agent"] } }),
        )
        .await
        .unwrap();
    assert_eq!(texts(&tagged["items"]), ["a learning"]);
}

#[tokio::test]
async fn get_reads_whole_items_and_reports_missing_ids() {
    let tools = tools();
    let id = store(
        &tools,
        json!({ "conversation": { "turns": [
        { "role": "user", "text": "hello" }, { "role": "assistant", "text": "hi" }
    ] } }),
    )
    .await;
    let result = tools
        .call(MEMORY_GET, json!({ "ids": [id, "unknown"] }))
        .await
        .unwrap();
    assert_eq!(texts(&result["items"]), ["user: hello\nassistant: hi"]);
    assert_eq!(result["missing"], json!(["unknown"]));
}

#[tokio::test]
async fn explore_counts_per_facet() {
    let tools = tools();
    store(
        &tools,
        json!({ "learning": { "text": "one" }, "tags": ["a", "b"] }),
    )
    .await;
    store(
        &tools,
        json!({ "learning": { "text": "two" }, "tags": ["a"] }),
    )
    .await;
    let page = tools
        .call(MEMORY_EXPLORE, json!({ "facet": "tag" }))
        .await
        .unwrap();
    assert_eq!(page["facet"], json!("tag"));
    assert_eq!(
        page["buckets"],
        json!([{ "value": "a", "count": 2 }, { "value": "b", "count": 1 }])
    );
    assert_eq!(page["total"], json!(2));
}

#[tokio::test]
async fn forget_by_ids_and_by_filter() {
    let tools = tools();
    let id = store(&tools, json!({ "learning": { "text": "first" } })).await;
    store(
        &tools,
        json!({ "learning": { "text": "second" }, "tags": ["drop"] }),
    )
    .await;
    store(&tools, json!({ "learning": { "text": "third" } })).await;

    let by_id = tools
        .call(MEMORY_FORGET, json!({ "ids": [id] }))
        .await
        .unwrap();
    assert_eq!(by_id, json!({ "forgotten": 1, "skipped": [] }));
    let by_filter = tools
        .call(MEMORY_FORGET, json!({ "filter": { "tags_any": ["drop"] } }))
        .await
        .unwrap();
    assert_eq!(by_filter, json!({ "forgotten": 1, "skipped": [] }));

    let left = tools.call(MEMORY_LIST, json!({})).await.unwrap();
    assert_eq!(texts(&left["items"]), ["third"]);
}

#[tokio::test]
async fn read_only_tools_omit_and_refuse_the_writes() {
    let tools = tools().read_only();
    let names: Vec<&str> = tools.specs().iter().map(|spec| spec.name).collect();
    assert_eq!(
        names,
        [
            MEMORY_RECALL,
            MEMORY_FETCH,
            MEMORY_LIST,
            MEMORY_GET,
            MEMORY_EXPLORE
        ]
    );
    for name in WRITE_TOOL_NAMES {
        let error = tools.call(name, json!({ "ids": ["x"] })).await.unwrap_err();
        assert!(matches!(error, Error::Unsupported(_)), "{name}: {error:?}");
    }
    assert!(tools.call(MEMORY_LIST, json!({})).await.is_ok());
}

#[tokio::test]
async fn unknown_tools_and_bad_arguments_are_invalid_requests() {
    let tools = tools();
    assert!(matches!(
        tools.call("memory_nuke", json!({})).await,
        Err(Error::InvalidRequest(_))
    ));
    let cases = [
        (MEMORY_RECALL, json!({ "question": "" }), "`question`"),
        (MEMORY_LIST, json!({ "limit": 0 }), "`limit`"),
        (MEMORY_LIST, json!({ "limit": 1000 }), "`limit`"),
        (MEMORY_LIST, json!("not an object"), "json object"),
        (MEMORY_GET, json!({ "ids": [] }), "`ids`"),
        (MEMORY_EXPLORE, json!({ "facet": "namespace" }), "`facet`"),
        (MEMORY_FETCH, json!({ "query": "x", "extra": 1 }), "`extra`"),
    ];
    for (name, args, field) in cases {
        match tools.call(name, args).await {
            Err(Error::InvalidRequest(message)) => {
                assert!(
                    message.starts_with(name) || name == "memory_nuke",
                    "{message}"
                );
                assert!(message.contains(field), "{name}: {message}");
                assert_eq!(message, message.to_lowercase(), "{message}");
            }
            other => panic!("{name}: expected an invalid request, got {other:?}"),
        }
    }
}

/// The reference engine advertising only keyword fetch.
struct KeywordOnly {
    inner: ReferenceEngine,
    descriptor: EngineDescriptor,
}

impl KeywordOnly {
    fn new() -> Self {
        let inner = ReferenceEngine::new();
        let descriptor = EngineDescriptor {
            fetch_modes: vec![FetchMode::Keyword],
            ..inner.descriptor().clone()
        };
        Self { inner, descriptor }
    }
}

#[async_trait]
impl MemoryEngine for KeywordOnly {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
    async fn health(&self) -> EngineHealth {
        self.inner.health().await
    }
    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer> {
        self.inner.recall(req).await
    }
    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage> {
        self.descriptor.ensure_mode(req.mode)?;
        self.inner.fetch(req).await
    }
    async fn store(&self, item: StoreItem) -> Result<StoreReceipt> {
        self.inner.store(item).await
    }
    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport> {
        self.inner.forget(target).await
    }
    async fn list(&self, req: ListRequest) -> Result<ListPage> {
        self.inner.list(req).await
    }
}

#[tokio::test]
async fn the_fetch_mode_enum_matches_the_engine_descriptor() {
    let reference = tools();
    let fetch = reference
        .specs()
        .into_iter()
        .find(|spec| spec.name == MEMORY_FETCH)
        .unwrap();
    let modes: Vec<&str> = FetchMode::ALL.iter().map(|mode| mode.as_str()).collect();
    assert_eq!(fetch.parameters["properties"]["mode"]["enum"], json!(modes));

    let keyword = MemoryTools::new(Arc::new(KeywordOnly::new()));
    let fetch = keyword
        .specs()
        .into_iter()
        .find(|spec| spec.name == MEMORY_FETCH)
        .unwrap();
    assert_eq!(
        fetch.parameters["properties"]["mode"]["enum"],
        json!(["keyword"])
    );

    // With no mode named, the engine's only mode is used.
    store(
        &keyword,
        json!({ "document": { "text": "keyword search works" } }),
    )
    .await;
    let hits = keyword
        .call(MEMORY_FETCH, json!({ "query": "keyword" }))
        .await
        .unwrap();
    assert_eq!(texts(&hits["hits"]), ["keyword search works"]);
    assert!(matches!(
        keyword
            .call(MEMORY_FETCH, json!({ "query": "x", "mode": "vector" }))
            .await,
        Err(Error::InvalidRequest(_))
    ));
}

#[test]
fn every_tool_name_is_listed_once() {
    let names: Vec<&str> = tools().specs().iter().map(|spec| spec.name).collect();
    assert_eq!(names, TOOL_NAMES);
}
