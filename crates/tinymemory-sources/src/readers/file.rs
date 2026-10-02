//! Single-file source reader.
//!
//! A `file` source names one file by `path` (absolute, or relative to the
//! workspace). It lists exactly one item — the file's name — and reads it with
//! the same size cap as the folder reader.
//!
//! [`FileReader::read_path`] reads a file with no configured source at all,
//! for a host that was handed a path (a drag-and-drop, a CLI argument);
//! [`crate::items::file_item`] turns that straight into a `StoreItem`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tinymemory_api::StoreItem;
use tinymemory_documents::DocumentConverter;

use crate::error::{Error, Result};
use crate::items;
use crate::types::{ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind};

use super::SourceReader;
use super::local_file::{LocalFile, modified_at, read_capped, resolve_base};

/// A reader over one local file.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileReader;

impl FileReader {
    /// The file a source names, resolved against `workspace`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the source has no `path`.
    pub fn resolve(source: &MemorySourceEntry, workspace: &Path) -> Result<PathBuf> {
        let path = source
            .path
            .as_deref()
            .ok_or_else(|| Error::Invalid("file source requires a path".to_string()))?;
        Ok(resolve_base(path, workspace))
    }

    /// Read the file at `path`, with no configured source.
    ///
    /// The path is canonicalised, so the returned [`LocalFile::path`] is
    /// absolute with symlinks resolved; its id is the file name.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for a missing file, [`Error::Invalid`] for a path
    /// that is not a regular file, [`Error::TooLarge`] for one over
    /// [`crate::FOLDER_FILE_SIZE_CAP_BYTES`], and [`Error::Io`] for a read
    /// failure.
    pub fn read_path(path: &Path) -> Result<LocalFile> {
        if !path.exists() {
            return Err(Error::NotFound(format!(
                "file not found: {}",
                path.display()
            )));
        }
        let canonical = std::fs::canonicalize(path)?;
        if !canonical.is_file() {
            return Err(Error::Invalid(format!(
                "not a regular file: {}",
                canonical.display()
            )));
        }
        let id = file_name(&canonical);
        read_capped(canonical, id)
    }

    /// Check `item_id` names this source's file and read it.
    fn read_listed(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<LocalFile> {
        let path = Self::resolve(source, workspace)?;
        let expected = file_name(&path);
        if item_id != expected {
            return Err(Error::NotFound(format!(
                "item '{item_id}' is not this source's file '{expected}'"
            )));
        }
        Self::read_path(&path)
    }
}

/// The final component of `path`, lossily decoded.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[async_trait]
impl SourceReader for FileReader {
    fn kind(&self) -> SourceKind {
        SourceKind::File
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        workspace: &Path,
    ) -> Result<Vec<SourceItem>> {
        let path = Self::resolve(source, workspace)?;
        let metadata = std::fs::metadata(&path)
            .map_err(|_| Error::NotFound(format!("file not found: {}", path.display())))?;
        let id = file_name(&path);
        Ok(vec![SourceItem {
            title: id.clone(),
            id,
            updated_at_ms: modified_at(&metadata).map(|at| at.timestamp_millis()),
        }])
    }

    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        workspace: &Path,
    ) -> Result<SourceContent> {
        let file = self.read_listed(source, item_id, workspace)?;
        let body = file.text()?;
        let lower = item_id.to_ascii_lowercase();
        let content_type = if lower.ends_with(".md") || lower.ends_with(".markdown") {
            ContentType::Markdown
        } else if lower.ends_with(".html") || lower.ends_with(".htm") {
            ContentType::Html
        } else {
            ContentType::Plaintext
        };
        Ok(SourceContent {
            id: item_id.to_string(),
            title: item_id.to_string(),
            body,
            content_type,
            metadata: serde_json::json!({ "path": file.path.display().to_string() }),
        })
    }

    async fn read_store_item(
        &self,
        source: &MemorySourceEntry,
        item: &SourceItem,
        workspace: &Path,
        converter: &dyn DocumentConverter,
    ) -> Result<StoreItem> {
        let file = self.read_listed(source, &item.id, workspace)?;
        let mut meta = items::base_meta(source);
        meta.workspace = Some(workspace.display().to_string());
        items::local_file_item(file, None, meta, converter).await
    }
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
