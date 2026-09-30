//! A driver whose `recall` answers with a fixed entry list, whatever the query.
//!
//! # Why this is not [`InMemoryProvider`](super::InMemoryProvider)
//!
//! That one substring-matches, which is right for a round-trip test and wrong
//! for a test that scripts a specific result set — scored entries, an over-long
//! entry, ten entries to overflow a budget — and asserts on what the *caller*
//! does with it. Those tests are about rendering and filtering downstream of
//! recall, so recall itself has to be a constant.
//!
//! Everything else is inert: writes are accepted and dropped, reads answer
//! empty. A test needing real storage wants the in-memory reference driver.

use async_trait::async_trait;
use tinymemory_api::capabilities::Capabilities;
use tinymemory_api::error::MemoryError;
use tinymemory_api::health::MemoryHealth;
use tinymemory_api::provider::{
    ExportPage, ExportRecord, ImportOutcome, MemoryCore, MemoryPortability, MemoryProvider,
    MemoryRecall, SourceScope,
};
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::types::{MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary};

/// The driver id [`FixedRecallProvider`] binds under.
pub const FIXED_RECALL_DRIVER_ID: &str = "fixed-recall";

/// A provider whose `recall` returns the entries it was built with.
#[derive(Debug, Clone)]
pub struct FixedRecallProvider {
    entries: Vec<MemoryEntry>,
}

impl FixedRecallProvider {
    /// Builds a driver whose `recall` answers `entries`, whatever the query.
    #[must_use]
    pub fn new(entries: Vec<MemoryEntry>) -> Self {
        Self { entries }
    }
}

#[async_trait]
impl MemoryCore for FixedRecallProvider {
    async fn store(
        &self,
        _namespace: &str,
        _key: &str,
        _content: &str,
        _category: MemoryCategory,
        _session_id: Option<&str>,
        _taint: MemoryTaint,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn get(&self, _namespace: &str, _key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        Ok(None)
    }

    async fn forget(&self, _namespace: &str, _key: &str) -> Result<bool, MemoryError> {
        Ok(false)
    }

    async fn list(
        &self,
        _namespace: Option<&str>,
        _category: Option<&MemoryCategory>,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(Vec::new())
    }

    async fn namespaces(&self) -> Result<Vec<NamespaceSummary>, MemoryError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl MemoryRecall for FixedRecallProvider {
    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _opts: &OwnedRecallOpts,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(self.entries.clone())
    }
}

#[async_trait]
impl MemoryPortability for FixedRecallProvider {
    async fn export_page(
        &self,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<ExportPage, MemoryError> {
        Err(MemoryError::Other(anyhow::anyhow!(
            "FixedRecallProvider does not implement export"
        )))
    }

    async fn import_records(
        &self,
        _records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError> {
        Err(MemoryError::Other(anyhow::anyhow!(
            "FixedRecallProvider does not implement import"
        )))
    }
}

#[async_trait]
impl MemoryProvider for FixedRecallProvider {
    fn driver_id(&self) -> &str {
        FIXED_RECALL_DRIVER_ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::mandatory()
    }

    async fn health(&self) -> MemoryHealth {
        MemoryHealth::Ready
    }
}

#[cfg(test)]
mod tests {
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
            .store("ns", "k", "v", MemoryCategory::Core, None, MemoryTaint::Internal)
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
}
