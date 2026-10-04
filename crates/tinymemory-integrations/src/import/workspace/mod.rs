//! Opening a v1 TinyCortex workspace.
//!
//! [`LegacyWorkspace::open`] refuses anything that is not a v1 store: the
//! directory must hold `memory/memory.db`, a SQLite database with the
//! `memory_docs`, `episodic_log` and `user_profile` tables and the columns
//! the importer reads. Columns that later v1 migrations added (`taint`,
//! `logical_namespace`, the profile columns from `state` on, the chunk
//! `content_path`) are probed and used when present.
//!
//! `memory_tree/chunks.db` is optional. When it is missing, unreadable as
//! SQLite, or lacks `mem_tree_chunks`, the chunk section is skipped silently.
//!
//! Both databases are opened read-only; the importer never writes to a legacy
//! workspace.

mod schema;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

pub(crate) use schema::{ChunkStore, MemorySchema};

use crate::import::checkpoint::Checkpoint;
use crate::import::error::{Error, Result};
use crate::import::items::Items;

/// A v1 TinyCortex workspace opened for import.
#[derive(Debug)]
pub struct LegacyWorkspace {
    /// Canonical workspace root.
    pub(crate) root: PathBuf,
    /// `root` as recorded in every item's `meta.workspace`.
    pub(crate) workspace_id: String,
    /// `memory/memory.db`, read-only.
    pub(crate) memory: Connection,
    /// Which optional columns `memory.db` has.
    pub(crate) schema: MemorySchema,
    /// `memory_tree/chunks.db`, when present and usable.
    pub(crate) chunks: Option<ChunkStore>,
}

impl LegacyWorkspace {
    /// Opens the v1 workspace rooted at `path` (the directory that contains
    /// `memory/memory.db`).
    ///
    /// # Errors
    ///
    /// - [`Error::NotFound`] if `path` does not exist.
    /// - [`Error::NotLegacy`] if `path` is not a directory, has no
    ///   `memory/memory.db`, that file is not SQLite, or it lacks a required
    ///   table or column.
    /// - [`Error::Io`] if the path cannot be canonicalised.
    /// - [`Error::Sqlite`] if the database cannot be opened or probed for a
    ///   reason other than not being SQLite.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::NotFound {
                path: path.to_path_buf(),
            });
        }
        let root = path.canonicalize().map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if !root.is_dir() {
            return Err(not_legacy(&root, "not a directory"));
        }
        let db = root.join("memory").join("memory.db");
        if !db.is_file() {
            return Err(not_legacy(&root, "memory/memory.db is missing"));
        }
        let memory = open_read_only(&db)?;
        let schema = match MemorySchema::probe(&memory) {
            Ok(Ok(schema)) => schema,
            Ok(Err(reason)) => return Err(not_legacy(&root, &reason)),
            Err(err) if is_not_a_database(&err) => {
                return Err(not_legacy(
                    &root,
                    "memory/memory.db is not a sqlite database",
                ));
            }
            Err(err) => return Err(err.into()),
        };
        let chunks = ChunkStore::open(&root)?;
        Ok(Self {
            workspace_id: root.display().to_string(),
            root,
            memory,
            schema,
            chunks,
        })
    }

    /// The canonical workspace root, recorded as `meta.workspace` on every
    /// imported item.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Whether `memory_tree/chunks.db` is present and will be imported.
    #[must_use]
    pub fn has_chunks(&self) -> bool {
        self.chunks.is_some()
    }

    /// Every importable item, from the beginning.
    #[must_use]
    pub fn items(&self) -> Items<'_> {
        Items::new(self, Checkpoint::default())
    }

    /// The items after `checkpoint`: exactly what [`Self::items`] would yield
    /// once the items up to and including the checkpoint's are dropped,
    /// provided the legacy store has not changed in between.
    #[must_use]
    pub fn items_from(&self, checkpoint: &Checkpoint) -> Items<'_> {
        Items::new(self, checkpoint.clone())
    }
}

/// Opens a SQLite file read-only.
pub(crate) fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Whether SQLite refused the file as not being a database.
pub(crate) fn is_not_a_database(err: &rusqlite::Error) -> bool {
    err.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase)
}

fn not_legacy(root: &Path, reason: &str) -> Error {
    Error::NotLegacy {
        path: root.to_path_buf(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
