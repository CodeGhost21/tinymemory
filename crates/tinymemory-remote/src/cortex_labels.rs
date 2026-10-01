//! Lookup labels: fixed-length digests a label filter can find a record by.
//!
//! Written on the TinyHumans wire only: by `store` and its tombstone, and by
//! every hosted family record.
//!
//! A label carries a digest of the value rather than the value. The engine
//! splits its label filter on commas and bounds a label's length, and a key or
//! a source id may be long or hold a comma; a digest is neither. The label only
//! narrows a listing: every reader re-checks the envelope it gets back, so a
//! collision costs a wasted row, never a wrong answer.

use sha2::{Digest, Sha256};

/// Hex digits of the SHA-256 a label keeps: 64 bits, far beyond any collision a
/// listing of one scope could meet.
const DIGEST_CHARS: usize = 16;

/// The first [`DIGEST_CHARS`] lowercase hex digits of `value`'s SHA-256.
pub(crate) fn digest(value: &str) -> String {
    let mut out = String::with_capacity(DIGEST_CHARS);
    for byte in Sha256::digest(value.as_bytes()).iter() {
        out.push_str(&format!("{byte:02x}"));
        if out.len() >= DIGEST_CHARS {
            break;
        }
    }
    out.truncate(DIGEST_CHARS);
    out
}

/// The label every version of the keyed record `key` carries.
pub(crate) fn key(key: &str) -> String {
    format!("tm:kh:{}", digest(key))
}

/// The label every record that came from `source_id` carries.
pub(crate) fn source(source_id: &str) -> String {
    format!("tm:srh:{}", digest(source_id))
}

/// The label every hosted family record of session `session_id` carries.
pub(crate) fn session(session_id: &str) -> String {
    format!("tm:sh:{}", digest(session_id))
}

#[cfg(test)]
#[path = "cortex_labels_tests.rs"]
mod test;
