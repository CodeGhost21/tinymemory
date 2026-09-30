use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parameters_schema_exposes_supported_source_filters() {
    let tool = MemoryTreeQuerySourceTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(
        schema["properties"]["source_kind"]["enum"],
        json!(["chat", "email", "document"])
    );
    assert_eq!(schema["properties"]["time_window_days"]["minimum"], 0);
}

#[tokio::test]
async fn execute_rejects_invalid_source_kind() {
    let tool = MemoryTreeQuerySourceTool::new(NoHost);
    let err = tool
        .execute(json!({
            "source_kind": "not-real"
        }))
        .await
        .expect_err("invalid source kind should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("memory_tree_query_source:") && !msg.contains("load config failed"),
        "expected a source-kind parse error, got: {msg}"
    );
}

#[tokio::test]
async fn execute_rejects_wrong_type_for_limit() {
    let tool = MemoryTreeQuerySourceTool::new(NoHost);
    let err = tool
        .execute(json!({
            "limit": "five"
        }))
        .await
        .expect_err("wrong limit type should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tree_query_source"));
}
