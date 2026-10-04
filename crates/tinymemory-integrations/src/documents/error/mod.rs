//! The crate-wide error and result alias.
//!
//! Every failure intake can have names what a caller can do about it: fix the
//! input ([`Error::Invalid`]), send something smaller ([`Error::TooLarge`]),
//! bind a converter for the format ([`Error::UnsupportedFormat`]), or look at
//! the converter that failed ([`Error::Converter`]).

/// Every way converting a document can fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The caller's input: an empty body, bytes that are not valid UTF-8 for a
    /// textual format, or a conversion that produced no text.
    #[error("invalid document: {0}")]
    Invalid(String),
    /// The document is over [`crate::MAX_DOCUMENT_BYTES`].
    #[error("document is {size} bytes, over the {limit}-byte intake limit")]
    TooLarge {
        /// The document's size in bytes.
        size: usize,
        /// The limit it exceeded.
        limit: usize,
    },
    /// No converter handles the detected format.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// A converter claimed the format and then failed.
    #[error("converter {converter} failed: {message}")]
    Converter {
        /// The converter's [`crate::DocumentConverter::name`].
        converter: String,
        /// What went wrong, as the converter reported it.
        message: String,
    },
}

impl From<Error> for tinymemory_api::Error {
    /// Classifies an intake failure in the contract's vocabulary: everything
    /// about the input is an invalid request, a missing converter is
    /// unsupported, and a converter's own failure is an engine-side error.
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid(_) | Error::TooLarge { .. } => Self::InvalidRequest(error.to_string()),
            Error::UnsupportedFormat(_) => Self::Unsupported(error.to_string()),
            Error::Converter { .. } => Self::Engine(error.to_string()),
        }
    }
}

/// Result alias for this crate's fallible operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
