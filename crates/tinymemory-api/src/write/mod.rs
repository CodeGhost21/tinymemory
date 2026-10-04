//! How long a store waits before it returns: [`WriteOptions`].
//!
//! [`MemoryEngine::store`](crate::MemoryEngine::store) returns only once the
//! item is readable, which is what imports, tools and tests want. An agent's
//! live turn wants the opposite: the write must be durable, but the turn must
//! not wait for indexing. [`MemoryEngine::store_with`](crate::MemoryEngine::store_with)
//! takes a [`WaitFor`] to choose:
//!
//! - [`WaitFor::Visible`] (the default) — exactly `store`: listed, and ranked
//!   by fetch and recall, on return.
//! - [`WaitFor::Accepted`] — the engine has durably accepted the item; it
//!   becomes listable and ranked shortly after. Replay detection still runs
//!   against what is already readable, so two identical `Accepted` stores in
//!   quick succession may both write.
//!
//! An engine without a cheaper acknowledgement serves `Accepted` as
//! `Visible`, which is always correct, only slower.
//!
//! # Example
//!
//! ```ignore
//! use tinymemory_api::{MemoryEngine, MemoryMeta, StoreItem, WaitFor, WriteOptions};
//! use tinymemory_api::conformance::ReferenceEngine;
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let engine = ReferenceEngine::new();
//! let item = StoreItem::document("logged without waiting", MemoryMeta::default());
//! let receipt = engine.store_with(item, WriteOptions::accepted()).await?;
//! assert!(!receipt.replayed);
//! assert_eq!(WriteOptions::default().wait, WaitFor::Visible);
//! # Ok::<(), tinymemory_api::Error>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};

/// How far a store goes before it returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitFor {
    /// The engine durably accepted the item; reads may lag a moment behind.
    Accepted,
    /// The item is listed and ranked on return, as [`crate::MemoryEngine::store`].
    #[default]
    Visible,
}

/// Options for [`crate::MemoryEngine::store_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WriteOptions {
    /// How far the store goes before it returns.
    #[serde(default)]
    pub wait: WaitFor,
}

impl WriteOptions {
    /// Return once the engine accepted the item: the hot-path write.
    #[must_use]
    pub fn accepted() -> Self {
        Self {
            wait: WaitFor::Accepted,
        }
    }

    /// Return once the item is readable: what `store` does.
    #[must_use]
    pub fn visible() -> Self {
        Self {
            wait: WaitFor::Visible,
        }
    }
}
