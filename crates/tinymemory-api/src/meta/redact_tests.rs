//! Which ids hold a phone number, their redacted form, and id comparison.

use super::*;

#[test]
fn a_phone_number_in_any_common_spelling_is_found() {
    for id in [
        "channel:whatsapp_+15551234567_+15551234567",
        "channel:whatsapp_15551234567@s.whatsapp.net_reply",
        "channel:sms_15551234567",
        "+1 (555) 123-4567",
        "555.123.4567",
        "call 555-1234 later",
    ] {
        assert!(holds_phone_number(id), "{id}");
    }
}

#[test]
fn ids_without_a_phone_number_are_left_alone() {
    for id in [
        "thread-0b6e1d8a-3f2c-4c1e-9a7b-5d2e8f1a6c3b",
        "thread-12345678-1234-1234-1234-123456789012",
        "worker-7f9c2d1e-8a4b-4c3d-9e2f-1a0b5c6d7e8f",
        "channel:email_jane.doe@example.com_inbox",
        "channel:slack_U02ABC123_C03DEF456",
        "t-1",
        "a1b2c3d4e5",
        "channel:email_jane1234567@example.com_inbox",
        "commit 9f31234567ab",
        "",
    ] {
        assert!(!holds_phone_number(id), "{id}");
    }
}

#[test]
fn a_redacted_id_is_stable_opaque_and_compares_equal() {
    let phone = "channel:whatsapp_+15551234567_+15551234567";
    let redacted = redacted_id(phone);
    assert_eq!(redacted, redacted_id(phone));
    assert!(redacted.starts_with(REDACTED_PREFIX));
    assert_eq!(redacted.len(), REDACTED_PREFIX.len() + 32);
    assert!(!redacted.contains("5551234567"));
    assert_ne!(redacted, redacted_id("channel:whatsapp_+15557654321"));

    assert!(same_id(Some(phone), phone), "a raw id stored before");
    assert!(same_id(Some(&redacted), phone), "a redacted id stored now");
    assert!(!same_id(Some(&redacted), "channel:whatsapp_+15557654321"));
    assert!(!same_id(None, phone));
    assert!(
        !same_id(Some("tm-redacted:not-a-digest"), phone),
        "only the value's own digest matches"
    );
}

#[test]
fn a_redacted_id_is_never_redacted_again() {
    let redacted = redacted_id("channel:sms_15551234567");
    let digit_heavy = format!("{REDACTED_PREFIX}{}", "1234567".repeat(4));
    assert!(!holds_phone_number(&redacted));
    assert!(
        !holds_phone_number(&digit_heavy),
        "its hex may run to 7 digits"
    );
    assert!(
        same_id(Some(&redacted), &redacted),
        "a read-back id finds itself"
    );
}
