//! Tests for the surrounding module.

use super::*;
use std::path::Path;

fn twitter_source() -> MemorySourceEntry {
    MemorySourceEntry {
        id: "src_tw".into(),
        kind: SourceKind::TwitterQuery,
        label: "AI tweets".into(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        query: Some("AI safety".into()),
        since_days: Some(3),
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
async fn list_items_returns_not_configured_error() {
    let reader = TwitterReader;
    let result = reader.list_items(&twitter_source(), Path::new(".")).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet configured"));
}

#[tokio::test]
async fn blank_query_is_rejected() {
    let mut source = twitter_source();
    source.query = Some("   ".into());
    let error = TwitterReader
        .list_items(&source, Path::new("."))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "twitter source requires a non-empty query"
    );
}

#[tokio::test]
async fn read_item_is_not_configured() {
    let error = TwitterReader
        .read_item(&twitter_source(), "1", Path::new("."))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("bearer token"));
}
