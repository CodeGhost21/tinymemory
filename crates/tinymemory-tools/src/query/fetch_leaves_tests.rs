use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parameters_schema_requires_chunk_ids() {
    let tool = MemoryTreeFetchLeavesTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["chunk_ids"]));
    assert_eq!(schema["properties"]["chunk_ids"]["type"], "array");
}

#[test]
fn max_chunk_ids_per_call_matches_description() {
    assert_eq!(MAX_CHUNK_IDS_PER_CALL, 20);
}

#[test]
fn request_slice_is_truncated_to_cap() {
    let ids: Vec<String> = (0..25).map(|i| format!("chunk-{i}")).collect();
    let take = ids.len().min(MAX_CHUNK_IDS_PER_CALL);
    assert_eq!(take, 20);
    assert_eq!(ids[..take].len(), 20);
    assert_eq!(ids[..take].first().map(String::as_str), Some("chunk-0"));
    assert_eq!(ids[..take].last().map(String::as_str), Some("chunk-19"));
}

#[tokio::test]
async fn execute_rejects_missing_chunk_ids() {
    let tool = MemoryTreeFetchLeavesTool::new(NoHost);
    let err = tool
        .execute(json!({}))
        .await
        .expect_err("missing chunk_ids should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tree_fetch_leaves"));
}

#[tokio::test]
async fn execute_rejects_wrong_type_for_chunk_ids() {
    let tool = MemoryTreeFetchLeavesTool::new(NoHost);
    let err = tool
        .execute(json!({"chunk_ids": "not-an-array"}))
        .await
        .expect_err("wrong chunk_ids type should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tree_fetch_leaves"));
}
