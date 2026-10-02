//! `MemoryIngest` over the hosted wire: documents, chat and mail written as
//! records the engine learns from.
//!
//! An item that names no namespace lands in its kind's source namespace —
//! `sources/chat`, `sources/email` or `sources/documents` — beside what the
//! source sink writes. So one recall across `sources` searches everything
//! ingested, the Brain view's forest is derived from it, and its leaves list
//! it. An item that names a namespace lands there, as a record of that
//! namespace. Each record carries its source id as provenance and as a lookup
//! label, which is how retrieval narrows to the sources a caller may see.
//!
//! # Keys
//!
//! A document keeps one key per source, `document:{source_id}`, and a new
//! version retires the one it replaced. Every message gets a key of its own,
//! `message:{source_id}:{digest}`, from everything the message carries: a host
//! may send every batch of one conversation under one source id — the
//! archivist does, for every session — and two messages sharing a key would
//! fold into one record. A message resent unchanged has the same key and is
//! not written again.
//!
//! # Cost
//!
//! The backend has no bulk route, so a batch is one write per message, paced
//! by the same gate as synced items, then one wait for the last.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::types::{IngestItem, IngestOutcome};
use tinymemory_api::provider::MemoryIngest;
use tinymemory_api::types::MemoryCategory;

use super::records::{newest_live, Place, Provenance, Record, Records, SUPERSEDED};
use super::sources::namespace_of as source_namespace;
use crate::cortex_labels;
use crate::cortex_provider::CortexProvider;

/// Whether a message's text should say who wrote it: not when the author is
/// one of the roles a conversation already states.
fn names_a_person(author: &str) -> bool {
    !matches!(
        author.trim().to_ascii_lowercase().as_str(),
        "user" | "assistant" | "tool" | "system"
    )
}

fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// The text the engine learns from: the item's content, under the header
/// lines that say who and what it is about — the lines the embedded engine
/// writes for the same item — when it has any.
pub(super) fn item_text(item: &IngestItem) -> String {
    let mut headers = Vec::new();
    if let Some(author) = present(item.author.as_deref()).filter(|a| names_a_person(a)) {
        headers.push(format!("From: {author}"));
    }
    if !item.to.is_empty() {
        headers.push(format!("To: {}", item.to.join(", ")));
    }
    if !item.cc.is_empty() {
        headers.push(format!("Cc: {}", item.cc.join(", ")));
    }
    if let Some(subject) = present(item.subject.as_deref()) {
        headers.push(format!("Subject: {subject}"));
    }
    if let Some(unsubscribe) = present(item.list_unsubscribe.as_deref()) {
        headers.push(format!("List-Unsubscribe: {unsubscribe}"));
    }
    if headers.is_empty() {
        return item.content.clone();
    }
    format!("{}\n\n{}", headers.join("\n"), item.content)
}

/// The namespace an item lands in: the one it names, else its kind's.
fn namespace_for(item: &IngestItem) -> String {
    item.namespace
        .clone()
        .unwrap_or_else(|| source_namespace(item.source.kind()).to_string())
}

/// A message's key, from everything it carries.
fn message_key(item: &IngestItem) -> Result<String, MemoryError> {
    let seed = serde_json::to_string(item)?;
    Ok(format!(
        "message:{}:{}",
        item.source_id,
        cortex_labels::digest(&seed)
    ))
}

/// One item as the record it is written as.
fn record(
    item: &IngestItem,
    key: String,
    category: MemoryCategory,
    session: Option<&str>,
) -> Record {
    Record {
        key,
        content: item_text(item),
        category,
        session_id: session.map(str::to_string),
        taint: item.taint,
        provenance: Provenance {
            source: Some(item.source_id.clone()),
            reference: item.source_ref.as_ref().map(|r| r.value.clone()),
            document: None,
        },
    }
}

impl CortexProvider {
    /// Where ingested items in `namespace` are written. Every ingested record
    /// is labelled, so a lookup here never walks the scope for a key it
    /// missed.
    fn ingest_place(&self, namespace: &str) -> Result<Place, MemoryError> {
        Place::family_namespace(&self.dialect, namespace)
            .map_err(|error| MemoryError::Invalid(format!("{error:#}")))
    }

    /// Writes one conversation or thread, in order, one record per message,
    /// skipping each message already held unchanged.
    async fn ingest_messages(
        &self,
        messages: Vec<IngestItem>,
        what: &str,
        category: MemoryCategory,
    ) -> Result<IngestOutcome, MemoryError> {
        let Some(first) = messages.first() else {
            return Ok(IngestOutcome::default());
        };
        if first.source_id.trim().is_empty()
            || messages.iter().any(|message| {
                message.source_id != first.source_id || message.content.trim().is_empty()
            })
        {
            return Err(MemoryError::Invalid(format!(
                "{what} batches must contain one non-empty {what}"
            )));
        }
        let namespace = namespace_for(first);
        if messages
            .iter()
            .any(|message| namespace_for(message) != namespace)
        {
            return Err(MemoryError::Invalid(format!(
                "{what} batches must use one namespace"
            )));
        }
        let place = self.ingest_place(&namespace)?;
        let chat = category == MemoryCategory::Conversation;
        let mut batch = Vec::with_capacity(messages.len());
        for message in &messages {
            // A chat message belongs to the session that owns it, so recall
            // can leave the current session's own words out.
            let session = present(Some(&message.owner)).filter(|_| chat);
            batch.push((
                record(message, message_key(message)?, category.clone(), session),
                message.timestamp.map(|stamp| stamp.to_rfc3339()),
            ));
        }
        let records = Records::new(&self.dialect);
        let keys: Vec<&str> = batch
            .iter()
            .map(|(record, _)| record.key.as_str())
            .collect();
        let held = records
            .versions(&place, &keys)
            .await
            .map_err(engine_error)?;
        let mut outcome = IngestOutcome::default();
        let mut last = None;
        for (record, observed) in &batch {
            let versions = held.get(&record.key).map(Vec::as_slice).unwrap_or_default();
            if let Some(live) = newest_live(versions).filter(|live| live.record == *record) {
                outcome.skipped += 1;
                outcome.ids.push(live.event_id.clone());
                continue;
            }
            self.families.pacing.wait().await;
            let event = records
                .append(&place, record, observed.clone(), false)
                .await
                .map_err(engine_error)?;
            outcome.written += 1;
            if let Some(event) = event {
                outcome.ids.push(event.id.clone());
                last = Some(event);
            }
        }
        if let Some(event) = last {
            records.wait(&place, &event).await.map_err(engine_error)?;
        }
        outcome.extract_jobs_enqueued = outcome.written;
        outcome.already_ingested = outcome.written == 0;
        Ok(outcome)
    }
}

#[async_trait]
impl MemoryIngest for CortexProvider {
    /// One record per source: a new version retires the one it replaced, and
    /// a version already held unchanged is not written again.
    async fn ingest_document(&self, item: IngestItem) -> Result<IngestOutcome, MemoryError> {
        if item.source_id.trim().is_empty() || item.content.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "document source id and content must not be empty".to_string(),
            ));
        }
        let place = self.ingest_place(&namespace_for(&item))?;
        let record = record(
            &item,
            format!("document:{}", item.source_id),
            MemoryCategory::Core,
            None,
        );
        let records = Records::new(&self.dialect);
        let held = records
            .versions(&place, &[&record.key])
            .await
            .map_err(engine_error)?
            .remove(&record.key)
            .unwrap_or_default();
        if let Some(live) = newest_live(&held).filter(|live| live.record == record) {
            return Ok(IngestOutcome {
                skipped: 1,
                ids: vec![live.event_id.clone()],
                already_ingested: true,
                ..IngestOutcome::default()
            });
        }
        let observed = item.timestamp.map(|stamp| stamp.to_rfc3339());
        let event = records
            .append(&place, &record, observed, false)
            .await
            .map_err(engine_error)?;
        let mut outcome = IngestOutcome {
            written: 1,
            extract_jobs_enqueued: 1,
            ..IngestOutcome::default()
        };
        if let Some(event) = event {
            records.wait(&place, &event).await.map_err(engine_error)?;
            outcome.ids.push(event.id);
        }
        records.retire(&place, &held, SUPERSEDED).await;
        Ok(outcome)
    }

    async fn ingest_chat(&self, messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError> {
        self.ingest_messages(messages, "chat", MemoryCategory::Conversation)
            .await
    }

    async fn ingest_email(&self, messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError> {
        self.ingest_messages(messages, "email", MemoryCategory::Core)
            .await
    }
}

#[cfg(test)]
#[path = "ingest_tests.rs"]
mod test;
