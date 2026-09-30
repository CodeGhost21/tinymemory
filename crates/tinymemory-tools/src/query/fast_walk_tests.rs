use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[tokio::test]
async fn missing_query_errors() {
    let err = run_fast_walk(json!({})).await.unwrap_err();
    assert!(err.to_string().contains("`query` is required"));
}

#[tokio::test]
async fn blank_query_errors() {
    let err = run_fast_walk(json!({"query": "   "})).await.unwrap_err();
    assert!(err.to_string().contains("`query` is required"));
}
