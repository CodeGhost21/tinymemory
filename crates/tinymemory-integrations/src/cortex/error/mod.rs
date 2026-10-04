//! The engine's errors are the contract's errors.
//!
//! This crate does not define a parallel `Error`. Every public operation is a
//! [`tinymemory_api::MemoryEngine`] method, and those return
//! [`tinymemory_api::Error`]; a second enum would only be converted into it at
//! every boundary and would invite variants the host cannot act on. So the
//! contract's enum is re-exported here as the crate-wide [`Error`], and
//! construction and configuration failures use [`Error::Config`].
//!
//! # How a CortexDB failure is classified
//!
//! | HTTP | Variant |
//! | --- | --- |
//! | 401, 403 | [`Error::Unauthorized`] |
//! | 402 | [`Error::Engine`] (hosted: prefixed `[USER_INSUFFICIENT_CREDITS]`) |
//! | 404 | [`Error::NotFound`] |
//! | 400, 413, 422 | [`Error::InvalidRequest`] |
//! | 409 | [`Error::Conflict`] |
//! | 429, 500, 502, 503, 504 | [`Error::Unavailable`] (retried on reads) |
//! | anything else | [`Error::Engine`] |
//!
//! Transport faults (timeout, DNS, TLS, refused connection) are
//! [`Error::Unavailable`] too.
//!
//! **Why 402 is `Engine`.** An exhausted credit balance is neither transient
//! (`Unavailable` would invite a retry loop that cannot succeed until someone
//! tops up) nor a credential fault (`Unauthorized` would send the host to its
//! sign-in flow). It is the engine refusing to serve, which is what `Engine`
//! means, and the `[USER_INSUFFICIENT_CREDITS]` prefix lets
//! [`is_insufficient_credits`] tell it apart so a host can show a top-up
//! prompt.
//!
//! # The `[CODE]` prefix
//!
//! The TinyHumans backend names every failure with an `errorCode`. The
//! contract's `Error` has no field for it, so a hosted failure's message
//! starts with `[CODE] ` and [`error_code`] reads it back. Direct CortexDB
//! failures carry no prefix.
//!
//! Messages never carry a credential: the request builder marks the
//! credential header sensitive, and no message is built from it.

pub use tinymemory_api::Error;

/// The crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// The TinyHumans backend's code for an exhausted credit balance (HTTP 402).
pub const INSUFFICIENT_CREDITS_CODE: &str = "USER_INSUFFICIENT_CREDITS";

/// The message every variant carries.
fn message_of(error: &Error) -> &str {
    match error {
        Error::Unsupported(m)
        | Error::InvalidRequest(m)
        | Error::Unauthorized(m)
        | Error::NotFound(m)
        | Error::Conflict(m)
        | Error::Unavailable(m)
        | Error::Engine(m)
        | Error::Config(m) => m,
    }
}

/// The TinyHumans `errorCode` a hosted failure carried, when it has one.
///
/// Parses the `[CODE] ` prefix hosted failures put on their message. Returns
/// `None` for a direct CortexDB failure, a local refusal, or a message that no
/// longer starts with the prefix.
#[must_use]
pub fn error_code(error: &Error) -> Option<&str> {
    let rest = message_of(error).strip_prefix('[')?;
    let (code, _) = rest.split_once("] ")?;
    let well_formed = !code.is_empty()
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    well_formed.then_some(code)
}

/// Whether `error` is the hosted backend's "not enough credits" refusal.
#[must_use]
pub fn is_insufficient_credits(error: &Error) -> bool {
    matches!(error, Error::Engine(_)) && error_code(error) == Some(INSUFFICIENT_CREDITS_CODE)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
