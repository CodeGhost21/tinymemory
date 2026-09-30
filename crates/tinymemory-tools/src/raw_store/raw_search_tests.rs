use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn default_limit_is_five() {
    assert_eq!(default_limit(), 5);
}

#[test]
fn args_deserialize_with_default_limit() {
    let args: Args = serde_json::from_value(json!({ "query": "alice" })).unwrap();
    assert_eq!(args.query, "alice");
    assert_eq!(args.limit, 5);
    assert!(args.kinds.is_none());
}

#[test]
fn parameters_schema_describes_required_query() {
    let tool = MemoryStoreRawSearchTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(schema["properties"]["limit"]["maximum"], 100);
}

#[tokio::test]
async fn execute_rejects_missing_query() {
    let tool = MemoryStoreRawSearchTool::new(NoHost);
    let err = tool
        .execute(json!({}))
        .await
        .expect_err("missing query should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_store_raw_search"));
}

#[tokio::test]
async fn execute_rejects_invalid_kind() {
    let tool = MemoryStoreRawSearchTool::new(NoHost);
    let err = tool
        .execute(json!({
            "query": "alice",
            "kinds": ["not-a-kind"]
        }))
        .await
        .expect_err("invalid kind should fail");
    assert!(err.to_string().contains("memory_store_raw_search:"));
}
