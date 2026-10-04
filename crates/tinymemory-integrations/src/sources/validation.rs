//! Field rules for a configured source, and the path-containment guard the
//! local readers share.

use std::path::{Path, PathBuf};

use crate::sources::error::{Error, Result};

use super::types::{MemorySourceEntry, SourceKind};

/// Validate required fields for `entry` based on its [`SourceKind`].
///
/// `id` and `label` are required for every kind; kind-specific fields follow.
///
/// # Errors
///
/// [`Error::Invalid`] with a human-readable message naming the first failing
/// rule.
pub fn validate_entry(entry: &MemorySourceEntry) -> Result<()> {
    if entry.id.trim().is_empty() {
        return Err(Error::Invalid("id is required".to_string()));
    }
    if entry.id.contains(':') || entry.id.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "id must not contain ':' or control characters".to_string(),
        ));
    }
    if entry.label.is_empty() {
        return Err(Error::Invalid("label is required".to_string()));
    }
    match entry.kind {
        SourceKind::Composio => {
            require_field(&entry.toolkit, "toolkit")?;
            require_field(&entry.connection_id, "connection_id")?;
        }
        SourceKind::Conversation => {
            // No kind-specific required fields — just enabled/disabled.
        }
        SourceKind::Folder | SourceKind::File => {
            require_field(&entry.path, "path")?;
        }
        SourceKind::GithubRepo => {
            require_field(&entry.url, "url")?;
        }
        SourceKind::RssFeed => {
            require_field(&entry.url, "url")?;
        }
        SourceKind::WebPage => {
            require_field(&entry.url, "url")?;
        }
    }
    Ok(())
}

/// Require that `value` is present and non-empty, naming it `name` in errors.
fn require_field(value: &Option<String>, name: &str) -> Result<()> {
    match value {
        Some(v) if !v.is_empty() => Ok(()),
        _ => Err(Error::Invalid(format!(
            "{name} is required for this source kind"
        ))),
    }
}

/// Canonicalize `target` and ensure it stays within canonicalized `base`.
///
/// This is the shared path-traversal guard for local readers. Both paths must
/// exist (they are passed through [`std::fs::canonicalize`], which resolves
/// symlinks and `..` segments). If the resolved target escapes the base
/// directory, the guard refuses it.
///
/// # Errors
///
/// [`Error::PathEscape`] carrying `"path traversal denied"` when the target
/// escapes, [`Error::Io`] when either path cannot be canonicalised.
pub fn ensure_within_base(base: &Path, target: &Path) -> Result<PathBuf> {
    let canonical_base = std::fs::canonicalize(base)?;
    let canonical_target = std::fs::canonicalize(target)?;
    if !canonical_target.starts_with(&canonical_base) {
        return Err(Error::PathEscape("path traversal denied".to_string()));
    }
    Ok(canonical_target)
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
