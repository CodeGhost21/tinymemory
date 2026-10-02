//! The [`MemoryEngine`] trait and the [`EngineDescriptor`] that advertises
//! what an engine offers.

use async_trait::async_trait;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::item::{StoreItem, StoreReceipt};
use crate::query::{
    FetchMode, FetchPage, FetchRequest, ForgetReport, ForgetTarget, ListPage, ListRequest,
    RecallAnswer, RecallRequest,
};

/// A memory engine: recall, fetch, store, forget and list over typed items.
///
/// Every method validates its request first (the `validate` methods on the
/// request types) so engines refuse malformed calls identically. A
/// [`FetchMode`] not listed in [`EngineDescriptor::fetch_modes`] fails with
/// [`Error::Unsupported`].
#[async_trait]
pub trait MemoryEngine: Send + Sync {
    /// What this engine is and offers.
    fn descriptor(&self) -> &EngineDescriptor;

    /// Whether the engine can serve right now.
    async fn health(&self) -> EngineHealth;

    /// Answers a question from stored items.
    ///
    /// # Errors
    ///
    /// Invalid requests, and the engine's own failures.
    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer>;

    /// Retrieves raw items matching a query.
    ///
    /// # Errors
    ///
    /// Invalid requests, [`Error::Unsupported`] for an undeclared mode, and
    /// the engine's own failures.
    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage>;

    /// Stores one item. Storing an identical item again is a replay.
    ///
    /// # Errors
    ///
    /// Invalid items, and the engine's own failures.
    async fn store(&self, item: StoreItem) -> Result<StoreReceipt>;

    /// Removes items by id or by a non-empty filter.
    ///
    /// # Errors
    ///
    /// An empty target, and the engine's own failures.
    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport>;

    /// Pages through stored items.
    ///
    /// # Errors
    ///
    /// Invalid requests, and the engine's own failures.
    async fn list(&self, req: ListRequest) -> Result<ListPage>;
}

/// What an engine is and offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngineDescriptor {
    /// Stable id used in configuration (`cortexdb`, `tinyhumans`).
    pub id: &'static str,
    /// Human-readable name.
    pub label: &'static str,
    /// One-sentence description.
    pub description: &'static str,
    /// Whether a third party runs the engine.
    pub hosted: bool,
    /// Whether configuration must name an endpoint.
    pub needs_endpoint: bool,
    /// Whether configuration must supply a credential.
    pub needs_key: bool,
    /// The endpoint used when configuration names none.
    pub default_endpoint: Option<&'static str>,
    /// The fetch modes the engine serves.
    pub fetch_modes: Vec<FetchMode>,
}

impl EngineDescriptor {
    /// Whether the engine serves `mode`.
    #[must_use]
    pub fn supports(&self, mode: FetchMode) -> bool {
        self.fetch_modes.contains(&mode)
    }

    /// Refuses a mode the engine does not serve.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] naming the mode and the engine.
    pub fn ensure_mode(&self, mode: FetchMode) -> Result<()> {
        if self.supports(mode) {
            Ok(())
        } else {
            Err(Error::Unsupported(format!(
                "engine `{}` does not offer {} fetch",
                self.id,
                mode.as_str()
            )))
        }
    }
}

/// Whether an engine can serve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum EngineHealth {
    /// Serving.
    Ok,
    /// Serving, impaired (rate limited, partially available).
    Degraded(String),
    /// Not serving.
    Down(String),
}

impl EngineHealth {
    /// Whether the engine is serving at all.
    #[must_use]
    pub fn is_serving(&self) -> bool {
        !matches!(self, Self::Down(_))
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
