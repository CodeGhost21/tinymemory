//! `tinymemory-import` — one-shot importers that read another assistant's
//! workspace and write what they find into a [`Memory`].
//!
//! Two sources are supported: **OpenClaw** (`memory/brain.db`, `MEMORY.md`,
//! `memory/*.md`) and **Hermes** (`MEMORY.md`, `USER.md`, `SOUL.md`). Both share
//! one shape: resolve the source workspace, refuse a self-migration, collect
//! entries, and, unless `dry_run`, back up the target's existing memory files
//! and write each entry, skipping content that is already present and renaming
//! a key that collides with different content.
//!
//! Which [`Memory`] to write into is the host's decision and arrives as a
//! closure (`open_target`), called only once there is something to write and
//! after the backup, so a host that refuses (for instance a null driver that
//! would discard every write) refuses at exactly the point it always did.

mod keys;
mod source;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use tinymemory_api::traits::Memory;
use tinymemory_api::types::MemoryCategory;

use keys::{backup_target_memory, next_available_key, paths_equal};
use source::{collect_source_entries, hermes_file_mappings};

/// One importable memory, before it is written.
#[derive(Debug, Clone)]
pub(crate) struct SourceEntry {
    pub(crate) key: String,
    pub(crate) content: String,
    pub(crate) category: MemoryCategory,
}

/// What an import read and wrote.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    /// Entries read from the source's SQLite database (OpenClaw only).
    pub from_sqlite: usize,
    /// Entries read from markdown files.
    pub from_markdown: usize,
    /// Entries written to the target.
    pub imported: usize,
    /// Entries skipped because the target already held identical content.
    pub skipped_unchanged: usize,
    /// Entries written under a new key because the key held different content.
    pub renamed_conflicts: usize,
}

/// The outcome of one import run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    /// The workspace that was read.
    pub source_workspace: PathBuf,
    /// The workspace the entries were (or, in a dry run, would be) written to.
    pub target_workspace: PathBuf,
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Counts.
    pub stats: MigrationStats,
    /// Non-fatal observations (missing files, the backup location, ...).
    pub warnings: Vec<String>,
}

/// Where an OpenClaw workspace lives when the caller names none.
pub fn resolve_openclaw_workspace(source: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = source {
        return Ok(path);
    }

    let Some(user_dirs) = UserDirs::new() else {
        bail!("Failed to determine user home directory");
    };

    Ok(user_dirs.home_dir().join(".openclaw").join("workspace"))
}

/// Where a Hermes workspace lives when the caller names none.
pub fn resolve_hermes_workspace(source: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = source {
        return Ok(path);
    }

    let Some(user_dirs) = UserDirs::new() else {
        bail!("Failed to determine user home directory");
    };

    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(local_app_data).join("hermes"));
        }
    }

    Ok(user_dirs.home_dir().join(".hermes"))
}

/// Import an OpenClaw workspace into the target.
///
/// `source_workspace` defaults to `~/.openclaw/workspace`. `open_target` yields
/// the memory to write into; it is only called when there is something to
/// import and this is not a dry run, after the backup.
pub async fn migrate_openclaw_memory(
    target_workspace: &Path,
    source_workspace: Option<PathBuf>,
    dry_run: bool,
    open_target: impl FnOnce() -> Result<Arc<dyn Memory>>,
) -> Result<MigrationReport> {
    let source_workspace = resolve_openclaw_workspace(source_workspace)?;
    if !source_workspace.exists() {
        bail!(
            "OpenClaw workspace not found at {}. Provide a valid source workspace.",
            source_workspace.display()
        );
    }

    if paths_equal(&source_workspace, target_workspace) {
        bail!("Source workspace matches current OpenHuman workspace; refusing self-migration");
    }

    let mut stats = MigrationStats::default();
    let entries = collect_source_entries(&source_workspace, &mut stats)?;
    let mut warnings = Vec::new();

    if entries.is_empty() {
        warnings.push(format!(
            "No importable memory found in {}",
            source_workspace.display()
        ));
        warnings.push("Checked for: memory/brain.db, MEMORY.md, memory/*.md".to_string());
        return Ok(MigrationReport {
            source_workspace,
            target_workspace: target_workspace.to_path_buf(),
            dry_run,
            stats,
            warnings,
        });
    }

    if dry_run {
        return Ok(MigrationReport {
            source_workspace,
            target_workspace: target_workspace.to_path_buf(),
            dry_run,
            stats,
            warnings,
        });
    }

    if let Some(backup_dir) = backup_target_memory(target_workspace)? {
        warnings.push(format!("Backup created: {}", backup_dir.display()));
    }

    let memory = open_target()?;

    for (idx, entry) in entries.into_iter().enumerate() {
        let mut key = entry.key.trim().to_string();
        if key.is_empty() {
            key = format!("openclaw_{idx}");
        }

        if let Some(existing) = memory.get("", &key).await? {
            if existing.content.trim() == entry.content.trim() {
                stats.skipped_unchanged += 1;
                continue;
            }

            let renamed = next_available_key(memory.as_ref(), &key).await?;
            key = renamed;
            stats.renamed_conflicts += 1;
        }

        memory
            .store("", &key, &entry.content, entry.category, None)
            .await?;
        stats.imported += 1;
    }

    Ok(MigrationReport {
        source_workspace,
        target_workspace: target_workspace.to_path_buf(),
        dry_run,
        stats,
        warnings,
    })
}

/// Import a Hermes workspace (`MEMORY.md`, `USER.md`, `SOUL.md`) into the target.
///
/// `source_workspace` defaults to `~/.hermes` (`%LOCALAPPDATA%\\hermes` on
/// Windows). `open_target` is called under the same conditions as in
/// [`migrate_openclaw_memory`].
pub async fn migrate_hermes_memory(
    target_workspace: &Path,
    source_workspace: Option<PathBuf>,
    dry_run: bool,
    open_target: impl FnOnce() -> Result<Arc<dyn Memory>>,
) -> Result<MigrationReport> {
    let source_workspace = resolve_hermes_workspace(source_workspace)?;
    if !source_workspace.exists() {
        bail!(
            "Hermes workspace not found at {}. Provide a valid source workspace.",
            source_workspace.display()
        );
    }

    if paths_equal(&source_workspace, target_workspace) {
        bail!("Source workspace matches current OpenHuman workspace; refusing self-migration");
    }

    let mut stats = MigrationStats::default();
    let mut warnings = Vec::new();
    let mut entries = Vec::new();

    for (filename, key, category) in hermes_file_mappings() {
        let path = source_workspace.join(filename);
        if !path.exists() {
            warnings.push(format!(
                "{filename} not found in {}",
                source_workspace.display()
            ));
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if content.trim().is_empty() {
            warnings.push(format!("{filename} is empty, skipping"));
            continue;
        }
        entries.push(SourceEntry {
            key: key.to_string(),
            content: content.trim().to_string(),
            category,
        });
    }

    stats.from_markdown = entries.len();

    if entries.is_empty() {
        warnings.push(format!(
            "No importable memory found in {}",
            source_workspace.display()
        ));
        warnings.push("Checked for: MEMORY.md, USER.md, SOUL.md".to_string());
        return Ok(MigrationReport {
            source_workspace,
            target_workspace: target_workspace.to_path_buf(),
            dry_run,
            stats,
            warnings,
        });
    }

    if dry_run {
        return Ok(MigrationReport {
            source_workspace,
            target_workspace: target_workspace.to_path_buf(),
            dry_run,
            stats,
            warnings,
        });
    }

    if let Some(backup_dir) = backup_target_memory(target_workspace)? {
        warnings.push(format!("Backup created: {}", backup_dir.display()));
    }

    let memory = open_target()?;

    for entry in entries {
        let mut key = entry.key;

        if let Some(existing) = memory.get("", &key).await? {
            if existing.content.trim() == entry.content.trim() {
                stats.skipped_unchanged += 1;
                continue;
            }
            let renamed = next_available_key(memory.as_ref(), &key).await?;
            key = renamed;
            stats.renamed_conflicts += 1;
        }

        memory
            .store("", &key, &entry.content, entry.category, None)
            .await?;
        stats.imported += 1;
    }

    Ok(MigrationReport {
        source_workspace,
        target_workspace: target_workspace.to_path_buf(),
        dry_run,
        stats,
        warnings,
    })
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
