//! Tool rules over the hosted wire, in the layout hosts read directly.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{MemoryCore, MemoryToolMemory};
use tinymemory_api::tool_memory::{ToolMemoryPriority, ToolMemoryRule, ToolMemorySource};
use tinymemory_api::types::MemoryCategory;

use crate::cortex_provider::families::test_support::hosted;

fn rule(tool: &str, id: &str, priority: ToolMemoryPriority, text: &str) -> ToolMemoryRule {
    ToolMemoryRule {
        id: id.to_string(),
        ..ToolMemoryRule::new(tool, text, priority, ToolMemorySource::default())
    }
}

#[tokio::test]
async fn a_rule_lands_where_the_host_reads_it() {
    let (provider, _state) = hosted().await;
    provider
        .put_tool_rule(rule(
            " Shell ",
            "r1",
            ToolMemoryPriority::High,
            "quote paths",
        ))
        .await
        .expect("put");
    let entry = provider
        .get("tool-shell", "rule/r1")
        .await
        .expect("get")
        .expect("the rule is an ordinary keyed record");
    assert_eq!(
        entry.category,
        MemoryCategory::Custom("tool_memory".to_string())
    );
    let stored: ToolMemoryRule = serde_json::from_str(&entry.content).expect("rule json");
    assert_eq!(stored.tool_name, "shell");
    assert_eq!(stored.rule, "quote paths");
}

#[tokio::test]
async fn rules_sort_by_priority_then_by_the_latest_update() {
    let (provider, _state) = hosted().await;
    for (id, priority) in [
        ("normal-old", ToolMemoryPriority::Normal),
        ("critical", ToolMemoryPriority::Critical),
        ("high", ToolMemoryPriority::High),
        ("normal-new", ToolMemoryPriority::Normal),
    ] {
        provider
            .put_tool_rule(rule("git", id, priority, "a rule"))
            .await
            .expect("put");
    }
    let ids: Vec<String> = provider
        .tool_rules("git")
        .await
        .expect("rules")
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["critical", "high", "normal-new", "normal-old"]);
}

#[tokio::test]
async fn a_rewrite_keeps_the_rules_creation_time() {
    let (provider, _state) = hosted().await;
    let mut first = rule("git", "r1", ToolMemoryPriority::Normal, "one");
    first.created_at = "2026-01-01T00:00:00+00:00".to_string();
    provider.put_tool_rule(first).await.expect("put");
    provider
        .put_tool_rule(rule("git", "r1", ToolMemoryPriority::Normal, "two"))
        .await
        .expect("rewrite");
    let rules = provider.tool_rules("git").await.expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule, "two");
    assert_eq!(rules[0].created_at, "2026-01-01T00:00:00+00:00");
}

#[tokio::test]
async fn a_delete_names_its_tool() {
    let (provider, _state) = hosted().await;
    provider
        .put_tool_rule(rule("shell", "r1", ToolMemoryPriority::Normal, "x"))
        .await
        .expect("put");
    assert!(!provider.delete_tool_rule("git", "r1").await.expect("miss"));
    assert!(provider.delete_tool_rule("shell", "r1").await.expect("hit"));
    assert!(!provider
        .delete_tool_rule("shell", "r1")
        .await
        .expect("again"));
    assert!(provider
        .tool_rules("shell")
        .await
        .expect("rules")
        .is_empty());
}

#[tokio::test]
async fn a_rule_needs_a_tool_and_a_body_but_not_an_id() {
    let (provider, _state) = hosted().await;
    for bad in [
        rule("  ", "r1", ToolMemoryPriority::Normal, "x"),
        rule("shell", "r1", ToolMemoryPriority::Normal, "  "),
    ] {
        assert!(matches!(
            provider.put_tool_rule(bad).await,
            Err(MemoryError::Invalid(_))
        ));
    }
    provider
        .put_tool_rule(rule("shell", "", ToolMemoryPriority::Normal, "x"))
        .await
        .expect("an empty id is generated");
    let rules = provider.tool_rules("shell").await.expect("rules");
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].id.is_empty());
}

#[tokio::test]
async fn a_row_that_is_not_a_rule_is_skipped() {
    let (provider, _state) = hosted().await;
    provider
        .store(
            "tool-shell",
            "note",
            "not a rule",
            MemoryCategory::Core,
            None,
            tinymemory_api::types::MemoryTaint::Internal,
        )
        .await
        .expect("store");
    provider
        .store(
            "tool-shell",
            "rule/garbled",
            "{not json",
            MemoryCategory::Core,
            None,
            tinymemory_api::types::MemoryTaint::Internal,
        )
        .await
        .expect("store");
    assert!(provider
        .tool_rules("shell")
        .await
        .expect("rules")
        .is_empty());
}
