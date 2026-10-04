//! Tests for the `[CODE]` prefix helpers.

use super::*;

#[test]
fn a_hosted_code_is_read_back_from_the_prefix() {
    let error = Error::Unauthorized("[UNAUTHORIZED] memory API memory/events — expired".into());
    assert_eq!(error_code(&error), Some("UNAUTHORIZED"));
}

#[test]
fn an_unprefixed_message_has_no_code() {
    assert_eq!(error_code(&Error::Engine("HTTP 500".into())), None);
    assert_eq!(error_code(&Error::Engine("[lowercase] nope".into())), None);
    assert_eq!(error_code(&Error::Engine("[] empty".into())), None);
}

#[test]
fn insufficient_credits_needs_both_the_variant_and_the_code() {
    let credits = Error::Engine(format!("[{INSUFFICIENT_CREDITS_CODE}] top up"));
    assert!(is_insufficient_credits(&credits));
    let wrong_variant = Error::Unavailable(format!("[{INSUFFICIENT_CREDITS_CODE}] top up"));
    assert!(!is_insufficient_credits(&wrong_variant));
    assert!(!is_insufficient_credits(&Error::Engine(
        "[RATE_LIMITED] slow".into()
    )));
}
