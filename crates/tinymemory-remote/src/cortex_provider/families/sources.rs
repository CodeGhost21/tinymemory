//! `MemorySourceSink` over the hosted wire.
//!
//! A synced item is a content record, keyed `item:<source_id>:<item_id>`, in
//! one of three namespaces chosen by where the item came from. Only this
//! family writes those namespaces, so every record in them is labelled by key
//! and by source.
//!
//! A batch costs what the backend's rate limit can bear:
//!
//! - **Lookups.** One listing per 40 items finds what is already held. An
//!   unchanged item is skipped rather than rewritten.
//! - **Writes.** Each write waits its turn at a pacing gate shared by every
//!   concurrent batch.
//! - **Visibility.** The batch waits once, for its last write to be listed,
//!   rather than once per item. The log is ordered, so the last write
//!   becoming listable means the earlier ones are.
//! - **Retiring.** Only then does it retire the versions its writes replaced.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::lock::Mutex;
use tinymemory_api::capabilities::Capability;
use tinymemory_api::chunks::SourceKind;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::types::{ForgetOutcome, ForgetSelector, IngestOutcome, SourceItem};
use tinymemory_api::provider::MemorySourceSink;
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::records::{newest_live, Place, Provenance, Record, Records, Version, SUPERSEDED};
use crate::cortex::AppendedEvent;
use crate::cortex_provider::CortexProvider;

/// The gap between two synced writes: about 200 a minute, leaving the rest of
/// the backend's 300-a-minute limit to lookups, polls and chat.
pub(super) const SOURCE_PACING: Duration = Duration::from_millis(300);

/// The audit note on removing a source's items.
const SOURCE_REMOVED: &str = "tinymemory: source removed";

/// One write at a time, at least one gap apart, across every batch.
///
/// The gate is held across the wait, so concurrent batches queue behind each
/// other rather than each spending the rate limit as if it were alone.
#[derive(Debug)]
pub(crate) struct Pacing {
    gap: Duration,
    next: Mutex<Option<Instant>>,
}

impl Pacing {
    pub(crate) fn new(gap: Duration) -> Self {
        Self {
            gap,
            next: Mutex::new(None),
        }
    }

    /// Returns once this write may go, and books the next slot.
    async fn wait(&self) {
        let mut next = self.next.lock().await;
        if let Some(at) = *next {
            let now = Instant::now();
            if at > now {
                tokio::time::sleep(at - now).await;
            }
        }
        *next = Some(Instant::now() + self.gap);
    }
}

/// Where items from `source_id` of `source_kind` land.
///
/// Mail and chat come from a Composio toolkit named by the first part of the
/// source id; everything else is a document.
fn placement(source_id: &str, source_kind: &str) -> SourceKind {
    if source_kind != "composio" {
        return SourceKind::Document;
    }
    match source_id.split(':').next().unwrap_or_default() {
        "gmail" | "outlook" => SourceKind::Email,
        "slack" | "discord" | "telegram" | "whatsapp" => SourceKind::Chat,
        _ => SourceKind::Document,
    }
}

/// The namespace a kind of source lands in.
fn namespace_of(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Chat => "sources/chat",
        SourceKind::Email => "sources/email",
        SourceKind::Document => "sources/documents",
    }
}

/// The text the engine learns from: the title, unless the content already
/// opens with it, then the content.
fn item_text(item: &SourceItem) -> String {
    let title = item.title.trim();
    if title.is_empty() || item.content.trim_start().starts_with(title) {
        item.content.clone()
    } else {
        format!("{title}\n\n{}", item.content)
    }
}

/// When the item was last true, as the engine's `observed_at`.
fn observed_at(item: &SourceItem) -> Option<String> {
    let millis = item.updated_at_ms?;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis).map(|at| at.to_rfc3339())
}

/// `error`, in the class it already had, saying how far the batch got. The
/// progress goes after the message so a backend code prefix, which callers
/// read, stays first.
fn with_progress(error: MemoryError, written: u32, total: usize) -> MemoryError {
    let say = |message: String| {
        format!("{message} (hosted memory had accepted {written} of {total} source items)")
    };
    match error {
        MemoryError::BudgetExceeded(message) => MemoryError::BudgetExceeded(say(message)),
        MemoryError::Unauthorized(message) => MemoryError::Unauthorized(say(message)),
        MemoryError::Invalid(message) => MemoryError::Invalid(say(message)),
        MemoryError::Unavailable(message) => MemoryError::Unavailable(say(message)),
        MemoryError::Unreachable(message) => MemoryError::Unreachable(say(message)),
        MemoryError::Timeout(message) => MemoryError::Timeout(say(message)),
        other => MemoryError::Backend(say(other.to_string())),
    }
}

impl CortexProvider {
    fn source_place(&self, kind: SourceKind) -> Result<Place, MemoryError> {
        Place::family_namespace(&self.dialect, namespace_of(kind)).map_err(engine_error)
    }

    /// Removes every item of `source_id` in the namespace of each of `kinds`,
    /// answering how many were live.
    async fn remove_source(
        &self,
        source_id: &str,
        kinds: &[SourceKind],
    ) -> Result<u64, MemoryError> {
        let records = Records::new(&self.dialect);
        let mut removed = 0;
        for kind in kinds {
            let place = self.source_place(*kind)?;
            let versions = records
                .of_source(&place, source_id)
                .await
                .map_err(engine_error)?;
            removed += live_keys(&versions);
            let ids: Vec<String> = versions.into_iter().map(|v| v.event_id).collect();
            self.dialect
                .forget_event_ids(&place.scope, &ids, SOURCE_REMOVED)
                .await
                .map_err(engine_error)?;
        }
        Ok(removed)
    }

    /// Removes the item whose event is `chunk_id`, every version of it, when
    /// that event is one of this account's synced items.
    async fn remove_chunk(&self, chunk_id: &str) -> Result<u64, MemoryError> {
        let Some(event) = self
            .dialect
            .event_by_id(chunk_id)
            .await
            .map_err(engine_error)?
        else {
            return Ok(0);
        };
        // The route is unpinned: another account's event comes back with its
        // scope nulled, and it is never ours to touch.
        let Some(scope) = event.get("scope").and_then(serde_json::Value::as_str) else {
            return Ok(0);
        };
        let Some(version) = Version::of(&event) else {
            return Ok(0);
        };
        for kind in [SourceKind::Chat, SourceKind::Email, SourceKind::Document] {
            let place = self.source_place(kind)?;
            if place.scope != scope {
                continue;
            }
            let records = Records::new(&self.dialect);
            let versions = records
                .versions(&place, &[&version.record.key])
                .await
                .map_err(engine_error)?
                .remove(&version.record.key)
                .unwrap_or_default();
            let live = u64::from(newest_live(&versions).is_some());
            let ids: Vec<String> = versions.into_iter().map(|v| v.event_id).collect();
            self.dialect
                .forget_event_ids(&place.scope, &ids, SOURCE_REMOVED)
                .await
                .map_err(engine_error)?;
            return Ok(live);
        }
        Ok(0)
    }
}

/// How many distinct keys among `versions` have a live newest version.
fn live_keys(versions: &[Version]) -> u64 {
    let keys: HashSet<&str> = versions.iter().map(|v| v.record.key.as_str()).collect();
    keys.into_iter()
        .filter(|key| {
            let of_key: Vec<Version> = versions
                .iter()
                .filter(|v| v.record.key == *key)
                .cloned()
                .collect();
            newest_live(&of_key).is_some()
        })
        .count() as u64
}

#[async_trait]
impl MemorySourceSink for CortexProvider {
    async fn accept_source_items(
        &self,
        source_id: &str,
        source_kind: &str,
        items: Vec<SourceItem>,
        taint: MemoryTaint,
    ) -> Result<IngestOutcome, MemoryError> {
        if source_id.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "a source batch needs a source id".to_string(),
            ));
        }
        let total = items.len();
        let place = self.source_place(placement(source_id, source_kind))?;
        let mut outcome = IngestOutcome::default();
        let mut batch: Vec<(Record, Option<String>)> = Vec::new();
        for item in &items {
            if item.content.trim().is_empty() {
                outcome.skipped += 1;
                continue;
            }
            let record = Record {
                key: format!("item:{source_id}:{}", item.item_id),
                content: item_text(item),
                category: MemoryCategory::Core,
                session_id: None,
                taint,
                provenance: Provenance {
                    source: Some(source_id.to_string()),
                    reference: Some(item.url.clone().unwrap_or_else(|| item.item_id.clone())),
                    document: None,
                },
            };
            batch.push((record, observed_at(item)));
        }
        let records = Records::new(&self.dialect);
        let keys: Vec<&str> = batch.iter().map(|(r, _)| r.key.as_str()).collect();
        let held = records
            .versions(&place, &keys)
            .await
            .map_err(|error| with_progress(engine_error(error), 0, total))?;
        let mut replaced: Vec<Version> = Vec::new();
        let mut last: Option<AppendedEvent> = None;
        let mut unchanged = 0;
        for (record, observed) in &batch {
            let versions = held.get(&record.key).cloned().unwrap_or_default();
            if let Some(live) = newest_live(&versions) {
                if live.record == *record {
                    unchanged += 1;
                    outcome.skipped += 1;
                    outcome.ids.push(live.event_id.clone());
                    continue;
                }
            }
            self.families.pacing.wait().await;
            let event = records
                .append(&place, record, observed.clone(), false)
                .await
                .map_err(|error| with_progress(engine_error(error), outcome.written, total))?;
            outcome.written += 1;
            replaced.extend(versions);
            if let Some(event) = event {
                outcome.ids.push(event.id.clone());
                last = Some(event);
            }
        }
        if let Some(event) = last {
            self.dialect
                .await_listed(&event.scope, &event.id)
                .await
                .map_err(|error| with_progress(engine_error(error), outcome.written, total))?;
        }
        records.retire(&place, &replaced, SUPERSEDED).await;
        outcome.already_ingested = outcome.written == 0 && unchanged > 0;
        Ok(outcome)
    }

    async fn forget_source(&self, source_id: &str) -> Result<u64, MemoryError> {
        self.remove_source(
            source_id,
            &[SourceKind::Chat, SourceKind::Email, SourceKind::Document],
        )
        .await
    }

    async fn forget_matching(
        &self,
        selector: &ForgetSelector,
    ) -> Result<ForgetOutcome, MemoryError> {
        let chunks_removed = match selector {
            ForgetSelector::Source {
                source_kind,
                source_id,
            } => {
                let kind = SourceKind::parse(source_kind).map_err(MemoryError::Invalid)?;
                self.remove_source(source_id, &[kind]).await?
            }
            ForgetSelector::Chunk { chunk_id } => self.remove_chunk(chunk_id).await?,
            ForgetSelector::SourcePrefix { .. } | ForgetSelector::Owner { .. } => {
                return Err(MemoryError::unsupported(Capability::Sources));
            }
        };
        Ok(ForgetOutcome {
            chunks_removed,
            trees_cleaned: 0,
        })
    }
}

#[cfg(test)]
#[path = "sources_test.rs"]
mod test;
