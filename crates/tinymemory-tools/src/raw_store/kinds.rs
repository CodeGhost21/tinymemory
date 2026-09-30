//! `memory_store_kinds` — introspection. Enumerate every storage shape the
//! bound driver persists, so an agent can plan a fan-out without hard-coding.
//!
//! The catalog comes from the driver rather than from a compiled-in list: it is
//! the engine's own vocabulary, and a host-side copy drifts. This one had —
//! the description below used to advertise `content`, `document` and `graph`,
//! none of which exist, while omitting `raw` and `entity`, which do.

use async_trait::async_trait;
use serde_json::{json, Value};

use tinymemory_api::provider::MemoryProvider;
use crate::MemoryToolHost;
use tinytools::{Tool, ToolExposure, ToolResult};

pub struct MemoryStoreKindsTool<H> {
    host: H,
}

impl<H> MemoryStoreKindsTool<H> {
    /// A tool over `host`.
    pub fn new(host: H) -> Self {
        Self { host }
    }
}

impl<H: Default> Default for MemoryStoreKindsTool<H> {
    fn default() -> Self {
        Self::new(H::default())
    }
}

#[async_trait]
impl<H: MemoryToolHost> Tool for MemoryStoreKindsTool<H> {
    /// Superseded by the `memory` tool, which dispatches every memory
    /// operation on one `action` field. Kept registered and dispatchable so a
    /// replayed transcript or a saved skill naming `memory_*` keeps working;
    /// hidden from the wire so eleven schemas do not ship where one does.
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Hidden
    }

    fn name(&self) -> &str {
        "memory_store_kinds"
    }

    fn description(&self) -> &str {
        "Return the catalog of memory_store storage kinds the active memory \
         driver persists. No arguments. Use when planning a multi-kind \
         retrieval fan-out."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        log::debug!("[tool][memory_store] kinds start");
        let guard = self.host.provider()
            .await
            .map_err(|e| anyhow::anyhow!("memory_store_kinds: {e}"))?;
        let kinds = guard
            .as_chunks()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "memory_store_kinds: memory driver does not support the chunk family"
                )
            })?
            .storage_kinds()
            .await
            .map_err(|e| anyhow::anyhow!("memory_store_kinds: {e}"))?;
        log::debug!("[tool][memory_store] kinds success count={}", kinds.len());
        Ok(ToolResult::success(serde_json::to_string(
            &json!({ "kinds": kinds }),
        )?))
    }
}

