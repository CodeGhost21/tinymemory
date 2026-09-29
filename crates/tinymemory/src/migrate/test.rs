//! `migrate::copy` between two in-memory providers.

#![allow(clippy::expect_used)]

use tinymemory_conformance::InMemoryProvider;

use super::*;
use crate::provider::MemoryCore;
use crate::types::{MemoryCategory, MemoryTaint};

async fn seed(provider: &InMemoryProvider, count: usize) {
    for index in 0..count {
        provider
            .store(
                &format!("ns/{}", index % 3),
                &format!("key-{index}"),
                &format!("content {index}"),
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await
            .expect("seed store");
    }
}

#[tokio::test]
async fn copy_moves_every_record_and_reports_progress() {
    let source = InMemoryProvider::new();
    let target = InMemoryProvider::new();
    seed(&source, 7).await;

    let mut ticks = Vec::new();
    let report = copy(&source, &target, |p| ticks.push(p))
        .await
        .expect("copy succeeds");

    assert_eq!(report.records, 7);
    assert_eq!(report.imported, 7);
    assert_eq!(report.failed, 0);
    assert!(report.pages >= 1);
    assert_eq!(ticks.len(), report.pages, "one progress tick per page");
    assert_eq!(ticks.last().map(|p| p.records), Some(7));

    for index in 0..7 {
        let entry = target
            .get(&format!("ns/{}", index % 3), &format!("key-{index}"))
            .await
            .expect("get")
            .expect("record was copied");
        assert_eq!(entry.content, format!("content {index}"));
    }
    // Non-destructive: the source still holds everything.
    assert!(source.get("ns/0", "key-0").await.expect("get").is_some());
}

#[tokio::test]
async fn copying_twice_does_not_duplicate() {
    let source = InMemoryProvider::new();
    let target = InMemoryProvider::new();
    seed(&source, 4).await;
    copy(&source, &target, |_| {}).await.expect("first copy");
    let second = copy(&source, &target, |_| {}).await.expect("second copy");
    assert_eq!(second.records, 4);
    assert_eq!(target.list(None, None, None).await.expect("list").len(), 4);
}

#[tokio::test]
async fn copying_an_empty_source_is_a_clean_no_op() {
    let report = copy(&InMemoryProvider::new(), &InMemoryProvider::new(), |_| {})
        .await
        .expect("copy");
    assert_eq!(report.records, 0);
    assert_eq!(report.pages, 1);
}
