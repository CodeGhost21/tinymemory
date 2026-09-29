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

/// A source whose export cursor never advances.
struct Looping(InMemoryProvider);

#[async_trait::async_trait]
impl MemoryCore for Looping {
    async fn store(
        &self,
        n: &str,
        k: &str,
        c: &str,
        cat: MemoryCategory,
        s: Option<&str>,
        t: MemoryTaint,
    ) -> Result<(), crate::error::MemoryError> {
        self.0.store(n, k, c, cat, s, t).await
    }
    async fn get(
        &self,
        n: &str,
        k: &str,
    ) -> Result<Option<crate::types::MemoryEntry>, crate::error::MemoryError> {
        self.0.get(n, k).await
    }
    async fn forget(&self, n: &str, k: &str) -> Result<bool, crate::error::MemoryError> {
        self.0.forget(n, k).await
    }
    async fn list(
        &self,
        n: Option<&str>,
        c: Option<&MemoryCategory>,
        s: Option<&str>,
    ) -> Result<Vec<crate::types::MemoryEntry>, crate::error::MemoryError> {
        self.0.list(n, c, s).await
    }
    async fn namespaces(
        &self,
    ) -> Result<Vec<crate::types::NamespaceSummary>, crate::error::MemoryError> {
        self.0.namespaces().await
    }
}

#[async_trait::async_trait]
impl crate::provider::MemoryRecall for Looping {
    async fn recall(
        &self,
        q: &str,
        l: usize,
        o: &crate::recall::OwnedRecallOpts,
        s: Option<&crate::provider::types::SourceScope>,
    ) -> Result<Vec<crate::types::MemoryEntry>, crate::error::MemoryError> {
        self.0.recall(q, l, o, s).await
    }
}

#[async_trait::async_trait]
impl crate::provider::MemoryPortability for Looping {
    async fn export_page(
        &self,
        _cursor: Option<&str>,
        limit: usize,
    ) -> Result<crate::provider::types::ExportPage, crate::error::MemoryError> {
        let mut page = self.0.export_page(None, limit).await?;
        page.next_cursor = Some("stuck".to_string());
        Ok(page)
    }
    async fn import_records(
        &self,
        r: Vec<crate::provider::types::ExportRecord>,
    ) -> Result<crate::provider::types::ImportOutcome, crate::error::MemoryError> {
        self.0.import_records(r).await
    }
}

#[async_trait::async_trait]
impl MemoryProvider for Looping {
    fn driver_id(&self) -> &str {
        "looping"
    }
    fn capabilities(&self) -> crate::capabilities::Capabilities {
        crate::capabilities::Capabilities::mandatory()
    }
    async fn health(&self) -> crate::health::MemoryHealth {
        crate::health::MemoryHealth::Ready
    }
}

#[tokio::test]
async fn a_repeating_cursor_is_refused_rather_than_followed() {
    let source = Looping(InMemoryProvider::new());
    seed(&source.0, 2).await;
    let target = InMemoryProvider::new();
    let error = copy(&source, &target, |_| {})
        .await
        .expect_err("a cursor that never advances must not loop");
    assert!(error.to_string().contains("cursor repeated"), "{error}");
}
