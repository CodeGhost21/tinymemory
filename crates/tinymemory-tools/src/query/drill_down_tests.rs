use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parameters_schema_requires_node_id() {
    let tool = MemoryTreeDrillDownTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["node_id"]));
    assert_eq!(schema["properties"]["max_depth"]["minimum"], 1);
}

#[test]
fn drill_down_request_deserializes_optional_fields() {
    let req: DrillDownRequest = serde_json::from_value(json!({
        "node_id": "summary-1",
        "max_depth": 2,
        "query": "deployment blockers",
        "limit": 7
    }))
    .unwrap();
    assert_eq!(req.node_id, "summary-1");
    assert_eq!(req.max_depth, Some(2));
    assert_eq!(req.query.as_deref(), Some("deployment blockers"));
    assert_eq!(req.limit, Some(7));
}

#[tokio::test]
async fn execute_rejects_missing_node_id() {
    let tool = MemoryTreeDrillDownTool::new(NoHost);
    let err = tool
        .execute(json!({}))
        .await
        .expect_err("missing node_id should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tree_drill_down"));
}

#[tokio::test]
async fn execute_rejects_zero_max_depth() {
    let tool = MemoryTreeDrillDownTool::new(NoHost);
    let err = tool
        .execute(json!({
            "node_id": "summary-1",
            "max_depth": 0
        }))
        .await
        .expect_err("max_depth=0 should fail at tool boundary");
    assert!(err.to_string().contains("max_depth must be >= 1"));
}
