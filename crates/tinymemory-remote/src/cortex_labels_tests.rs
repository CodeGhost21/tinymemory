//! Lookup labels: fixed length, comma-free, stable, and kept apart by kind.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn a_label_is_fixed_length_and_comma_free() {
    let long = "x".repeat(5_000);
    for value in ["", "k", "a,b", "ünï/cödé", long.as_str()] {
        let label = key(value);
        let digest = label.strip_prefix("tm:kh:").expect("key prefix");
        assert_eq!(digest.len(), 16, "{label}");
        assert!(!label.contains(','), "{label}");
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{label}"
        );
    }
}

#[test]
fn the_same_value_always_reads_as_the_same_label() {
    assert_eq!(key("abc"), key("abc"));
    assert_ne!(key("abc"), key("abd"));
    // The first 64 bits of SHA-256("abc").
    assert_eq!(digest("abc"), "ba7816bf8f01cfea");
}

#[test]
fn the_key_and_source_lookups_stay_apart() {
    assert_ne!(key("same"), source("same"));
    assert!(source("same").starts_with("tm:srh:"));
}
