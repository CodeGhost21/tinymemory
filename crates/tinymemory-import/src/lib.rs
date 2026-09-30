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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use tinymemory_api::traits::Memory;
use tinymemory_api::types::MemoryCategory;

use keys::{backup_target_memory, next_available_key, paths_equal};

#[derive(Debug, Clone)]
struct SourceEntry {
    key: String,
    content: String,
    category: MemoryCategory,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    pub from_sqlite: usize,
    pub from_markdown: usize,
    pub imported: usize,
    pub skipped_unchanged: usize,
    pub renamed_conflicts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub source_workspace: PathBuf,
    pub target_workspace: PathBuf,
    pub dry_run: bool,
    pub stats: MigrationStats,
    pub warnings: Vec<String>,
}

