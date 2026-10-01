use super::*;
use std::sync::Arc;

fn entry(content: &str) -> MemoryEntry {
    MemoryEntry {
        id: "id".into(),
        key: "key".into(),
        content: content.into(),
        namespace: Some("ns".into()),
        category: MemoryCategory::Core,
        timestamp: "2026-01-01T00:00:00Z".into(),
        session_id: None,
        score: None,
        taint: MemoryTaint::Internal,
    }
}

#[tokio::test]
async fn recall_ignores_the_query_and_returns_the_fixed_entries() {
    let driver = FixedRecallProvider::new(vec![entry("a"), entry("b")]);
    let opts = OwnedRecallOpts::default();
    let hits = driver.recall("anything", 1, &opts, None).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].content, "a");
}

#[tokio::test]
async fn writes_are_dropped_and_reads_answer_empty() {
    let driver = FixedRecallProvider::new(vec![]);
    driver
        .store(
            "ns",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .unwrap();
    assert!(driver.get("ns", "k").await.unwrap().is_none());
    assert!(driver.list(None, None, None).await.unwrap().is_empty());
    assert!(driver.namespaces().await.unwrap().is_empty());
    assert!(!driver.forget("ns", "k").await.unwrap());
}

#[tokio::test]
async fn advertises_only_the_mandatory_families_and_is_ready() {
    let driver: Arc<dyn MemoryProvider> = Arc::new(FixedRecallProvider::new(vec![]));
    assert_eq!(driver.driver_id(), FIXED_RECALL_DRIVER_ID);
    assert_eq!(driver.capabilities(), Capabilities::mandatory());
    assert!(matches!(driver.health().await, MemoryHealth::Ready));
    assert!(driver.export_page(None, 1).await.is_err());
    assert!(driver.import_records(vec![]).await.is_err());
}
