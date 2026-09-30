use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn args_require_tool_name() {
    let args: Args = serde_json::from_value(json!({ "tool_name": "bash" })).unwrap();
    assert_eq!(args.tool_name, "bash");
}

#[test]
fn parameters_schema_requires_tool_name() {
    let tool = MemoryToolsListTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["tool_name"]));
    assert_eq!(schema["properties"]["tool_name"]["type"], "string");
}

#[tokio::test]
async fn execute_rejects_missing_tool_name() {
    let tool = MemoryToolsListTool::new(NoHost);
    let err = tool
        .execute(json!({}))
        .await
        .expect_err("missing tool_name should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tools_list"));
}
