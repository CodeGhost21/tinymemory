//! Conversation source reader.
//!
//! Treats every local agent conversation thread as a memory source item.
//! Threads are JSON files under `<workspace>/threads/`, shaped
//! `{ title, messages: [{ role, content, created_at? }] }`. As a
//! [`SourceContent`] a thread renders to markdown; as a store item it becomes
//! a [`StoreItem::Conversation`] with one [`Turn`] per non-empty message.
//!
//! Safety: `item_id` is rejected if it contains path separators or `..`, and the
//! resolved file is re-checked for containment within the threads directory.

use std::path::{Path, PathBuf};

use crate::documents::DocumentConverter;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use tinymemory_api::{Role, StoreItem, Turn};

use crate::sources::error::{Error, Result};
use crate::sources::items;
use crate::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};
use crate::sources::validation::ensure_within_base;

use super::SourceReader;
use super::local_file::modified_at;

/// One thread read from disk, parsed into turns.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    /// The thread's id (its file stem).
    pub id: String,
    /// The thread's title, when it has one.
    pub title: Option<String>,
    /// Non-empty messages in order, as turns.
    pub turns: Vec<Turn>,
    /// The thread file's modification time.
    pub modified: Option<DateTime<Utc>>,
}

/// A reader over local agent conversation threads.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationReader;

impl ConversationReader {
    /// Resolve and containment-check the file for `item_id`.
    fn thread_path(item_id: &str, workspace: &Path) -> Result<PathBuf> {
        // Validate item_id to prevent path traversal before touching the FS.
        if item_id.is_empty()
            || matches!(item_id, "." | "..")
            || item_id.contains('/')
            || item_id.contains('\\')
        {
            return Err(Error::Invalid(
                "invalid item_id: path traversal denied".to_string(),
            ));
        }
        let threads_dir = workspace.join("threads");
        let thread_path = threads_dir.join(format!("{item_id}.json"));
        if !thread_path.exists() {
            return Err(Error::NotFound(format!("thread '{item_id}' not found")));
        }
        // Re-check containment after resolving symlinks.
        ensure_within_base(&threads_dir, &thread_path)
    }

    /// Read and parse one thread file.
    fn read_json(item_id: &str, workspace: &Path) -> Result<(serde_json::Value, PathBuf)> {
        let path = Self::thread_path(item_id, workspace)?;
        let raw = std::fs::read_to_string(&path)?;
        Ok((serde_json::from_str(&raw)?, path))
    }

    /// Read one thread by id as turns.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an id with path separators, [`Error::NotFound`]
    /// for a missing thread, [`Error::PathEscape`] for one that resolves
    /// outside the threads directory, and [`Error::Json`] for a file that is
    /// not JSON.
    pub fn read_thread(&self, item_id: &str, workspace: &Path) -> Result<Thread> {
        let (parsed, path) = Self::read_json(item_id, workspace)?;
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| modified_at(&metadata));
        Ok(Thread {
            id: item_id.to_string(),
            title: parsed
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            turns: thread_turns(&parsed),
            modified,
        })
    }
}

#[async_trait]
impl SourceReader for ConversationReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Conversation
    }

    async fn list_items(
        &self,
        _source: &MemorySourceEntry,
        workspace: &Path,
    ) -> Result<Vec<SourceItem>> {
        let threads_dir = workspace.join("threads");
        if !threads_dir.exists() {
            return Ok(Vec::new());
        }

        let mut items = Vec::new();
        for entry in std::fs::read_dir(&threads_dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let modified_ms = entry
                .metadata()
                .ok()
                .and_then(|m| modified_at(&m))
                .map(|at| at.timestamp_millis());
            items.push(SourceItem {
                title: id.clone(),
                id,
                updated_at_ms: modified_ms,
            });
        }
        Ok(items)
    }

    /// Read one thread's content by its `item_id` (the thread's file stem, as
    /// produced by [`list_items`](Self::list_items)), rendered as markdown.
    async fn read_item(
        &self,
        _source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<SourceContent> {
        let (parsed, _) = Self::read_json(item_id, workspace)?;
        let title = parsed
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(item_id)
            .to_string();
        Ok(SourceContent {
            id: item_id.to_string(),
            title,
            body: format_thread_as_markdown(&parsed),
            content_type: ContentType::Markdown,
            metadata: serde_json::json!({
                "source_type": "conversation",
                "thread_id": item_id,
            }),
        })
    }

    async fn read_store_item(
        &self,
        source: &MemorySourceEntry,
        item: &SourceItem,
        workspace: &Path,
        _converter: &dyn DocumentConverter,
    ) -> Result<StoreItem> {
        let thread = self.read_thread(&item.id, workspace)?;
        let mut meta = items::base_meta(source);
        meta.workspace = Some(workspace.display().to_string());
        items::conversation_item(thread, meta)
    }
}

/// Map a message's role string onto a [`Role`]. `None` for a role this
/// contract has no name for; such messages are dropped from the turns.
fn parse_role(role: &str) -> Option<Role> {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" | "human" => Some(Role::User),
        "assistant" | "agent" | "ai" | "bot" | "model" => Some(Role::Assistant),
        "system" | "developer" => Some(Role::System),
        "tool" | "function" => Some(Role::Tool),
        _ => None,
    }
}

/// Read a message timestamp: an RFC 3339 string, or epoch seconds or
/// milliseconds as a number (values above 10^12 are taken as milliseconds).
fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|at| at.with_timezone(&Utc));
    }
    let number = value.as_i64()?;
    if number.abs() >= 1_000_000_000_000 {
        Utc.timestamp_millis_opt(number).single()
    } else {
        Utc.timestamp_opt(number, 0).single()
    }
}

/// The thread's non-empty messages with a known role, as turns.
fn thread_turns(thread: &serde_json::Value) -> Vec<Turn> {
    let Some(messages) = thread.get("messages").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    messages
        .iter()
        .filter_map(|message| {
            let text = message.get("content").and_then(|v| v.as_str())?;
            if text.trim().is_empty() {
                return None;
            }
            let role_text = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let Some(role) = parse_role(role_text) else {
                log::debug!(
                    "[memory_sources:conversation] skipping message with role={role_text:?}"
                );
                return None;
            };
            let at = ["created_at", "timestamp", "at"]
                .iter()
                .find_map(|key| message.get(*key).and_then(parse_timestamp));
            Some(Turn {
                role,
                text: text.to_string(),
                at,
                tool_calls: Vec::new(),
            })
        })
        .collect()
}

/// Render a thread JSON value (`{ title, messages: [{ role, content }] }`) to
/// markdown. Messages with empty content are skipped.
fn format_thread_as_markdown(thread: &serde_json::Value) -> String {
    let mut out = String::new();

    if let Some(title) = thread.get("title").and_then(|v| v.as_str()) {
        out.push_str(&format!("# {title}\n\n"));
    }

    let messages = thread
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for msg in &messages {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if content.is_empty() {
            continue;
        }

        out.push_str(&format!("**{role}**: {content}\n\n"));
    }

    out
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
