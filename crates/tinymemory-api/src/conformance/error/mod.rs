//! What a failed conformance run reports.

/// A conformance failure, naming the check that failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The engine answered, but not the way the contract requires.
    #[error("conformance check `{check}` failed: {detail}")]
    Check {
        /// The check's name.
        check: &'static str,
        /// What was wrong.
        detail: String,
    },
    /// The engine failed a call the contract requires it to serve.
    #[error("conformance check `{check}` failed: the engine returned an error: {source}")]
    Engine {
        /// The check's name.
        check: &'static str,
        /// The engine's error.
        source: crate::Error,
    },
}

/// The crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
