//! [`MigrateStep::Content`](super::MigrateStep::Content): ingested content,
//! re-sent raw.
//!
//! A summary tree, its embeddings and its extracted entities are derived from
//! the chunks a driver stored, by that driver, in its own embedding space —
//! copying them would hand the target vectors it cannot compare and summaries
//! it did not write. So the step reads each logical source's chunks back,
//! oldest first, and sends their text to the target's ingest, which chunks,
//! embeds and summarises it the way it does everything else:
//!
//! - a document's chunks are joined into the document again, one
//!   `ingest_document` per source;
//! - mail goes to `ingest_email` and chat to `ingest_chat`, a chunk per
//!   message, in batches.
//!
//! The chunks do not record which provider a source came from or how it was
//! tainted, so both are inferred: the provider from the source id's prefix,
//! the taint as external for everything but the agent's own conversations.
//! Erring towards external only costs the content some trust in recall; the
//! other way would trust fetched text as the user's own.

use crate::capabilities::Capability;
use crate::chunks::{Chunk, DataSource, SourceKind};
use crate::error::MemoryError;
use crate::provider::types::{IngestItem, IngestOutcome};
use crate::provider::{ChunkQuery, MemoryChunks, MemoryIngest, MemoryProvider};
use crate::types::MemoryTaint;

use super::{unserved, CopyOptions, CopyProgress, MigrateStep, StepReport};

/// Most sources one listing returns; the embedded engine's own ceiling.
const SOURCE_LIMIT: usize = 10_000;

/// Chunks one listing page asks for.
const CHUNK_PAGE: usize = 1_000;

/// Messages one chat or mail ingest carries.
const MESSAGE_BATCH: usize = 100;

/// The source id agent conversations are ingested under.
const AGENT_CONVERSATIONS: &str = "conversations:";

/// The provider a source's id names, by its prefix, else the generic member
/// of its kind.
pub(super) fn data_source(kind: SourceKind, source_id: &str) -> DataSource {
    let id = source_id.trim().to_ascii_lowercase();
    let named = |prefixes: &[&str]| prefixes.iter().any(|p| id.starts_with(p));
    match kind {
        SourceKind::Email if named(&["gmail"]) => DataSource::Gmail,
        SourceKind::Email => DataSource::OtherEmail,
        SourceKind::Chat if named(&["discord"]) => DataSource::Discord,
        SourceKind::Chat if named(&["telegram"]) => DataSource::Telegram,
        SourceKind::Chat if named(&["whatsapp"]) => DataSource::Whatsapp,
        SourceKind::Chat => DataSource::Conversation,
        SourceKind::Document if named(&["notion"]) => DataSource::Notion,
        SourceKind::Document if named(&["drive", "gdrive", "google_drive", "googledrive"]) => {
            DataSource::DriveDocs
        }
        SourceKind::Document if named(&["meeting"]) => DataSource::MeetingNotes,
        SourceKind::Document if named(&["http://", "https://", "web"]) => DataSource::WebPage,
        SourceKind::Document => DataSource::Upload,
    }
}

/// The taint a replayed source is stamped with. See the module docs.
pub(super) fn taint_of(source_id: &str) -> MemoryTaint {
    if source_id.starts_with(AGENT_CONVERSATIONS) {
        MemoryTaint::Internal
    } else {
        MemoryTaint::ExternalSync
    }
}

/// One chunk as the item it is re-sent as.
fn item(chunk: &Chunk, body: String, source: DataSource, taint: MemoryTaint) -> IngestItem {
    let metadata = &chunk.metadata;
    IngestItem {
        namespace: None,
        source,
        source_id: metadata.source_id.clone(),
        owner: metadata.owner.clone(),
        source_ref: metadata.source_ref.clone(),
        content: body,
        mime: None,
        timestamp: Some(metadata.timestamp),
        tags: metadata.tags.clone(),
        author: None,
        channel_label: None,
        platform: None,
        to: Vec::new(),
        cc: Vec::new(),
        subject: None,
        list_unsubscribe: None,
        taint,
        path_scope: metadata.path_scope.clone(),
    }
}

/// Every chunk of one source, oldest first, each with its full body.
async fn source_chunks(
    chunks: &dyn MemoryChunks,
    kind: SourceKind,
    source_id: &str,
) -> Result<Vec<(Chunk, String)>, MemoryError> {
    let mut rows = Vec::new();
    loop {
        let query = ChunkQuery {
            source_kind: Some(kind),
            source_id: Some(source_id.to_string()),
            limit: Some(CHUNK_PAGE),
            offset: Some(rows.len()),
            exclude_dropped: true,
            ..ChunkQuery::default()
        };
        let page = chunks.list_chunks(&query, None).await?;
        let full = page.len() == CHUNK_PAGE;
        rows.extend(page);
        if !full {
            break;
        }
    }
    rows.sort_by(|a, b| {
        a.seq_in_source
            .cmp(&b.seq_in_source)
            .then(a.metadata.timestamp.cmp(&b.metadata.timestamp))
    });
    rows.dedup_by(|a, b| a.id == b.id);
    let mut bodies = Vec::with_capacity(rows.len());
    for chunk in rows {
        // A listing carries a preview; the body is the chunk's text. A vault
        // read that failed falls back to the preview rather than to nothing.
        let body = chunks
            .chunk_detail(&chunk.id)
            .await?
            .and_then(|detail| detail.body)
            .unwrap_or_else(|| chunk.content.clone());
        bodies.push((chunk, body));
    }
    Ok(bodies)
}

/// Sends one source's chunks to `ingest`.
async fn resend(
    ingest: &dyn MemoryIngest,
    kind: SourceKind,
    source_id: &str,
    chunks: Vec<(Chunk, String)>,
) -> Result<Vec<IngestOutcome>, MemoryError> {
    let source = data_source(kind, source_id);
    let taint = taint_of(source_id);
    if kind == SourceKind::Document {
        let Some((first, _)) = chunks.first() else {
            return Ok(Vec::new());
        };
        let mut document = item(first, String::new(), source, taint);
        document.timestamp = chunks.iter().map(|(c, _)| c.metadata.timestamp).max();
        let mut tags: Vec<String> = Vec::new();
        for (chunk, _) in &chunks {
            for tag in &chunk.metadata.tags {
                if !tags.contains(tag) {
                    tags.push(tag.clone());
                }
            }
        }
        document.tags = tags;
        document.content = chunks
            .into_iter()
            .map(|(_, body)| body)
            .collect::<Vec<_>>()
            .join("\n\n");
        return Ok(vec![ingest.ingest_document(document).await?]);
    }
    let messages: Vec<IngestItem> = chunks
        .iter()
        .map(|(chunk, body)| item(chunk, body.clone(), source, taint))
        .collect();
    let mut outcomes = Vec::new();
    for batch in messages.chunks(MESSAGE_BATCH) {
        let batch = batch.to_vec();
        outcomes.push(match kind {
            SourceKind::Email => match ingest.ingest_email(batch.clone()).await {
                // A driver that does not split mail stores it as a
                // conversation rather than losing it.
                Err(MemoryError::Unsupported { .. }) => ingest.ingest_chat(batch).await?,
                other => other?,
            },
            _ => ingest.ingest_chat(batch).await?,
        });
    }
    Ok(outcomes)
}

/// Re-sends the source's ingested content to the target. See the module docs.
pub(super) async fn replay(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    options: &CopyOptions,
    progress: &mut impl FnMut(CopyProgress),
) -> anyhow::Result<StepReport> {
    let step = MigrateStep::Content;
    if !options.replay_content {
        return Ok(StepReport::skipped(
            step,
            "the caller chose not to re-send content",
        ));
    }
    let Some(chunks) = from.as_chunks() else {
        return Ok(StepReport::skipped(
            step,
            unserved("source", Capability::Chunks),
        ));
    };
    let Some(ingest) = to.as_ingest() else {
        return Ok(StepReport::skipped(
            step,
            unserved("target", Capability::Ingest),
        ));
    };
    let sources = match chunks.source_totals(SOURCE_LIMIT, None).await {
        Ok(sources) => sources,
        Err(MemoryError::Unsupported { .. }) => {
            return Ok(StepReport::skipped(
                step,
                "the source cannot list the sources it holds",
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let mut report = StepReport::new(step);
    for total in sources {
        if options
            .skip_source_prefixes
            .iter()
            .any(|prefix| total.source_id.starts_with(prefix.as_str()))
        {
            continue;
        }
        let read = source_chunks(chunks, total.source_kind, &total.source_id).await?;
        let count = read.len();
        report.read += count;
        match resend(ingest, total.source_kind, &total.source_id, read).await {
            Ok(outcomes) => {
                for outcome in outcomes {
                    if outcome.already_ingested {
                        report.unchanged += count;
                    }
                    report.written += usize::try_from(outcome.written).unwrap_or(usize::MAX);
                }
            }
            Err(error @ (MemoryError::Invalid(_) | MemoryError::Unsupported { .. })) => {
                report.failed += count;
                report.note(format!(
                    "{} source {}: {error}",
                    total.source_kind.as_str(),
                    total.source_id
                ));
            }
            Err(error) => return Err(error.into()),
        }
        progress(CopyProgress {
            step,
            read: report.read,
            written: report.written,
        });
    }
    Ok(report)
}
