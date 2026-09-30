use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parameters_schema_requires_query() {
    let tool = MemoryTreeSearchEntitiesTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(
        schema["properties"]["limit"]["description"].is_string(),
        true
    );
}

#[test]
fn kind_enum_contains_expected_memory_entity_kinds() {
    let tool = MemoryTreeSearchEntitiesTool::new(NoHost);
    let schema = tool.parameters_schema();
    let kinds = schema["properties"]["kinds"]["items"]["enum"]
        .as_array()
        .unwrap();
    for required in ["email", "person", "organization", "topic"] {
        assert!(
            kinds.iter().any(|v| v == required),
            "missing kind {required}"
        );
    }
}

#[tokio::test]
async fn execute_rejects_missing_query() {
    let tool = MemoryTreeSearchEntitiesTool::new(NoHost);
    let err = tool
        .execute(json!({}))
        .await
        .expect_err("missing query should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tree_search_entities"));
}
