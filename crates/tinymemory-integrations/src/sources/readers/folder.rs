//! Local folder source reader.
//!
//! Walks files under a local directory and reads them. With a configured glob
//! only matching files are taken; without one, the reader takes markdown,
//! plain-text and source-code files ([`is_default_candidate`]).
//!
//! Safety:
//!
//! - file sizes are capped at [`FOLDER_FILE_SIZE_CAP_BYTES`] (10 MB) on both
//!   list and read;
//! - `read_item` is guarded against path traversal and symlink escapes via
//!   [`ensure_within_base`], and symlinks are never followed while walking;
//! - version-control, build and dependency directories (`.git`, `target`,
//!   `node_modules`, …) and hidden files and directories are skipped, so a
//!   folder source never ingests a repository's object store or a `.env`.
//!
//! The directory walk uses `walkdir`; glob patterns are compiled to a `regex`
//! (matched against the slash-normalised path relative to the folder root).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use regex::Regex;
use tinymemory_api::StoreItem;
use crate::documents::{language_for_path, DocumentConverter, DocumentFormat};
use walkdir::WalkDir;

use crate::sources::error::{Error, Result};
use crate::sources::items;
use crate::sources::types::{ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind};
use crate::sources::validation::ensure_within_base;
use crate::sources::FOLDER_FILE_SIZE_CAP_BYTES;

use super::local_file::{modified_at, read_capped, resolve_base, LocalFile};
use super::SourceReader;

/// Directory names never descended into, wherever they appear.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "__pycache__",
    "venv",
];

/// Build the "folder does not exist" error so it always says **where the reader
/// looked**, not merely what it was configured with.
///
/// The resolved path is appended only when it differs from the configured one,
/// so an absolute source does not get a redundant echo of itself
/// (tinyhumansai/openhuman#5830).
fn missing_folder_error(base_path: &str, resolved: &Path) -> Error {
    let resolved = resolved.display().to_string();
    if resolved == base_path {
        Error::NotFound(format!("folder does not exist: {base_path}"))
    } else {
        Error::NotFound(format!(
            "folder does not exist: {base_path} (resolved to {resolved})"
        ))
    }
}

/// Whether a file is taken by a folder source with no glob: markdown, plain
/// text, or source code ([`language_for_path`]). HTML, PDF, images and
/// anything unrecognised need an explicit glob.
#[must_use]
pub fn is_default_candidate(relative_path: &str) -> bool {
    language_for_path(relative_path).is_some()
        || matches!(
            DocumentFormat::from_filename(relative_path),
            Some(DocumentFormat::Markdown | DocumentFormat::PlainText)
        )
}

/// Which files a folder source selects: its glob, or the default set.
#[derive(Debug)]
enum Selection {
    Glob { pattern: String, matcher: Regex },
    Default,
}

impl Selection {
    fn for_source(source: &MemorySourceEntry) -> Result<Self> {
        match source.glob.as_deref() {
            Some(pattern) => Ok(Self::Glob {
                pattern: pattern.to_string(),
                matcher: glob_to_regex(pattern)?,
            }),
            None => Ok(Self::Default),
        }
    }

    fn matches(&self, relative: &str) -> bool {
        match self {
            Self::Glob { matcher, .. } => matcher.is_match(relative),
            Self::Default => is_default_candidate(relative),
        }
    }

    fn describe(&self) -> &str {
        match self {
            Self::Glob { pattern, .. } => pattern,
            Self::Default => "markdown, text and code files",
        }
    }
}

/// A reader over a local folder of files.
#[derive(Debug, Clone, Copy, Default)]
pub struct FolderReader;

impl FolderReader {
    /// The folder root a source reads, resolved against `workspace`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the source has no `path`.
    pub fn root(source: &MemorySourceEntry, workspace: &Path) -> Result<PathBuf> {
        let base_path = source
            .path
            .as_deref()
            .ok_or_else(|| Error::Invalid("folder source requires a path".to_string()))?;
        Ok(resolve_base(base_path, workspace))
    }

    /// Read one listed file's raw bytes, with the same glob, containment and
    /// size checks as [`SourceReader::read_item`].
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a missing path or an id outside the source's
    /// selection, [`Error::NotFound`] for a missing file,
    /// [`Error::PathEscape`] for one that resolves outside the folder, and
    /// [`Error::TooLarge`] for one over the size cap.
    pub fn read_raw(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<LocalFile> {
        let base = Self::root(source, workspace)?;
        let selection = Selection::for_source(source)?;
        let normalized_id = normalize_rel(Path::new(item_id));
        if !selection.matches(&normalized_id) || is_ignored_path(&normalized_id) {
            return Err(Error::Invalid(format!(
                "item '{item_id}' is outside source glob '{}'",
                selection.describe()
            )));
        }

        let file_path = base.join(item_id);
        if !file_path.exists() {
            return Err(Error::NotFound(format!(
                "file not found: {}",
                file_path.display()
            )));
        }

        // Containment is checked against the *resolved* base, the same root
        // the file was joined onto — defends against `..` traversal and
        // symlink escapes.
        let canonical_file = ensure_within_base(&base, &file_path)?;
        read_capped(canonical_file, item_id.to_string())
    }
}

#[async_trait]
impl SourceReader for FolderReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        workspace: &Path,
    ) -> Result<Vec<SourceItem>> {
        let base = Self::root(source, workspace)?;
        if !base.exists() {
            let configured = source.path.as_deref().unwrap_or_default();
            return Err(missing_folder_error(configured, &base));
        }
        let selection = Selection::for_source(source)?;

        let mut items = Vec::new();
        let walker = WalkDir::new(&base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_ignored(entry));
        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(&base) else {
                continue;
            };
            let rel_str = normalize_rel(rel);
            if !selection.matches(&rel_str) {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(path) else {
                continue;
            };
            if metadata.len() > FOLDER_FILE_SIZE_CAP_BYTES {
                continue;
            }
            items.push(SourceItem {
                id: rel_str.clone(),
                title: rel_str,
                updated_at_ms: modified_at(&metadata).map(|at| at.timestamp_millis()),
            });
        }
        log::debug!(
            "[memory_sources:folder] listed {} items under {}",
            items.len(),
            base.display()
        );
        Ok(items)
    }

    /// Read one file's content by its `item_id` (the slash-normalised path
    /// relative to the source's `path`, as produced by
    /// [`list_items`](Self::list_items)).
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<SourceContent> {
        let file = self.read_raw(source, item_id, workspace)?;
        let body = file.text()?;
        Ok(SourceContent {
            id: item_id.to_string(),
            title: item_id.to_string(),
            body,
            content_type: content_type_for(item_id),
            metadata: serde_json::json!({}),
        })
    }

    async fn read_store_item(
        &self,
        source: &MemorySourceEntry,
        item: &SourceItem,
        workspace: &Path,
        converter: &dyn DocumentConverter,
    ) -> Result<StoreItem> {
        let file = self.read_raw(source, &item.id, workspace)?;
        let mut meta = items::base_meta(source);
        meta.workspace = Some(workspace.display().to_string());
        items::local_file_item(file, meta, converter).await
    }
}

/// The content type a file's extension implies.
fn content_type_for(item_id: &str) -> ContentType {
    let lower = item_id.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        ContentType::Markdown
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        ContentType::Html
    } else {
        ContentType::Plaintext
    }
}

/// Whether a walk entry (below the root) is skipped: an ignored directory, or
/// any hidden file or directory.
fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || (entry.file_type().is_dir() && IGNORED_DIRS.contains(&name.as_ref()))
}

/// Whether a relative id passes through a component the walk would skip, so
/// `read_item` refuses exactly what `list_items` never lists. `..` is left to
/// the containment check, which reports it as a path escape.
fn is_ignored_path(relative: &str) -> bool {
    let mut components = relative.split('/').peekable();
    while let Some(component) = components.next() {
        let is_dir = components.peek().is_some();
        let hidden = component.starts_with('.') && component != "..";
        if hidden || (is_dir && IGNORED_DIRS.contains(&component)) {
            return true;
        }
    }
    false
}

/// Normalise a relative path to forward slashes for glob matching.
fn normalize_rel(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compile a shell-style glob into an anchored [`Regex`] matched against a
/// slash-normalised relative path.
///
/// Supported syntax: `*` (any run of non-separator chars), `?` (one
/// non-separator char), `**` (any run including separators), and `**/` (zero or
/// more leading directories). All other regex metacharacters are escaped.
fn glob_to_regex(pattern: &str) -> Result<Regex> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        // `**/` — zero or more leading directories.
                        re.push_str("(?:.*/)?");
                        i += 3;
                    } else {
                        // `**` — any run including separators.
                        re.push_str(".*");
                        i += 2;
                    }
                } else {
                    // `*` — any run excluding separators.
                    re.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            '/' => {
                re.push('/');
                i += 1;
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(c);
                i += 1;
            }
            other => {
                re.push(other);
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| Error::Invalid(format!("invalid glob pattern: {e}")))
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
