//! Opening a v1 TinyCortex workspace.
//!
//! A v1 store is either of two databases, and a workspace may hold both:
//!
//! - `memory/memory.db`, written by the first v1 engine: a SQLite database
//!   with the `memory_docs`, `episodic_log` and `user_profile` tables and
//!   the columns the importer reads. When the file exists it must be one;
//!   anything else is refused.
//! - `memory_tree/chunks.db`, written by the later v1 engine (TinyCortex),
//!   which stopped writing `memory.db`: a store with only this one is a v1
//!   store too. When it is missing, unreadable as SQLite, or lacks a usable
//!   `mem_tree_chunks`, the chunk section is skipped.
//!
//! [`LegacyWorkspace::open`] refuses a directory with neither. Columns that
//! later v1 migrations added (`taint`, `logical_namespace`, the profile
//! columns from `state` on, the chunk `content_path`) are probed and used
//! when present.
//!
//! Both databases are opened read-only; the importer never writes to a legacy
//! workspace.

mod schema;

use std::path::{Path, PathBuf};

use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

pub(crate) use schema::{ChunkStore, MemorySchema};

use crate::import::checkpoint::Checkpoint;
use crate::import::counts::LegacyCounts;
use crate::import::error::{Error, Result};
use crate::import::items::Items;

/// A v1 TinyCortex workspace opened for import.
#[derive(Debug)]
pub struct LegacyWorkspace {
    /// Canonical workspace root.
    pub(crate) root: PathBuf,
    /// `root` as recorded in every item's `meta.workspace`.
    pub(crate) workspace_id: String,
    /// `memory/memory.db`, read-only, when the workspace has one.
    pub(crate) memory: Option<Connection>,
    /// Which optional columns `memory.db` has (all off without one).
    pub(crate) schema: MemorySchema,
    /// `memory_tree/chunks.db`, when present and usable.
    pub(crate) chunks: Option<ChunkStore>,
}

impl LegacyWorkspace {
    /// Opens the v1 workspace rooted at `path` (the directory that contains
    /// `memory/memory.db`, `memory_tree/chunks.db`, or both).
    ///
    /// # Errors
    ///
    /// - [`Error::NotFound`] if `path` does not exist.
    /// - [`Error::NotLegacy`] if `path` is not a directory, has neither a
    ///   `memory/memory.db` nor a usable `memory_tree/chunks.db`, or its
    ///   `memory/memory.db` is not SQLite or lacks a required table or
    ///   column.
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
        if db.exists() && !db.is_file() {
            return Err(not_legacy(&root, "memory/memory.db is not a file"));
        }
        let (memory, schema) = if db.is_file() {
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
            (Some(memory), schema)
        } else {
            (None, MemorySchema::default())
        };
        let chunks = ChunkStore::open(&root)?;
        if memory.is_none() && chunks.is_none() {
            return Err(not_legacy(
                &root,
                "neither memory/memory.db nor a usable memory_tree/chunks.db is present",
            ));
        }
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

    /// Whether `memory/memory.db` is present and will be imported.
    #[must_use]
    pub fn has_memory_db(&self) -> bool {
        self.memory.is_some()
    }

    /// Whether `memory_tree/chunks.db` is present and will be imported.
    #[must_use]
    pub fn has_chunks(&self) -> bool {
        self.chunks.is_some()
    }

    /// How many items each section yields, counted without importing: one
    /// aggregate query per section, no chunk body read. See
    /// [`LegacyCounts`] for its two edge cases.
    ///
    /// # Errors
    ///
    /// [`Error::Sqlite`] if a count query fails.
    pub fn counts(&self) -> Result<LegacyCounts> {
        crate::import::counts::count(self)
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

/// Opens a SQLite file read-only, with [`HAS_TEXT_FN`] attached.
pub(crate) fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.create_scalar_function(
        HAS_TEXT_FN,
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            Ok(match ctx.get_raw(0) {
                ValueRef::Text(bytes) => !String::from_utf8_lossy(bytes).trim().is_empty(),
                _ => false,
            })
        },
    )?;
    Ok(conn)
}

/// A SQL function true when its argument is text with something other than
/// whitespace in it, by Rust's [`str::trim`]: the test every section applies
/// to the rows it reads, so a count in SQL skips exactly the rows the import
/// skips.
pub(crate) const HAS_TEXT_FN: &str = "tm_has_text";

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
