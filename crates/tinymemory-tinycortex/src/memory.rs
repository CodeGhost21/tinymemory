//! [`TinycortexMemory`] — a TinyCortex storage backend, seen through the
//! TinyMemory contract's [`Memory`] trait.
//!
//! Every method is a plain delegation. It used to be a delegation *plus a
//! conversion*, because the engine's contract crate defined its own copies of
//! the memory value types; since tinymemory#18 §A1 `tinycortex-api` re-exports
//! this contract instead, so the two sides name one type and there is nothing
//! left to convert. The one method that is still not purely mechanical is
//! called out below.

use std::sync::Arc;

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::traits::Memory;
use tinymemory_api::types::{
    MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary, RecallOpts,
};

/// A TinyCortex backend exposed as a TinyMemory [`Memory`].
pub struct TinycortexMemory {
    inner: Arc<dyn tinycortex::memory::Memory>,
}

impl std::fmt::Debug for TinycortexMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The backend is not `Debug` and may hold a path or a connection
        // string; its name is the part that is safe to render.
        f.debug_struct("TinycortexMemory")
            .field("backend", &self.inner.name())
            .finish()
    }
}

impl TinycortexMemory {
    /// Wrap a TinyCortex backend.
    #[must_use]
    pub fn new(inner: Arc<dyn tinycortex::memory::Memory>) -> Self {
        Self { inner }
    }

    /// The wrapped backend, for a caller that still needs engine-native access.
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn tinycortex::memory::Memory> {
        &self.inner
    }
}

#[async_trait]
impl Memory for TinycortexMemory {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // §A4: the engine refuses empty content with an opaque anyhow
        // (vendor/tinycortex, outside this repo's reach), which the mandatory
        // composition can only flatten to `Other` — indistinguishable from a
        // backend failure. Enforce the same documented rule HERE, typed, so
        // the refusal arrives as the `Invalid` the conformance suite now
        // requires. Not message-sniffing: this mirrors the engine's stated
        // contract, it does not parse its prose.
        if content.trim().is_empty() {
            return Err(anyhow::Error::new(MemoryError::Invalid(
                "memory content cannot be empty".to_string(),
            )));
        }
        self.inner
            .store(namespace, key, content, category, session_id)
            .await
    }

    /// **Overridden, and it must stay overridden.** The trait default degrades
    /// to [`Memory::store`], which drops the taint — so a backend reached
    /// through the default would launder externally-sourced content into
    /// internal-trust content. Forwarding to the engine's own
    /// `store_with_taint` keeps provenance intact end to end.
    async fn store_with_taint(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: MemoryTaint,
    ) -> anyhow::Result<()> {
        // Same typed refusal as `store` — see the comment there.
        if content.trim().is_empty() {
            return Err(anyhow::Error::new(MemoryError::Invalid(
                "memory content cannot be empty".to_string(),
            )));
        }
        self.inner
            .store_with_taint(namespace, key, content, category, session_id, taint)
            .await
    }

    /// The engine's `RecallOpts` borrows its string fields, so the owned form
    /// has to outlive the borrow taken from it — hence the local binding rather
    /// than a temporary in the call.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let owned = OwnedRecallOpts::from(opts);
        Ok(self.inner.recall(query, limit, (&owned).into()).await?)
    }

    async fn recall_relevant_by_vector(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
        min_vector_similarity: f64,
    ) -> anyhow::Result<Vec<(String, String)>> {
        self.inner
            .recall_relevant_by_vector(namespace, query, limit, min_vector_similarity)
            .await
    }

    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        self.inner.get(namespace, key).await
    }

    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        self.inner.list(namespace, category, session_id).await
    }

    async fn forget(&self, namespace: &str, key: &str) -> anyhow::Result<bool> {
        self.inner.forget(namespace, key).await
    }

    async fn namespace_summaries(&self) -> anyhow::Result<Vec<NamespaceSummary>> {
        self.inner.namespace_summaries().await
    }

    async fn count(&self) -> anyhow::Result<usize> {
        self.inner.count().await
    }

    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod test;
