use super::*;
use crate::test_host::NoHost;
use serde_json::json;
use tinytools::Tool;

#[test]
fn args_deserialize_optional_filters() {
    let args: Args = serde_json::from_value(json!({
        "source_kind": "chat",
        "source_id": "slack:#eng",
        "owner": "alice",
        "since_ms": 10,
        "until_ms": 20,
        "tags_all_of": ["person:alice"],
        "limit": 25
    }))
    .unwrap();

    assert_eq!(args.source_kind.as_deref(), Some("chat"));
    assert_eq!(args.source_id.as_deref(), Some("slack:#eng"));
    assert_eq!(args.owner.as_deref(), Some("alice"));
    assert_eq!(args.since_ms, Some(10));
    assert_eq!(args.until_ms, Some(20));
    assert_eq!(args.tags_all_of, Some(vec!["person:alice".to_string()]));
    assert_eq!(args.limit, Some(25));
}

#[test]
fn parameters_schema_exposes_supported_source_kinds() {
    let tool = MemoryStoreRawChunksTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(
        schema["properties"]["source_kind"]["enum"],
        json!(["chat", "email", "document"])
    );
    assert_eq!(schema["properties"]["limit"]["maximum"], 1000);
}

#[tokio::test]
async fn execute_rejects_invalid_source_kind() {
    let tool = MemoryStoreRawChunksTool::new(NoHost);
    let err = tool
        .execute(json!({
            "source_kind": "not-real"
        }))
        .await
        .expect_err("invalid source kind should fail");
    assert!(err.to_string().contains("memory_store_raw_chunks:"));
}

#[tokio::test]
async fn execute_rejects_wrong_type_for_limit() {
    let tool = MemoryStoreRawChunksTool::new(NoHost);
    let err = tool
        .execute(json!({
            "limit": "ten"
        }))
        .await
        .expect_err("wrong limit type should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_store_raw_chunks"));
}
