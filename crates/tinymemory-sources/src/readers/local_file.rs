//! One local file, read whole and size-capped.
//!
//! The folder and file readers share this: both resolve a configured path
//! against the workspace, both refuse files over
//! [`FOLDER_FILE_SIZE_CAP_BYTES`], and both hand the raw bytes on so a host
//! converter can handle formats that are not UTF-8 text (PDF, DOCX).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tinymemory_documents::RawDocument;

use crate::error::{Error, Result};
use crate::FOLDER_FILE_SIZE_CAP_BYTES;

/// A file read from disk, before any conversion.
#[derive(Debug, Clone)]
pub struct LocalFile {
    /// The canonical absolute path the bytes were read from.
    pub path: PathBuf,
    /// The id the reader listed it under (folder-relative, slash-separated,
    /// or the file name for a single-file source).
    pub id: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Last modification time, when the filesystem reports one.
    pub modified: Option<DateTime<Utc>>,
}

impl LocalFile {
    /// The file as a [`RawDocument`] for conversion: the bytes, named by the
    /// listed id so format and language detection see the extension.
    #[must_use]
    pub fn to_raw_document(&self) -> RawDocument {
        RawDocument::new(self.bytes.clone()).with_filename(self.id.clone())
    }

    /// The body decoded as UTF-8.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] with [`std::io::ErrorKind::InvalidData`] when the file
    /// is not valid UTF-8; it is reported, never lossily decoded.
    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.bytes.clone()).map_err(|error| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("stream did not contain valid UTF-8: {error}"),
            ))
        })
    }
}

/// Resolve a configured path against the workspace.
///
/// An absolute path is taken verbatim. A **relative** path is anchored on the
/// workspace instead of the process working directory, which is whatever
/// directory the host happened to start in (for the desktop app, its build
/// directory — tinyhumansai/openhuman#5830).
pub(crate) fn resolve_base(base_path: &str, workspace: &Path) -> PathBuf {
    let configured = Path::new(base_path);
    if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        workspace.join(configured)
    }
}

/// The modification time of `metadata`, as a UTC instant.
pub(crate) fn modified_at(metadata: &std::fs::Metadata) -> Option<DateTime<Utc>> {
    metadata.modified().ok().map(DateTime::<Utc>::from)
}

/// Read `canonical` (already containment-checked by the caller) whole,
/// refusing it when it is over the size cap.
pub(crate) fn read_capped(canonical: PathBuf, id: String) -> Result<LocalFile> {
    let metadata = std::fs::metadata(&canonical)?;
    if metadata.len() > FOLDER_FILE_SIZE_CAP_BYTES {
        return Err(Error::TooLarge(format!(
            "file exceeds {FOLDER_FILE_SIZE_CAP_BYTES}-byte limit: {}",
            canonical.display()
        )));
    }
    let bytes = std::fs::read(&canonical)?;
    Ok(LocalFile {
        modified: modified_at(&metadata),
        path: canonical,
        id,
        bytes,
    })
}
