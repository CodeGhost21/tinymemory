//! A host with no memory bound, for unit tests of validation and wording that
//! must hold before any driver is reached.

use std::sync::Arc;

use async_trait::async_trait;
use tinymemory_api::provider::MemoryProvider;
use tinytools::ToolResult;

use crate::{MemoryToolHost, QueryEmbedder};

/// Every call that needs a driver fails with [`NO_DRIVER`].
#[derive(Clone, Copy, Default)]
pub(crate) struct NoHost;

/// What [`NoHost`] reports when a tool asks it for a driver.
pub(crate) const NO_DRIVER: &str = "no memory driver is bound";

#[async_trait]
impl MemoryToolHost for NoHost {
    async fn provider(&self) -> Result<Arc<dyn MemoryProvider>, String> {
        Err(NO_DRIVER.to_string())
    }

    fn chunk_source_allowed(&self, _tags: &[String], _source_id: &str) -> bool {
        true
    }

    async fn embedder(&self) -> Result<Box<dyn QueryEmbedder>, String> {
        Err(format!("load config failed: {NO_DRIVER}"))
    }

    async fn ingest_document(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Err(anyhow::anyhow!("ingest_document: {NO_DRIVER}"))
    }
}
