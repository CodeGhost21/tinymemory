//! Tests for the surrounding module.

use super::*;
use std::path::Path;

fn test_source() -> MemorySourceEntry {
    MemorySourceEntry {
        id: "src_1".into(),
        kind: SourceKind::Composio,
        label: "Gmail".into(),
        enabled: true,
        toolkit: Some("gmail".into()),
        connection_id: Some("cmp_123".into()),
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_items: None,
        max_commits: None,
        max_issues: None,
        max_prs: None,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: None,
    }
}

#[tokio::test]
async fn list_items_returns_connection_as_item() {
    let reader = ComposioReader;
    let items = reader
        .list_items(&test_source(), Path::new("."))
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "cmp_123");
}

#[tokio::test]
async fn read_item_describes_the_provider_pipeline() {
    let content = ComposioReader
        .read_item(&test_source(), "cmp_123", Path::new("."))
        .await
        .unwrap();
    assert_eq!(content.title, "gmail sync data");
    assert!(content.body.contains("provider sync pipeline"));
    assert_eq!(content.metadata["toolkit"], "gmail");
    assert_eq!(content.metadata["connection_id"], "cmp_123");
}
