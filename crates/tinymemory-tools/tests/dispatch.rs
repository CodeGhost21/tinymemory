//! Each tool reaches the right driver family with the arguments it was given,
//! validates before it asks the host for a driver, and words its errors the way
//! the agent has always read them.

mod common;

use common::TestHost;
use serde_json::json;
use tinymemory_tools::query::{
    MemoryTreeCoverWindowTool, MemoryTreeDrillDownTool, MemoryTreeFetchLeavesTool,
    MemoryTreeQuerySourceTool, MemoryTreeSearchEntitiesTool, MemoryTreeTool,
};
use tinymemory_tools::raw_store::{
    MemoryStoreKindsTool, MemoryStoreRawChunksTool, MemoryStoreRawSearchTool,
};
use tinymemory_tools::search::{
    MemoryChunkContextTool, MemoryHybridSearchTool, MemoryVectorSearchTool,
};
use tinymemory_tools::tool_memory::{MemoryToolsListTool, MemoryToolsPutTool};
use tinytools::Tool;

fn message(result: anyhow::Result<tinytools::ToolResult>) -> String {
    result.expect_err("expected an error").to_string()
}

// ── the memory_tree dispatcher ───────────────────────────────────────────────

#[tokio::test]
async fn memory_tree_routes_each_mode_to_its_retrieval_member() {
    let cases = [
        (
            json!({"mode": "search_entities", "query": "alice"}),
            "retrieval.search_entities",
        ),
        (
            json!({"mode": "query_source", "source_id": "slack:#eng"}),
            "retrieval.retrieve_source",
        ),
        (
            json!({"mode": "drill_down", "node_id": "n1"}),
            "retrieval.retrieve_children",
        ),
        (
            json!({"mode": "cover_window", "since_ms": 1, "until_ms": 2}),
            "retrieval.cover_window",
        ),
        (
            json!({"mode": "fetch_leaves", "chunk_ids": ["c1"]}),
            "retrieval.retrieve_leaves",
        ),
        (
            json!({"mode": "walk", "query": "what happened"}),
            "retrieval.fast_retrieve",
        ),
        (
            json!({"mode": "smart_walk", "query": "what happened"}),
            "retrieval.fast_retrieve",
        ),
    ];
    for (args, method) in cases {
        let host = TestHost::bound();
        let tool = MemoryTreeTool::new(host.clone());
        let result = tool.execute(args.clone()).await.expect("mode succeeds");
        assert!(!result.is_error, "{args}: {result:?}");
        assert_eq!(host.methods(), vec![method.to_string()], "{args}");
        assert!(host.all_unscoped(), "{args}: no explicit scope");
    }
}

#[tokio::test]
async fn memory_tree_ingest_document_is_the_hosts() {
    let host = TestHost::bound();
    let tool = MemoryTreeTool::new(host.clone());
    let args = json!({"mode": "ingest_document", "title": "t", "body": "b"});
    let result = tool.execute(args.clone()).await.unwrap();
    assert_eq!(result.output(), format!("host-ingest:{args}"));
    assert!(host.methods().is_empty(), "the driver is the host's to touch");
}

#[tokio::test]
async fn memory_tree_rejects_a_missing_or_unknown_mode() {
    let tool = MemoryTreeTool::new(TestHost::unbound());
    assert_eq!(
        message(tool.execute(json!({})).await),
        "memory_tree: `mode` is required"
    );
    assert_eq!(
        message(tool.execute(json!({"mode": "nope"})).await),
        "memory_tree: unknown mode `nope`. Valid: search_entities, query_source, drill_down, \
         cover_window, fetch_leaves, ingest_document, walk, smart_walk"
    );
    assert_eq!(
        message(tool.execute(json!({"mode": "walk", "query": "  "})).await),
        "memory_tree walk: `query` is required"
    );
}

// ── argument validation happens before a driver is resolved ──────────────────

#[tokio::test]
async fn bad_arguments_fail_before_the_host_is_asked_for_a_driver() {
    let host = TestHost::unbound();

    assert!(message(
        MemoryTreeQuerySourceTool::new(host.clone())
            .execute(json!({"source_kind": "bogus"}))
            .await
    )
    .starts_with("memory_tree_query_source: "));
    assert!(message(
        MemoryTreeCoverWindowTool::new(host.clone())
            .execute(json!({"since_ms": 1, "until_ms": 2, "source_kind": "bogus"}))
            .await
    )
    .starts_with("memory_tree_cover_window: "));
    assert_eq!(
        message(
            MemoryTreeDrillDownTool::new(host.clone())
                .execute(json!({"node_id": "n", "max_depth": 0}))
                .await
        ),
        "memory_tree_drill_down: max_depth must be >= 1"
    );
    assert!(message(
        MemoryStoreRawChunksTool::new(host.clone())
            .execute(json!({"limit": 5000}))
            .await
    )
    .contains("limit must be between 1 and 1000"));
    assert!(message(
        MemoryHybridSearchTool::new(host.clone())
            .execute(json!({"query": "q", "namespace": "global", "mode": "mystery"}))
            .await
    )
    .contains("unknown mode 'mystery'"));
    assert_eq!(
        message(
            MemoryVectorSearchTool::new(host.clone())
                .execute(json!({"query": "  "}))
                .await
        ),
        "memory_vector_search: query cannot be empty"
    );
    assert!(message(
        MemoryTreeFetchLeavesTool::new(host.clone())
            .execute(json!({"wrong": 1}))
            .await
    )
    .starts_with("invalid arguments for memory_tree_fetch_leaves: "));
}

#[tokio::test]
async fn an_unbound_driver_is_reported_under_the_tools_name() {
    let host = TestHost::unbound();
    assert_eq!(
        message(MemoryStoreKindsTool::new(host.clone()).execute(json!({})).await),
        "memory_store_kinds: no memory driver is bound"
    );
    assert_eq!(
        message(
            MemoryTreeSearchEntitiesTool::new(host.clone())
                .execute(json!({"query": "a"}))
                .await
        ),
        "memory_tree_search_entities: no memory driver is bound"
    );
    assert_eq!(
        message(
            MemoryStoreRawSearchTool::new(host.clone())
                .execute(json!({"query": "a"}))
                .await
        ),
        "memory_store_raw_search: no memory driver is bound"
    );
    assert_eq!(
        message(
            MemoryChunkContextTool::new(host.clone())
                .execute(json!({"chunk_id": "c"}))
                .await
        ),
        "memory_chunk_context: no memory driver is bound"
    );
}

#[tokio::test]
async fn the_vector_tool_resolves_the_embedder_first_and_words_its_failure_as_the_hosts() {
    assert_eq!(
        message(
            MemoryVectorSearchTool::new(TestHost::unbound())
                .execute(json!({"query": "hello"}))
                .await
        ),
        "memory_vector_search: load config failed: no workspace"
    );
}

// ── the raw-store and tool-memory families ───────────────────────────────────

#[tokio::test]
async fn store_kinds_reads_the_chunk_family() {
    let host = TestHost::bound();
    let result = MemoryStoreKindsTool::new(host.clone())
        .execute(json!({}))
        .await
        .unwrap();
    assert_eq!(result.output(), r#"{"kinds":[]}"#);
    assert_eq!(host.methods(), vec!["chunks.storage_kinds"]);
}

#[tokio::test]
async fn raw_chunks_and_chunk_context_list_or_look_up_chunks() {
    let host = TestHost::bound();
    let result = MemoryStoreRawChunksTool::new(host.clone())
        .execute(json!({"source_kind": "chat", "limit": 25}))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(host.methods(), vec!["chunks.list_chunks"]);

    let host = TestHost::bound();
    let error = message(
        MemoryChunkContextTool::new(host.clone())
            .execute(json!({"chunk_id": "missing"}))
            .await,
    );
    assert_eq!(error, "memory_chunk_context: chunk_id not found");
    assert_eq!(host.methods(), vec!["chunks.get_chunk"]);
}

#[tokio::test]
async fn tool_memory_put_then_list_round_trips_through_the_family() {
    let host = TestHost::bound();
    let put = MemoryToolsPutTool::new(host.clone())
        .execute(json!({
            "tool_name": "shell",
            "rule": "never run rm -rf",
            "priority": "critical",
            "tags": ["safety"]
        }))
        .await
        .unwrap();
    assert!(!put.is_error, "{put:?}");
    let list = MemoryToolsListTool::new(host.clone())
        .execute(json!({"tool_name": "shell"}))
        .await
        .unwrap();
    assert!(list.output().contains("never run rm -rf"), "{list:?}");
    assert!(list.output().contains("critical") || list.output().contains("Critical"));
}

#[test]
fn the_shared_no_tool_memory_message_is_unchanged() {
    assert_eq!(
        tinymemory_tools::NO_TOOL_MEMORY,
        "memory driver does not support the tool_memory family"
    );
}
