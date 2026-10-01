//! High-level memory sync orchestration.
//!
//! This module owns the user-facing "sync my memory" workflow:
//!
//! 1. accept a manual or scheduled sync request
//! 2. emit coarse lifecycle events for UI visibility
//! 3. dispatch into the engine's sync backends
//! 4. rely on `memory_store` + `memory_queue` + `memory_tree` backends to
//!    persist, enqueue, ingest, and seal the resulting data
//!
//! The low-level provider implementations live in the engine crate; this module
//! is the orchestration seam the `memory` domain presents to RPC/tools/UI.
//!
//! It sits in the contract crate for the same reason [`crate::events`] does —
//! it is the vocabulary a host reads sync progress in, and emitting a stage is
//! a `publish` onto that bus. `tinymemory_core::sync_events` re-exports it.

use serde::{Deserialize, Serialize};

/// Why a sync run was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySyncTrigger {
    Manual,
    Cron,
}

impl MemorySyncTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Cron => "cron",
        }
    }
}

/// Coarse orchestration stages surfaced to the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySyncStage {
    Requested,
    Fetching,
    Stored,
    Queued,
    Ingesting,
    Completed,
    Failed,
}

impl MemorySyncStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Fetching => "fetching",
            Self::Stored => "stored",
            Self::Queued => "queued",
            Self::Ingesting => "ingesting",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// Publish a coarse sync lifecycle event for UI subscribers.
///
/// `source_id` is the originating `MemorySourceEntry.id` when this event
/// can be attributed to a specific memory-source row. Pass `None` for
/// non-memory-source sync paths (channel-provider syncs, etc.) to avoid
/// corrupting the per-row indicator on the frontend.
pub fn emit_sync_stage(
    trigger: MemorySyncTrigger,
    stage: MemorySyncStage,
    provider: Option<&str>,
    connection_id: Option<&str>,
    detail: Option<String>,
    source_id: Option<&str>,
) {
    log::debug!(
        "[memory-sync] emit stage={} trigger={} provider={:?} connection_id={:?} source_id={:?}",
        stage.as_str(),
        trigger.as_str(),
        provider,
        connection_id,
        source_id
    );
    crate::events::publish(crate::events::MemoryEvent::SyncStageChanged {
        trigger: trigger.as_str().to_string(),
        stage: stage.as_str().to_string(),
        provider: provider.map(str::to_string),
        connection_id: connection_id.map(str::to_string),
        detail,
        source_id: source_id.map(str::to_string),
    });
}

/// Extract the originating memory-source id from a composite `source_id` of
/// the form `"mem_src:<source_id>:<item_id>"` used by the reader-based ingest
/// path (folder, RSS, web-page sources).
///
/// The encoding is: `mem_src:` prefix, followed by the memory-source id (a
/// short alphanumeric slug, no colons), then `:`, then the item id (which
/// may contain colons, e.g. RSS GUIDs that are URLs like
/// `https://example.com/feed/1`).
///
/// Because the **source_id** is always the first colon-delimited segment after
/// `"mem_src:"`, we find the **first** colon — not the last — to extract it.
///
/// Returns `None` when the source_id is not in this format (e.g. channel-
/// provider syncs such as `"slack:workspace-1"`).
pub fn extract_mem_src_id(composite_source_id: &str) -> Option<&str> {
    let rest = composite_source_id.strip_prefix("mem_src:")?;
    // format: mem_src:<source_id>:<item_id>
    // source_id is a plain slug (no colons). item_id follows after the first colon.
    let colon_pos = rest.find(':')?;
    let source_id = &rest[..colon_pos];
    // Both halves must be non-empty. `"mem_src::item"` parses structurally but
    // names no source, and `Some("")` is not a source id any caller can use.
    //
    // This is a clarification, not a behaviour change: the one consumer is
    // `source_scope::chunk_source_allowed_in`, which tests the result against
    // an allowlist that `normalize` has already stripped empty entries from, so
    // `Some("")` and `None` both deny. Returning `None` says so at the parse
    // rather than relying on the allowlist to be empty-free.
    if source_id.is_empty() || colon_pos + 1 >= rest.len() {
        return None;
    }
    Some(source_id)
}

#[cfg(test)]
#[path = "sync_events_tests.rs"]
mod tests;
