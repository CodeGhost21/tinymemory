//! The configured-source registry.
//!
//! Sources are persisted as `[[memory_sources]]` entries in a TOML config file
//! (typically `config.toml`). In OpenHuman this lived on a large shared `Config`
//! struct loaded through an async RPC; this crate does not own that global
//! config, so the registry here is a small self-contained reader/writer over a
//! single TOML file. Other top-level keys in the file are preserved across
//! writes — only the `memory_sources` array is rewritten.
//!
//! Every mutation follows the spec's atomic load-modify-validate-save cycle:
//! load the current file, apply the change in memory, validate, and persist.
//! Each on-disk write (`SourceRegistry::atomic_write`) is atomic (temp file +
//! rename), so a crash mid-write cannot leave a truncated `config.toml`.
//!
//! Because that rename replaces a file the *host* also writes, this registry
//! owes the host its permission contract as well as its contents: the temp file
//! is created owner-only, so a source mutation cannot hand back a config that is
//! more permissive than the one it replaced. See `create_owner_only`, which is
//! private, so this is a plain reference rather than an intra-doc link.
//!
//! The complete load-modify-save cycle is guarded by a process-wide mutation
//! lock, so separate [`SourceRegistry`] handles cannot overwrite one another's
//! in-process updates. Atomic rename protects each individual disk write.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::error::{Error, Result};

use super::types::{MemorySourceEntry, MemorySourcePatch, SourceKind};

/// Wrap a registry I/O or codec failure, naming what was being done.
fn registry_error(action: impl std::fmt::Display, error: impl std::fmt::Display) -> Error {
    Error::Registry(format!("{action}: {error}"))
}

/// Serializes each registry load-modify-save transaction in this process.
///
/// A single lock deliberately covers every path: registry mutation is rare,
/// and correctness is more important than allowing unrelated config files to
/// race through their atomic renames. The on-disk rename remains the crash-
/// safety boundary; this mutex closes the in-process lost-update window.
static REGISTRY_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn mutation_guard() -> std::sync::MutexGuard<'static, ()> {
    // A poisoned lock means a previous mutation panicked while holding it. The
    // guard's data is `()`, so there is nothing torn to inherit -- recover and
    // continue rather than cascading the panic into every later mutation.
    REGISTRY_MUTATION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Conservative default sync caps for a Composio toolkit, keyed by toolkit slug.
///
/// Single source of truth for the cheap out-of-the-box sync volume. Applied to a
/// source entry when it is first registered. Never overwrites a user-customised
/// cap. Returns `(max_items, sync_depth_days)`.
#[must_use]
pub fn memory_sync_defaults_for_toolkit(toolkit: &str) -> (Option<u32>, Option<u32>) {
    match toolkit {
        "gmail" => (Some(100), Some(30)),
        "slack" => (Some(50), Some(14)),
        "notion" => (Some(30), Some(30)),
        "linear" => (Some(50), Some(30)),
        "clickup" => (Some(50), Some(30)),
        "github" => (Some(50), Some(30)),
        // Generic fallback for any toolkit not listed above.
        _ => (Some(30), Some(14)),
    }
}

/// Apply conservative per-kind cap defaults to a new source entry.
///
/// Only fills fields that are still `None` — never overwrites a caller-supplied
/// value, so re-running it over an entry a user has already tuned is a no-op.
/// The retroactive Composio migration applies the same reasoning through
/// [`memory_sync_defaults_for_toolkit`], which is why the two live together:
/// creation time and migration time must agree, and they only do if the policy
/// has one address.
///
/// Folder, web-page and Composio kinds have nothing to fill here. Composio caps
/// are set at upsert time from the toolkit slug, which this function does not
/// have.
pub fn apply_kind_defaults(entry: &mut MemorySourceEntry) {
    match entry.kind {
        SourceKind::GithubRepo => {
            if entry.max_prs.is_none() {
                entry.max_prs = Some(10);
            }
            if entry.max_issues.is_none() {
                entry.max_issues = Some(10);
            }
            if entry.max_commits.is_none() {
                entry.max_commits = Some(50);
            }
        }
        SourceKind::RssFeed if entry.max_items.is_none() => {
            entry.max_items = Some(20);
        }
        _ => {}
    }
}

/// A registry of [`MemorySourceEntry`] values backed by a TOML config file.
///
/// Construct one with [`SourceRegistry::new`], pointing at the `config.toml`
/// path. The file need not exist yet — reads return an empty list and the first
/// write creates it (and any missing parent directories).
#[derive(Debug, Clone)]
pub struct SourceRegistry {
    path: PathBuf,
}

/// Create `path` for writing, restricted to the owner on platforms that have
/// file modes, and refusing to reuse anything already at that path.
///
/// The mode is part of the `open(2)` call rather than a `chmod` afterwards, so
/// the file is never even momentarily group- or world-readable. That matters
/// because [`SourceRegistry::atomic_write`] renames this temp file over the
/// host's `config.toml`, and a rename carries the *source* file's mode onto the
/// destination: a temp file created at the default `0o666 & ~umask` (0644 under
/// the usual 022) silently re-widens the live config on every source mutation,
/// undoing any hardening the host applied when it wrote that file itself.
///
/// `create_new` is deliberate too. The caller already names the temp file with a
/// fresh UUID, so a collision means something else put a file — or a symlink —
/// where this one was about to go, and failing is the safe answer.
///
/// Note that `mode` is masked by the process umask, so a pathological umask can
/// make the result *narrower* than `0o600`. That is not a weakening, and it is
/// the same property every other umask-respecting create in the tree has.
fn create_owner_only(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

impl SourceRegistry {
    /// Create a registry persisted at `config_path`.
    #[must_use]
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            path: config_path.into(),
        }
    }

    /// The config file path this registry reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole config file into a TOML table (empty if it doesn't exist).
    fn read_table(&self) -> Result<toml::Table> {
        if !self.path.exists() {
            return Ok(toml::Table::new());
        }
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| registry_error(format!("failed to read {}", self.path.display()), e))?;
        let table: toml::Table = toml::from_str(&text)
            .map_err(|e| registry_error(format!("failed to parse {}", self.path.display()), e))?;
        Ok(table)
    }

    /// List all configured sources.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or decoded.
    pub fn list(&self) -> Result<Vec<MemorySourceEntry>> {
        let table = self.read_table()?;
        match table.get("memory_sources") {
            Some(value) => value
                .clone()
                .try_into()
                .map_err(|e| registry_error("failed to decode [[memory_sources]]", e)),
            None => Ok(Vec::new()),
        }
    }

    /// List enabled sources of a given [`SourceKind`].
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or decoded.
    pub fn list_enabled_by_kind(&self, kind: SourceKind) -> Result<Vec<MemorySourceEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.kind == kind && s.enabled)
            .collect())
    }

    /// Get a single source by id, if present.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or decoded.
    pub fn get(&self, id: &str) -> Result<Option<MemorySourceEntry>> {
        Ok(self.list()?.into_iter().find(|s| s.id == id))
    }

    /// Persist the full source list, preserving any other top-level config
    /// keys.
    ///
    /// Writes are atomic: the new TOML is written to a same-directory temp file
    /// and then renamed over the config. This keeps a failed/crashed write from
    /// leaving a truncated `config.toml`, matching the OpenHuman source
    /// registry contract. The temp file is created owner-only so the rename
    /// cannot widen the live config — see [`create_owner_only`].
    ///
    /// Mutation callers hold [`REGISTRY_MUTATION_LOCK`] across their initial
    /// read and this preserving re-read, keeping the two snapshots ordered with
    /// respect to every other in-process writer.
    fn write_all(&self, entries: &[MemorySourceEntry]) -> Result<()> {
        let mut table = self.read_table()?;
        let value = toml::Value::try_from(entries)
            .map_err(|e| registry_error("failed to encode memory_sources", e))?;
        table.insert("memory_sources".to_string(), value);
        let text = toml::to_string_pretty(&table)
            .map_err(|e| registry_error("failed to serialize config", e))?;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    registry_error(format!("failed to create {}", parent.display()), e)
                })?;
            }
        }
        self.atomic_write(text.as_bytes())?;
        Ok(())
    }

    fn atomic_write(&self, bytes: &[u8]) -> Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let filename = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                Error::Registry(format!(
                    "config path has no file name: {}",
                    self.path.display()
                ))
            })?;
        let tmp_path = parent.join(format!(
            ".{filename}.tmp-{}",
            uuid::Uuid::new_v4().as_simple()
        ));

        let write_result = (|| -> Result<()> {
            {
                let mut file = create_owner_only(&tmp_path).map_err(|e| {
                    registry_error(format!("failed to create {}", tmp_path.display()), e)
                })?;
                use std::io::Write;
                file.write_all(bytes).map_err(|e| {
                    registry_error(format!("failed to write {}", tmp_path.display()), e)
                })?;
                file.sync_all().map_err(|e| {
                    registry_error(format!("failed to sync {}", tmp_path.display()), e)
                })?;
            }
            std::fs::rename(&tmp_path, &self.path).map_err(|e| {
                registry_error(
                    format!(
                        "failed to atomically replace {} with {}",
                        self.path.display(),
                        tmp_path.display()
                    ),
                    e,
                )
            })?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        write_result
    }

    /// Validate and add a new source. Fails if the id already exists.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an entry that fails validation or reuses an id,
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn add(&self, entry: MemorySourceEntry) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        entry.validate()?;
        let mut sources = self.list()?;
        if sources.iter().any(|s| s.id == entry.id) {
            return Err(Error::Invalid(format!(
                "source with id '{}' already exists",
                entry.id
            )));
        }
        sources.push(entry.clone());
        self.write_all(&sources)?;
        Ok(entry)
    }

    /// Apply a [`MemorySourcePatch`] to an existing source, then re-validate and
    /// save. Fails if no source has the given id.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown id, [`Error::Invalid`] for a patch
    /// field the kind does not use or a result that fails validation,
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn update(&self, id: &str, patch: MemorySourcePatch) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let entry = sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::NotFound(format!("source '{id}' not found")))?;

        patch.validate_for_kind(entry.kind.clone())?;
        patch.apply_to(entry);
        entry.validate()?;
        let updated = entry.clone();
        self.write_all(&sources)?;
        Ok(updated)
    }

    /// Remove a source by id. Returns `true` if an entry was removed.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let before = sources.len();
        sources.retain(|s| s.id != id);
        let removed = sources.len() < before;
        if removed {
            self.write_all(&sources)?;
        }
        Ok(removed)
    }

    /// Remove every composio source bound to `connection_id`. Returns the count
    /// removed. Mirrors [`SourceRegistry::upsert_composio_source`], which keys
    /// composio sources on `connection_id` rather than the `src_*` id.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn remove_composio_source_by_connection_id(&self, connection_id: &str) -> Result<usize> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let before = sources.len();
        sources.retain(|s| {
            !(s.kind == SourceKind::Composio && s.connection_id.as_deref() == Some(connection_id))
        });
        let removed = before - sources.len();
        if removed > 0 {
            self.write_all(&sources)?;
        }
        Ok(removed)
    }

    /// Upsert a composio source keyed on `connection_id`.
    ///
    /// If a source with the same `connection_id` exists, its label is updated;
    /// otherwise a new entry is inserted with conservative per-toolkit caps. The
    /// update path never clobbers user-customised caps.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn upsert_composio_source(
        &self,
        toolkit: &str,
        connection_id: &str,
        label: &str,
    ) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let (entry, _was_insert) =
            upsert_composio_entry_in_place(&mut sources, toolkit, connection_id, label);
        self.write_all(&sources)?;
        Ok(entry)
    }

    /// Batch-upsert Composio sources with one load and one atomic save.
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn upsert_composio_sources_batch(&self, targets: &[ComposioUpsertTarget]) -> Result<u32> {
        if targets.is_empty() {
            return Ok(0);
        }
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        for (toolkit, connection_id, label) in targets {
            upsert_composio_entry_in_place(&mut sources, toolkit, connection_id, label);
        }
        self.write_all(&sources)?;
        Ok(targets.len().min(u32::MAX as usize) as u32)
    }

    /// Replace the whole registry with `entries`, validating each first.
    ///
    /// The write-through behind a host-config view whose `memory_sources_json`
    /// reads this file: a setter that only updated an in-memory snapshot would
    /// be invisible to the very next getter (openhuman#5820). Same atomic
    /// load-modify-validate-save cycle as the other mutations, so other
    /// top-level keys in the file are preserved.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when an entry fails validation, [`Error::Registry`]
    /// when the file cannot be read, parsed, serialized or atomically replaced.
    pub fn replace_all(&self, entries: &[MemorySourceEntry]) -> Result<()> {
        let _guard = mutation_guard();
        for entry in entries {
            entry.validate().map_err(|reason| {
                Error::Invalid(format!("invalid memory source `{}`: {reason}", entry.id))
            })?;
        }
        self.write_all(entries)
    }

    /// Enable every source and clear all per-source caps ("All In" mode).
    ///
    /// # Errors
    ///
    /// [`Error::Registry`] when the file cannot be read or written.
    pub fn apply_all_in(&self) -> Result<Vec<MemorySourceEntry>> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        for source in &mut sources {
            source.enabled = true;
            source.max_items = None;
            source.sync_depth_days = None;
            source.max_commits = None;
            source.max_issues = None;
            source.max_prs = None;
            source.max_tokens_per_sync = None;
            source.max_cost_per_sync_usd = None;
        }
        self.write_all(&sources)?;
        Ok(sources)
    }
}

/// `(toolkit, account_id, label)` — the three fields that identify which
/// Composio account a source upserts into.
pub type ComposioUpsertTarget = (String, String, String);

/// Apply a single composio upsert to an in-memory source list.
///
/// Pure (no I/O) so the registry path and unit tests share one find-or-push
/// predicate. Returns the resulting entry and whether it was a fresh insert.
pub(crate) fn upsert_composio_entry_in_place(
    sources: &mut Vec<MemorySourceEntry>,
    toolkit: &str,
    connection_id: &str,
    label: &str,
) -> (MemorySourceEntry, bool) {
    if let Some(existing) = sources.iter_mut().find(|s| {
        s.kind == SourceKind::Composio && s.connection_id.as_deref() == Some(connection_id)
    }) {
        existing.label = label.to_string();
        return (existing.clone(), false);
    }

    let (default_max_items, default_sync_depth_days) = memory_sync_defaults_for_toolkit(toolkit);
    let entry = MemorySourceEntry {
        toolkit: Some(toolkit.to_string()),
        connection_id: Some(connection_id.to_string()),
        max_items: default_max_items,
        sync_depth_days: default_sync_depth_days,
        ..MemorySourceEntry::new(
            format!("src_{}", uuid::Uuid::new_v4().as_simple()),
            SourceKind::Composio,
            label,
        )
    };
    sources.push(entry.clone());
    (entry, true)
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
