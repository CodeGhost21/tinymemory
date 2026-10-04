//! The sources module's error and result alias.
//!
//! Variants say what went wrong in terms a host can act on: bad
//! configuration or input ([`Error::Invalid`]), something that is not there
//! ([`Error::NotFound`]), a path that escapes its root ([`Error::PathEscape`]),
//! a body over a cap ([`Error::TooLarge`]), a network target that could not be
//! reached ([`Error::Unreachable`]) or answered with a failure
//! ([`Error::Upstream`]). A network reader's own diagnostic is
//! [`Error::Reader`], whose message is the reader's text verbatim.
//!
//! `From<Error> for tinymemory_api::Error` maps each onto the contract's
//! vocabulary, so a host that stores what a reader produced reports one kind
//! of error.

/// Every way reading a source can fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The source configuration or the caller's input is invalid.
    #[error("invalid source input: {0}")]
    Invalid(String),
    /// The addressed folder, file, thread or item does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A path resolved outside the root it was confined to.
    #[error("path escape: {0}")]
    PathEscape(String),
    /// A file or response body is over its size cap.
    #[error("too large: {0}")]
    TooLarge(String),
    /// The request never completed (connection, DNS, timeout, interrupted
    /// read); the same call may succeed later.
    #[error("unreachable: {0}")]
    Unreachable(String),
    /// The remote end answered, with a failure status.
    #[error("upstream error: {0}")]
    Upstream(String),
    /// A network reader's own diagnostic, carried verbatim.
    #[error("{0}")]
    Reader(String),
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON document (a thread file) did not parse.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Converting a body to markdown failed.
    #[error(transparent)]
    Document(#[from] crate::documents::Error),
}

impl From<Error> for tinymemory_api::Error {
    /// Classifies a reader failure in the contract's vocabulary.
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid(_) | Error::PathEscape(_) | Error::TooLarge(_) | Error::Json(_) => {
                Self::InvalidRequest(error.to_string())
            }
            Error::NotFound(_) => Self::NotFound(error.to_string()),
            Error::Unreachable(_) => Self::Unavailable(error.to_string()),
            Error::Document(inner) => inner.into(),
            Error::Upstream(_) | Error::Reader(_) | Error::Io(_) => Self::Engine(error.to_string()),
        }
    }
}

/// Result alias for this module's fallible operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
