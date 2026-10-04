//! The crate-wide [`Error`] and [`Result`].

use std::path::PathBuf;

use crate::import::checkpoint::Checkpoint;

/// Everything that can go wrong opening, reading or migrating a legacy
/// workspace.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The path given to [`crate::import::LegacyWorkspace::open`] does not exist.
    #[error("no legacy workspace at {}", path.display())]
    NotFound {
        /// The path that was looked up.
        path: PathBuf,
    },
    /// The path exists but is not a v1 TinyCortex workspace.
    #[error("{} is not a v1 tinycortex workspace: {reason}", path.display())]
    NotLegacy {
        /// The path that was inspected.
        path: PathBuf,
        /// What was missing or wrong.
        reason: String,
    },
    /// A legacy SQLite database could not be read.
    #[error("legacy sqlite read failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A file referenced by the legacy store could not be read.
    #[error("reading {} failed: {source}", path.display())]
    Io {
        /// The file that was read.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A [`crate::import::Checkpoint`] could not be encoded or decoded as JSON.
    #[error("checkpoint json is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The engine refused a batch during [`crate::import::migrate`].
    /// Everything up to `checkpoint` is stored; resume from it.
    #[error("migration stopped: the engine failed: {source}")]
    Engine {
        /// The engine's own error.
        #[source]
        source: tinymemory_api::Error,
        /// The last committed resume point.
        checkpoint: Checkpoint,
    },
}

impl Error {
    /// The checkpoint to resume a [`crate::import::migrate`] from, when the
    /// error carries one ([`Error::Engine`]).
    #[must_use]
    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        match self {
            Self::Engine { checkpoint, .. } => Some(checkpoint),
            _ => None,
        }
    }
}

/// The crate-wide result.
pub type Result<T> = std::result::Result<T, Error>;
