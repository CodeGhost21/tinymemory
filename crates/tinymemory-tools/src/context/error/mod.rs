//! Why a context document could not be compiled.

/// A context compilation failure.
///
/// Engine failures are not here: a brief whose recall fails is skipped, and a
/// failed learnings listing leaves the learnings out, so only a spec that
/// cannot produce a document is an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The spec cannot produce a document (zero budget, a blank question).
    #[error("invalid context spec: {0}")]
    InvalidSpec(String),
}

/// The context module's result alias.
pub type Result<T> = std::result::Result<T, Error>;
