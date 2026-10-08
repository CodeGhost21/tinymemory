//! Ids that must not be stored as they are: one holding a phone number.
//!
//! A host's thread and source ids are often opaque (`thread-<uuid>`), but a
//! channel's can name the person at the other end: a WhatsApp chat is keyed
//! by the sender's phone number. An engine that keeps memory off the
//! device stores such an id only as its [`redacted_id`], a stable digest, so
//! the number never leaves; an email address is not redacted. Every
//! comparison of an id against a stored one goes through [`same_id`], so a
//! filter, or a host's own thread, still finds memory stored redacted.

use sha2::{Digest, Sha256};

/// How a redacted id begins.
pub const REDACTED_PREFIX: &str = "tm-redacted:";

/// Digits that make a run a phone number: the shortest local numbers have 7.
const PHONE_DIGITS: usize = 7;

/// Whether `value` holds a phone number: after removing every canonical UUID
/// (`8-4-4-4-12` hex digits, which are ids, not numbers), a run of at least
/// seven digits, the digits optionally separated by spaces, dashes, dots or
/// parentheses and led by `+` (`+1 (555) 123-4567`, `15551234567`,
/// `555.123.4567`), that no letter touches: digits glued to a letter on
/// either side are part of a word or a hex id (`jane1234567@example.com`,
/// `9f31234567ab`), not a number. An id already redacted holds none, so
/// storing one read back redacts nothing twice.
#[must_use]
pub fn holds_phone_number(value: &str) -> bool {
    if value.starts_with(REDACTED_PREFIX) {
        return false;
    }
    let (mut digits, mut glued, mut prev) = (0, false, b'_');
    for byte in strip_uuids(value.as_bytes()) {
        match byte {
            b'0'..=b'9' => {
                if digits == 0 {
                    glued = prev.is_ascii_alphabetic();
                }
                digits += 1;
            }
            b' ' | b'-' | b'.' | b'(' | b')' | b'+' => {}
            _ => {
                let glued_after = byte.is_ascii_alphabetic() && prev.is_ascii_digit();
                if digits >= PHONE_DIGITS && !glued && !glued_after {
                    return true;
                }
                digits = 0;
            }
        }
        prev = byte;
    }
    digits >= PHONE_DIGITS && !glued
}

/// The stable form `value` is stored in when it holds a phone number:
/// [`REDACTED_PREFIX`] and the first 32 hex digits (128 bits) of its
/// SHA-256. The same value always gives the same form.
#[must_use]
pub fn redacted_id(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let hex: String = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{REDACTED_PREFIX}{hex}")
}

/// Whether a stored id `held` is `wanted`: the same value, or `wanted`'s
/// [`redacted_id`] when the stored one was redacted.
#[must_use]
pub fn same_id(held: Option<&str>, wanted: &str) -> bool {
    match held {
        Some(held) if held == wanted => true,
        Some(held) => held.starts_with(REDACTED_PREFIX) && held == redacted_id(wanted),
        None => false,
    }
}

/// `bytes` with every canonical UUID replaced by `_`.
fn strip_uuids(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match uuid_at(&bytes[at..]) {
            Some(len) => {
                out.push(b'_');
                at += len;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    out
}

/// The length of the canonical UUID `bytes` starts with, if any.
fn uuid_at(bytes: &[u8]) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut at = 0;
    for (index, len) in GROUPS.iter().enumerate() {
        let group = bytes.get(at..at + len)?;
        if !group.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        at += len;
        if index + 1 < GROUPS.len() {
            if bytes.get(at) != Some(&b'-') {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
