//! Typed metadata carried by every stored item.
//!
//! [`MemoryMeta`] says where an item came from and what it is about: the
//! workspace, folder and file it was read from, the code language, the
//! repository and commit, the conversation thread and turns, the agent and tool
//! call that produced it, and the [`SourceRef`] that names its reader. Every
//! field is optional except the source, so a reader fills in what it knows.
//!
//! [`MetaFilter`] is the matching query side: the same fields as exact
//! matches (with `folder` and `file_path` also matching as a path prefix), plus
//! item kinds, source kinds, a tag set and an observation window.

mod filter;
mod redact;

pub use filter::MetaFilter;
pub use redact::{REDACTED_PREFIX, holds_phone_number, redacted_id, same_id};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::namespace::Namespace;

/// Where an item came from and what it is about.
///
/// Fields are added as the contract grows, so build one with
/// `..MemoryMeta::default()` for the fields you do not set. A literal that
/// names every field stops compiling when a field is added, which is a
/// major release.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryMeta {
    /// The memory node the item belongs to; the root (shared by every
    /// agent) by default. See [`crate::namespace`].
    #[serde(skip_serializing_if = "Namespace::is_root")]
    pub namespace: Namespace,
    /// Absolute path or logical workspace id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Containing folder, absolute or workspace-relative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The file the item was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Code language (`rust`, `python`) or natural-language tag (`en`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Repository, as `owner/name` or a remote URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Commit the item was read at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// URL the item was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Conversation thread the item belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Which turns of the thread the item covers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<TurnRange>,
    /// Agent that produced the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Tool call that produced the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallRef>,
    /// The reader or producer that supplied the item.
    pub source: SourceRef,
    /// Free-form tags.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// When the underlying fact was observed, as opposed to when it was stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
    /// Whether an engine that derives layers from what it stores (facts,
    /// beliefs, concepts) may derive them from this item. `Some(false)`
    /// stores and indexes the item, so it stays searchable, but derives
    /// nothing from it: runtime state, tool output, machine-written
    /// summaries. `None`, the default, leaves it to the engine; engines that
    /// derive nothing ignore it. Unset, it is not serialized, so an item's
    /// fingerprint is the same as before the field existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derive: Option<bool>,
}

impl MemoryMeta {
    /// Metadata naming only its source.
    #[must_use]
    pub fn from_source(kind: SourceKind, id: Option<String>) -> Self {
        Self {
            source: SourceRef { kind, id },
            ..Self::default()
        }
    }
}

/// An inclusive range of conversation turns, zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRange {
    /// First turn covered.
    pub first: u32,
    /// Last turn covered, inclusive.
    pub last: u32,
}

/// A reference to one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRef {
    /// The tool's name.
    pub name: String,
    /// The call's id, when the producer assigned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Which reader or producer supplied an item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// The kind of source.
    pub kind: SourceKind,
    /// The source's own id (a configured source id, a feed URL, a thread id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The kind of reader or producer an item came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A local folder.
    Folder,
    /// A single file.
    File,
    /// A web page.
    Link,
    /// A GitHub repository.
    Github,
    /// An RSS or Atom feed.
    Rss,
    /// A Composio toolkit payload (Gmail, Slack, Notion, ...).
    Composio,
    /// A host conversation.
    Conversation,
    /// Written by an agent directly; the default.
    #[default]
    Agent,
    /// Imported from a legacy store.
    Import,
}

impl SourceKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Folder,
        Self::File,
        Self::Link,
        Self::Github,
        Self::Rss,
        Self::Composio,
        Self::Conversation,
        Self::Agent,
        Self::Import,
    ];

    /// The stable snake_case wire string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::File => "file",
            Self::Link => "link",
            Self::Github => "github",
            Self::Rss => "rss",
            Self::Composio => "composio",
            Self::Conversation => "conversation",
            Self::Agent => "agent",
            Self::Import => "import",
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
