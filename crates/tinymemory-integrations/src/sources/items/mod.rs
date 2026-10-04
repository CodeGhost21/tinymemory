//! Turning reader output into `StoreItem`s with their `MemoryMeta`.
//!
//! Every item a source produces names its reader in
//! `meta.source = SourceRef { kind, id: Some(entry.id) }`, with the config kind
//! mapped through [`SourceKind::api_kind`](crate::sources::SourceKind::api_kind). The
//! rest of the metadata depends on the kind:
//!
//! | Kind | Item | Metadata filled |
//! | --- | --- | --- |
//! | folder, file | document | `workspace`, `folder` (containing directory), `file_path`, `language` (by extension), `observed_at` (mtime), `mime` |
//! | github | document | `repo` (`owner/name`), `commit` (commit items), `url` (issues and PRs), `observed_at` |
//! | link | document | `url` |
//! | rss | document | `url` (the entry's link), `observed_at` (published) |
//! | composio | document | `tags = [toolkit]` (payloads: see [`crate::sources::composio`]) |
//! | conversation | conversation | `workspace`, `thread_id`, `turns`, `observed_at` (last turn) |
//!
//! Every document body is markdown, converted through `tinymemory-documents`:
//! local files through the host's [`DocumentConverter`] (so a bound PDF or
//! DOCX converter applies), reader bodies through
//! [`markdown_from_text`].
//!
//! [`collect_items`] drives a reader end to end: list, then read each item as
//! a `StoreItem`, collecting per-item failures instead of aborting the pass.

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use tinymemory_api::{DocumentBody, MemoryMeta, SourceRef, StoreItem, TurnRange};
use crate::documents::{
    document_item, language_for_path, markdown_from_text, DocumentConverter, DocumentFormat,
};

use crate::sources::error::{Error, Result};
use crate::sources::readers::conversation::Thread;
use crate::sources::readers::file::FileReader;
use crate::sources::readers::local_file::LocalFile;
use crate::sources::readers::SourceReader;
use crate::sources::types::{ContentType, MemorySourceEntry, SourceContent, SourceKind};

/// Metadata naming `entry` as the source: `source.kind` is the entry's kind
/// mapped onto the contract, `source.id` its id. A Composio entry also gets
/// its toolkit as a tag.
#[must_use]
pub fn base_meta(entry: &MemorySourceEntry) -> MemoryMeta {
    let mut meta = MemoryMeta {
        source: SourceRef {
            kind: entry.kind.api_kind(),
            id: Some(entry.id.clone()),
        },
        ..MemoryMeta::default()
    };
    if entry.kind == SourceKind::Composio {
        if let Some(toolkit) = entry.toolkit.as_deref().filter(|t| !t.is_empty()) {
            meta.tags = vec![toolkit.to_string()];
        }
    }
    meta
}

/// Convert a local file and wrap it as a document.
///
/// Fills `folder` (the file's containing directory), `file_path` (its
/// canonical path) and `observed_at` (its mtime) on top of `meta`; the
/// converter's result supplies the title and `mime`, and `language` comes
/// from the extension unless `meta` already set one.
///
/// # Errors
///
/// [`Error::Document`] when the converter refuses or fails, for example a
/// format no bound converter handles.
pub async fn local_file_item(
    file: LocalFile,
    mut meta: MemoryMeta,
    converter: &dyn DocumentConverter,
) -> Result<StoreItem> {
    meta.file_path = Some(file.path.display().to_string());
    meta.folder = file.path.parent().map(|dir| dir.display().to_string());
    if meta.observed_at.is_none() {
        meta.observed_at = file.modified;
    }
    let document = file.to_raw_document();
    Ok(document_item(converter, &document, meta).await?)
}

/// Read the file at `path` — no configured source needed — and wrap it as a
/// document with `source.kind = File`.
///
/// `workspace` is recorded when given; `source_id` names the configured
/// source, if any.
///
/// # Errors
///
/// Whatever [`FileReader::read_path`] returns, plus [`Error::Document`] when
/// conversion fails.
pub async fn file_item(
    path: &Path,
    workspace: Option<&str>,
    source_id: Option<String>,
    converter: &dyn DocumentConverter,
) -> Result<StoreItem> {
    let file = FileReader::read_path(path)?;
    let mut meta = MemoryMeta::from_source(tinymemory_api::SourceKind::File, source_id);
    meta.workspace = workspace.map(str::to_string);
    local_file_item(file, meta, converter).await
}

/// Wrap a parsed thread as a [`StoreItem::Conversation`].
///
/// Fills `thread_id`, `turns` (all of them, zero-based) and `observed_at`
/// (the last timestamped turn, else the file's mtime) on top of `meta`.
///
/// # Errors
///
/// [`Error::Invalid`] for a thread with no non-empty turns.
pub fn conversation_item(thread: Thread, mut meta: MemoryMeta) -> Result<StoreItem> {
    if thread.turns.is_empty() {
        return Err(Error::Invalid(format!(
            "thread '{}' has no turns to store",
            thread.id
        )));
    }
    let last = u32::try_from(thread.turns.len() - 1).unwrap_or(u32::MAX);
    meta.thread_id = Some(thread.id);
    meta.turns = Some(TurnRange { first: 0, last });
    if meta.observed_at.is_none() {
        meta.observed_at = thread
            .turns
            .iter()
            .rev()
            .find_map(|turn| turn.at)
            .or(thread.modified);
    }
    Ok(StoreItem::Conversation {
        turns: thread.turns,
        meta,
    })
}

/// Wrap a reader's [`SourceContent`] as a document, with the metadata its
/// source kind carries (see the module table).
///
/// `updated_at_ms` is the listing's timestamp for the item, used for
/// `observed_at` when the content carries no better one.
///
/// # Errors
///
/// [`Error::Invalid`] when the body converts to no text.
pub fn content_item(
    entry: &MemorySourceEntry,
    content: SourceContent,
    updated_at_ms: Option<i64>,
) -> Result<StoreItem> {
    let format = match content.content_type {
        ContentType::Markdown => DocumentFormat::Markdown,
        ContentType::Html => DocumentFormat::Html,
        ContentType::Plaintext => DocumentFormat::PlainText,
    };
    let markdown = markdown_from_text(&content.body, format);
    if markdown.trim().is_empty() {
        return Err(Error::Invalid(format!(
            "item '{}' has no text to store",
            content.id
        )));
    }
    let mime = match format {
        DocumentFormat::PlainText => DocumentFormat::PlainText.mime(),
        _ => DocumentFormat::Markdown.mime(),
    };

    let mut meta = base_meta(entry);
    meta.observed_at = millis(updated_at_ms);
    let metadata = &content.metadata;
    match entry.kind {
        SourceKind::GithubRepo => github_meta(&mut meta, &content.id, metadata),
        SourceKind::RssFeed => {
            meta.url = string_field(metadata, "link");
            if let Some(published) = string_field(metadata, "published")
                .as_deref()
                .and_then(parse_feed_time)
            {
                meta.observed_at = Some(published);
            }
        }
        SourceKind::WebPage => {
            meta.url = string_field(metadata, "url").or_else(|| Some(content.id.clone()));
        }
        SourceKind::Folder => {
            if let Some(root) = entry.path.as_deref() {
                let path = Path::new(root).join(&content.id);
                meta.file_path = Some(path.display().to_string());
                meta.folder = path.parent().map(|dir| dir.display().to_string());
            }
            meta.language = language_for_path(&content.id).map(str::to_string);
        }
        SourceKind::File => {
            meta.file_path = string_field(metadata, "path").or_else(|| entry.path.clone());
            meta.language = language_for_path(&content.id).map(str::to_string);
        }
        SourceKind::Conversation => meta.thread_id = Some(content.id.clone()),
        SourceKind::Composio => {}
    }

    Ok(StoreItem::Document {
        title: Some(content.title).filter(|title| !title.trim().is_empty()),
        body: DocumentBody::Text(markdown),
        mime: Some(mime.to_string()),
        meta,
    })
}

/// GitHub metadata from a reader item id (`commit:<sha>`, `issue:<n>`,
/// `pr:<n>`) and its content metadata (`owner`, `repo`, `sha`, `number`).
fn github_meta(meta: &mut MemoryMeta, item_id: &str, metadata: &serde_json::Value) {
    let owner = string_field(metadata, "owner");
    let repo = string_field(metadata, "repo");
    let slug = owner
        .zip(repo)
        .map(|(owner, repo)| format!("{owner}/{repo}"));
    meta.repo = slug.clone();
    let number = metadata
        .get("number")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n.to_string());
    if let Some(sha) = item_id.strip_prefix("commit:") {
        meta.commit = string_field(metadata, "sha").or_else(|| Some(sha.to_string()));
    } else if let Some(n) = item_id.strip_prefix("issue:") {
        let n = number.unwrap_or_else(|| n.to_string());
        meta.url = slug.map(|slug| format!("https://github.com/{slug}/issues/{n}"));
    } else if let Some(n) = item_id.strip_prefix("pr:") {
        let n = number.unwrap_or_else(|| n.to_string());
        meta.url = slug.map(|slug| format!("https://github.com/{slug}/pull/{n}"));
    }
}

/// A non-empty string field of a JSON object.
fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Epoch milliseconds as a UTC instant.
fn millis(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.and_then(|ms| Utc.timestamp_millis_opt(ms).single())
}

/// An RSS `pubDate` (RFC 2822) or Atom `updated` (RFC 3339) timestamp.
fn parse_feed_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(text)
        .or_else(|_| DateTime::parse_from_rfc3339(text))
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// One item a [`collect_items`] pass could not turn into a `StoreItem`.
#[derive(Debug)]
pub struct SkippedItem {
    /// The reader-scoped item id.
    pub id: String,
    /// Why it was skipped.
    pub error: Error,
}

/// What a [`collect_items`] pass produced.
#[derive(Debug, Default)]
pub struct Collected {
    /// The items read, in listing order.
    pub items: Vec<StoreItem>,
    /// Items that failed to read or convert, with the reason.
    pub skipped: Vec<SkippedItem>,
}

/// List `entry` through `reader` and read every item as a `StoreItem`.
///
/// One bad item (an unreadable file, a format no converter handles) does not
/// abort the pass: it lands in [`Collected::skipped`] and the rest are still
/// read.
///
/// # Errors
///
/// Only the listing's failure; per-item failures are collected.
pub async fn collect_items(
    reader: &dyn SourceReader,
    entry: &MemorySourceEntry,
    workspace: &Path,
    converter: &dyn DocumentConverter,
) -> Result<Collected> {
    let listed = reader.list_items(entry, workspace).await?;
    let mut collected = Collected::default();
    for item in &listed {
        match reader
            .read_store_item(entry, item, workspace, converter)
            .await
        {
            Ok(store_item) => collected.items.push(store_item),
            Err(error) => {
                log::debug!(
                    "[memory_sources:items] skipping item source={} id={} error={error}",
                    entry.id,
                    item.id
                );
                collected.skipped.push(SkippedItem {
                    id: item.id.clone(),
                    error,
                });
            }
        }
    }
    log::debug!(
        "[memory_sources:items] collected source={} items={} skipped={}",
        entry.id,
        collected.items.len(),
        collected.skipped.len()
    );
    Ok(collected)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
