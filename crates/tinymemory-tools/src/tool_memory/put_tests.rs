use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parse_priority_defaults_to_normal() {
    assert_eq!(parse_priority(None), ToolMemoryPriority::Normal);
    assert_eq!(parse_priority(Some("normal")), ToolMemoryPriority::Normal);
    assert_eq!(parse_priority(Some("unknown")), ToolMemoryPriority::Normal);
}

#[test]
fn parse_priority_accepts_critical_and_high_case_insensitively() {
    assert_eq!(
        parse_priority(Some("critical")),
        ToolMemoryPriority::Critical
    );
    assert_eq!(
        parse_priority(Some("CRITICAL")),
        ToolMemoryPriority::Critical
    );
    assert_eq!(parse_priority(Some("high")), ToolMemoryPriority::High);
    assert_eq!(parse_priority(Some("HiGh")), ToolMemoryPriority::High);
}

#[test]
fn args_default_tags_to_empty() {
    let args: Args = serde_json::from_value(json!({
        "tool_name": "bash",
        "rule": "Never run rm -rf"
    }))
    .unwrap();
    assert_eq!(args.tool_name, "bash");
    assert_eq!(args.rule, "Never run rm -rf");
    assert!(args.priority.is_none());
    assert!(args.tags.is_empty());
}

#[test]
fn parameters_schema_describes_priority_enum() {
    let tool = MemoryToolsPutTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], json!(["tool_name", "rule"]));
    assert_eq!(
        schema["properties"]["priority"]["enum"],
        json!(["critical", "high", "normal"])
    );
}

#[tokio::test]
async fn execute_rejects_missing_required_fields() {
    let tool = MemoryToolsPutTool::new(NoHost);
    let err = tool
        .execute(json!({ "tool_name": "bash" }))
        .await
        .expect_err("missing rule should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tools_put"));

    let err = tool
        .execute(json!({ "rule": "Never run rm -rf" }))
        .await
        .expect_err("missing tool_name should fail");
    assert!(err
        .to_string()
        .contains("invalid arguments for memory_tools_put"));
}
