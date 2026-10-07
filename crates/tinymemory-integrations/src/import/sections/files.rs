//! Workspace files: the goals document and the persona directives.
//!
//! The later v1 engine kept two small markdown files beside its stores:
//! `MEMORY_GOALS.md` (the user's long-term goals, at most about 2,000
//! characters) and `persona/directives.md` (how the assistant should behave,
//! compiled from the persona). Each that exists and has text becomes one
//! [`LearningKind::Other`] learning of its whole text, tagged with the
//! file's name (`goals`, `persona`), observed at the file's modification
//! time. A missing file is skipped; one that cannot be read is an error. At
//! most [`MAX_FILE_BYTES`] of a file are read: a longer one is cut there (at a
//! character boundary) and also tagged `truncated`.

use std::io::ErrorKind;

use tinymemory_api::{LearningKind, StoreItem};

use super::{Mark, Scanned, import_meta};
use crate::import::convert;
use crate::import::error::{Error, Result};
use crate::import::workspace::LegacyWorkspace;

/// The files, in the order they are read: `(name, path below the root)`.
const FILES: [(&str, &str); 2] = [
    ("goals", "MEMORY_GOALS.md"),
    ("persona", "persona/directives.md"),
];

/// Most bytes of a file read: far above a real goals document (about 2,000
/// characters) or a persona's directives, and a bound on what an oversized
/// or substituted file can make the importer load.
pub(crate) const MAX_FILE_BYTES: u64 = 256 * 1024;

/// At most `limit` of the files after `after` (by name).
pub(super) fn page(
    ws: &LegacyWorkspace,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Scanned>> {
    let start = after.map_or(0, |after| {
        FILES
            .iter()
            .position(|(name, _)| *name == after)
            .map_or(FILES.len(), |index| index + 1)
    });
    FILES[start..]
        .iter()
        .take(limit)
        .map(|(name, relative)| {
            Ok(Scanned {
                item: file(ws, name, relative)?,
                mark: Mark::File((*name).to_string()),
            })
        })
        .collect()
}

/// The files that exist and have text.
pub(super) fn count(ws: &LegacyWorkspace) -> Result<u64> {
    let mut total = 0;
    for (name, relative) in FILES {
        if file(ws, name, relative)?.is_some() {
            total += 1;
        }
    }
    Ok(total)
}

fn file(ws: &LegacyWorkspace, name: &str, relative: &str) -> Result<Option<StoreItem>> {
    let path = ws.root.join(relative);
    let (text, truncated) = match read_bounded(&path) {
        Ok(read) => read,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::Io { path, source }),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let mut meta = import_meta(ws, format!("file:{relative}"));
    meta.tags = vec![name.to_string()];
    if truncated {
        meta.tags.push("truncated".to_string());
    }
    meta.observed_at = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| convert::from_unix_seconds(since.as_secs_f64()));
    Ok(Some(StoreItem::Learning {
        text: text.trim().to_string(),
        kind: LearningKind::Other,
        confidence: convert::DEFAULT_CONFIDENCE,
        evidence: None,
        meta,
    }))
}

/// Reads at most [`MAX_FILE_BYTES`] of `path` as text, cut back to the last
/// whole character, and whether anything was left unread.
fn read_bounded(path: &std::path::Path) -> std::io::Result<(String, bool)> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > MAX_FILE_BYTES;
    if truncated {
        bytes.truncate(usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // A cut can split the last character; anything else is not text.
        Err(error) if !truncated => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
        Err(error) => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }
    };
    Ok((text, truncated))
}
