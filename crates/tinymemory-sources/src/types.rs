//! Core types for memory sources.
//!
//! A *memory source* answers the question "what feeds my memory?". Each
//! configured source is a [`MemorySourceEntry`] persisted in `config.toml`
//! under `[[memory_sources]]`. The [`SourceKind`] discriminator selects which
//! kind-specific fields are required; required-field checks live in
//! [`crate::validation`] and are surfaced via
//! [`MemorySourceEntry::validate`].
//!
//! Reader output contracts ([`SourceItem`], [`SourceContent`], [`ContentType`])
//! are shared across every reader implementation so the host can ingest source
//! payloads uniformly regardless of where they came from; [`crate::items`]
//! turns them into `StoreItem`s.
//!
//! Wire strings are snake_case and are part of the persisted contract — do not
//! rename them when porting from OpenHuman.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub(crate) fn default_true() -> bool {
    true
}

/// The kind of a configured memory source.
///
/// The wire representation is snake_case (`github_repo`, `rss_feed`, …) and is
/// persisted in `config.toml`; it must stay stable across versions. Each maps
/// onto one [`tinymemory_api::SourceKind`] through [`SourceKind::api_kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A Composio OAuth connector (Gmail, Slack, Notion, …). Network-backed;
    /// the live fetch is owned by the host, not this crate.
    Composio,
    /// Local agent conversation transcripts stored in the workspace.
    Conversation,
    /// A local folder of files matched by an optional glob.
    Folder,
    /// A single local file.
    File,
    /// A GitHub repository's project activity (commits, issues, PRs).
    GithubRepo,
    /// An RSS/Atom feed.
    RssFeed,
    /// A single web page, optionally narrowed by a CSS selector.
    WebPage,
}

impl SourceKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::Composio,
        Self::Conversation,
        Self::Folder,
        Self::File,
        Self::GithubRepo,
        Self::RssFeed,
        Self::WebPage,
    ];

    /// The stable snake_case wire string for this kind.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Composio => "composio",
            SourceKind::Conversation => "conversation",
            SourceKind::Folder => "folder",
            SourceKind::File => "file",
            SourceKind::GithubRepo => "github_repo",
            SourceKind::RssFeed => "rss_feed",
            SourceKind::WebPage => "web_page",
        }
    }

    /// The contract's [`tinymemory_api::SourceKind`] for items this kind of
    /// source produces: a web page is a `Link`, a GitHub repository `Github`,
    /// an RSS feed `Rss`; the rest keep their name.
    #[must_use]
    pub fn api_kind(&self) -> tinymemory_api::SourceKind {
        use tinymemory_api::SourceKind as Api;
        match self {
            SourceKind::Composio => Api::Composio,
            SourceKind::Conversation => Api::Conversation,
            SourceKind::Folder => Api::Folder,
            SourceKind::File => Api::File,
            SourceKind::GithubRepo => Api::Github,
            SourceKind::RssFeed => Api::Rss,
            SourceKind::WebPage => Api::Link,
        }
    }
}

/// A configured memory source entry persisted in `config.toml`.
///
/// All kind-specific fields are flattened onto the struct as `Option`s. The
/// [`kind`](MemorySourceEntry::kind) discriminator determines which fields are
/// required; validation is enforced at add/update time via
/// [`MemorySourceEntry::validate`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySourceEntry {
    /// Stable unique id (e.g. `src_<uuid>`).
    pub id: String,
    /// Discriminator selecting the kind-specific fields below.
    pub kind: SourceKind,
    /// Human-readable label shown in UIs.
    pub label: String,
    /// Whether this source participates in sync. Defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    // ── Composio ──
    /// Composio toolkit slug (e.g. `gmail`). Required for `composio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolkit: Option<String>,
    /// Composio connection id. Required for `composio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    // ── Folder / File ──
    /// Filesystem path of the folder or file to read. Required for `folder`
    /// and `file`; a relative path is anchored on the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional glob applied under a folder's `path`. When absent the folder
    /// reader takes markdown, plain-text and source-code files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,

    // ── GithubRepo / RssFeed / WebPage (shared) ──
    /// Source URL. Required for `github_repo`, `rss_feed`, and `web_page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    // ── GithubRepo ──
    /// Branch to read (defaults to the repo default when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Optional path filters within the repo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Max commits to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_commits: Option<u32>,
    /// Max issues to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_issues: Option<u32>,
    /// Max pull requests to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_prs: Option<u32>,

    // ── RssFeed ──
    /// Max feed items to pull per sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,

    // ── WebPage ──
    /// Optional CSS selector to narrow extracted content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    // ── Sync Budget (all source kinds) ──
    /// Maximum tokens to consume per sync run. Sync stops once this budget is hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_sync: Option<u64>,
    /// Maximum cost in USD per sync run. Refuses LLM calls once reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_per_sync_usd: Option<f64>,
    /// Sync depth in days — only fetch items from the last N days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_depth_days: Option<u32>,
}

impl MemorySourceEntry {
    /// An enabled entry of `kind` with every optional field unset.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: SourceKind, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            enabled: true,
            toolkit: None,
            connection_id: None,
            path: None,
            glob: None,
            url: None,
            branch: None,
            paths: Vec::new(),
            max_commits: None,
            max_issues: None,
            max_prs: None,
            max_items: None,
            selector: None,
            max_tokens_per_sync: None,
            max_cost_per_sync_usd: None,
            sync_depth_days: None,
        }
    }

    /// Validate required fields for this entry's [`SourceKind`].
    ///
    /// Delegates to [`crate::validation::validate_entry`].
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the first failing rule.
    pub fn validate(&self) -> Result<()> {
        crate::validation::validate_entry(self)
    }
}

fn deserialize_double_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// Partial update payload for a source entry.
///
/// An absent field leaves the current value unchanged. For optional source
/// properties, an explicit JSON `null` clears the value while a concrete value
/// replaces it.
#[derive(Debug, Default, Deserialize)]
pub struct MemorySourcePatch {
    /// New human-readable label for the source.
    #[serde(default)]
    pub label: Option<String>,
    /// Toggle whether the source participates in sync.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Composio toolkit slug (e.g. `gmail`, `slack`).
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub toolkit: Option<Option<String>>,
    /// Composio connection id this source binds to.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub connection_id: Option<Option<String>>,
    /// Filesystem root for a local-files source.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub path: Option<Option<String>>,
    /// Glob filter applied under [`MemorySourcePatch::path`].
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub glob: Option<Option<String>>,
    /// Remote URL for a git/web source.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub url: Option<Option<String>>,
    /// Git branch to track.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub branch: Option<Option<String>>,
    /// Explicit path allowlist within a repo source.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// Cap on the number of items pulled per sync.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_items: Option<Option<u32>>,
    /// Source-specific selector (e.g. a CSS selector for a web page).
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub selector: Option<Option<String>>,
    /// Token budget per sync run.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_tokens_per_sync: Option<Option<u64>>,
    /// Cost budget per sync run, in USD.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_cost_per_sync_usd: Option<Option<f64>>,
    /// History depth in days for tree/summary backfill.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub sync_depth_days: Option<Option<u32>>,
    /// Cap on commits ingested from a git source.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_commits: Option<Option<u32>>,
    /// Cap on issues ingested from a repo source.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_issues: Option<Option<u32>>,
    /// Cap on pull requests ingested from a repo source.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_prs: Option<Option<u32>>,
}

impl MemorySourcePatch {
    /// Reject fields that do not apply to `kind`.
    ///
    /// A patch is a partial update, so a caller can set a field the source's
    /// kind has no use for — a git branch on an RSS feed. Catching that here
    /// keeps a nonsensical value out of the registry rather than letting the
    /// reader discover it later.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the first inapplicable field.
    pub fn validate_for_kind(&self, kind: SourceKind) -> Result<()> {
        let reject = |field: &str| {
            Err(Error::Invalid(format!(
                "field '{field}' is not applicable to source kind '{}'",
                kind.as_str()
            )))
        };
        if (self.toolkit.is_some() || self.connection_id.is_some()) && kind != SourceKind::Composio
        {
            return reject("toolkit/connection_id");
        }
        if self.path.is_some() && !matches!(kind, SourceKind::Folder | SourceKind::File) {
            return reject("path");
        }
        if self.glob.is_some() && kind != SourceKind::Folder {
            return reject("glob");
        }
        if (self.branch.is_some()
            || self.paths.is_some()
            || self.max_commits.is_some()
            || self.max_issues.is_some()
            || self.max_prs.is_some())
            && kind != SourceKind::GithubRepo
        {
            return reject("github repository fields");
        }
        if self.selector.is_some() && kind != SourceKind::WebPage {
            return reject("selector");
        }
        // `max_items` is the per-run ingest cap. It applies to RSS feeds and to
        // Composio connections — the host UI (`SourceSettingsPanel`) exposes it
        // for both, and a Composio source is created with a toolkit default, so
        // rejecting it on edit desynced the UI from the store. Other kinds have
        // no per-run item cap.
        if matches!(self.max_items, Some(Some(_)))
            && !matches!(kind, SourceKind::RssFeed | SourceKind::Composio)
        {
            return reject("max_items");
        }
        if self.url.is_some()
            && kind != SourceKind::GithubRepo
            && kind != SourceKind::RssFeed
            && kind != SourceKind::WebPage
        {
            return reject("url");
        }
        Ok(())
    }

    /// Apply each present field of this patch onto `entry` in place.
    pub fn apply_to(self, entry: &mut MemorySourceEntry) {
        if let Some(value) = self.label {
            entry.label = value;
        }
        if let Some(value) = self.enabled {
            entry.enabled = value;
        }
        if let Some(value) = self.toolkit {
            entry.toolkit = value;
        }
        if let Some(value) = self.connection_id {
            entry.connection_id = value;
        }
        if let Some(value) = self.path {
            entry.path = value;
        }
        if let Some(value) = self.glob {
            entry.glob = value;
        }
        if let Some(value) = self.url {
            entry.url = value;
        }
        if let Some(value) = self.branch {
            entry.branch = value;
        }
        if let Some(value) = self.paths {
            entry.paths = value;
        }
        if let Some(value) = self.max_items {
            entry.max_items = value;
        }
        if let Some(value) = self.selector {
            entry.selector = value;
        }
        if let Some(value) = self.max_tokens_per_sync {
            entry.max_tokens_per_sync = value;
        }
        if let Some(value) = self.max_cost_per_sync_usd {
            entry.max_cost_per_sync_usd = value;
        }
        if let Some(value) = self.sync_depth_days {
            entry.sync_depth_days = value;
        }
        if let Some(value) = self.max_commits {
            entry.max_commits = value;
        }
        if let Some(value) = self.max_issues {
            entry.max_issues = value;
        }
        if let Some(value) = self.max_prs {
            entry.max_prs = value;
        }
    }
}

/// One item listed from a source reader.
///
/// `id` is reader-scoped (e.g. a folder-relative path or a thread id) and is
/// stable enough to pass back into `SourceReader::read_item`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceItem {
    /// Reader-scoped item id.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Last-modified time in epoch milliseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
}

/// The rendered content type of a [`SourceContent`] body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    /// Markdown body.
    Markdown,
    /// Raw HTML body.
    Html,
    /// Plain text body.
    Plaintext,
}

/// Content read from a single source item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceContent {
    /// Reader-scoped item id (matches the [`SourceItem::id`] it was read from).
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// The item body, rendered as [`content_type`](SourceContent::content_type).
    pub body: String,
    /// How [`body`](SourceContent::body) should be interpreted.
    pub content_type: ContentType,
    /// Reader-specific metadata (JSON object).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
