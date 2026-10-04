//! Composio source reader — a placeholder over the provider pipeline.
//!
//! Composio data does not arrive item by item: the host runs toolkit actions
//! with its credentials and hands the responses to [`crate::sources::composio`], which
//! normalises them and maps them to `StoreItem`s. For a Composio source,
//! `list_items` returns the connection as one sync target and `read_item`
//! describes that pipeline. The reader exists so the registry can query every
//! source kind uniformly.

use std::path::Path;

use async_trait::async_trait;

use super::SourceReader;
use crate::sources::error::Result;
use crate::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};

/// Lists a Composio connection as a single sync target.
///
/// Composio data arrives through the provider sync pipeline rather than
/// item-by-item, so `read_item` returns a description of that rather than
/// content. The reader exists so the registry can query every source kind
/// uniformly.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComposioReader;

#[async_trait]
impl SourceReader for ComposioReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Composio
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        _workspace: &Path,
    ) -> Result<Vec<SourceItem>> {
        let toolkit = source.toolkit.as_deref().unwrap_or("unknown");
        let connection_id = source.connection_id.as_deref().unwrap_or("unknown");

        log::debug!(
            "[memory_sources:composio] list_items toolkit={toolkit} connection_id={connection_id}"
        );

        Ok(vec![SourceItem {
            id: connection_id.to_string(),
            title: format!("{toolkit} connection"),
            updated_at_ms: None,
        }])
    }

    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        _workspace: &Path,
    ) -> Result<SourceContent> {
        let toolkit = source.toolkit.as_deref().unwrap_or("unknown");
        Ok(SourceContent {
            id: item_id.to_string(),
            title: format!("{toolkit} sync data"),
            body: format!(
                "Composio {toolkit} data is synced via the provider sync pipeline, not read item-by-item."
            ),
            content_type: ContentType::Plaintext,
            metadata: serde_json::json!({
                "toolkit": toolkit,
                "connection_id": source.connection_id,
            }),
        })
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
