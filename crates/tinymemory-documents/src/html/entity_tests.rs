//! Tests for HTML entity decoding.

use super::*;

#[test]
fn text_without_an_ampersand_is_returned_unchanged() {
    assert_eq!(decode_entities("plain prose"), "plain prose");
}

#[test]
fn the_named_entities_that_appear_in_prose_are_decoded() {
    assert_eq!(decode_entities("a &amp; b"), "a & b");
    assert_eq!(decode_entities("&lt;tag&gt;"), "<tag>");
    assert_eq!(decode_entities("&quot;quoted&quot;"), "\"quoted\"");
    assert_eq!(decode_entities("it&apos;s"), "it's");
    assert_eq!(decode_entities("wait&hellip;"), "wait…");
    assert_eq!(decode_entities("a&mdash;b"), "a—b");
}

#[test]
fn a_non_breaking_space_becomes_an_ordinary_one() {
    assert_eq!(decode_entities("a&nbsp;b"), "a b");
}

#[test]
fn decimal_and_hex_numeric_entities_are_decoded() {
    assert_eq!(decode_entities("&#65;&#66;"), "AB");
    assert_eq!(decode_entities("&#x41;&#X42;"), "AB");
    assert_eq!(decode_entities("&#8230;"), "…");
}

#[test]
fn an_unrecognised_entity_is_passed_through_verbatim() {
    assert_eq!(decode_entities("&nosuch;"), "&nosuch;");
}

#[test]
fn an_out_of_range_numeric_entity_is_passed_through() {
    assert_eq!(decode_entities("&#99999999;"), "&#99999999;");
}

#[test]
fn a_bare_ampersand_survives() {
    assert_eq!(decode_entities("Tom & Jerry"), "Tom & Jerry");
    assert_eq!(decode_entities("ends with &"), "ends with &");
}

#[test]
fn a_long_run_after_an_ampersand_is_not_treated_as_an_entity() {
    let input = "a & this is a long sentence; not an entity";
    assert_eq!(decode_entities(input), input);
}

#[test]
fn several_entities_in_one_string_are_all_decoded() {
    assert_eq!(
        decode_entities("&lt;a href=&quot;x&quot;&gt;A &amp; B&lt;/a&gt;"),
        "<a href=\"x\">A & B</a>"
    );
}

#[test]
fn a_multibyte_run_after_an_ampersand_does_not_panic() {
    // Each `€` is 3 bytes, so the 12-byte scan window lands mid-character
    // unless the scan snaps back to a char boundary first.
    let input = "&€€€€;";
    assert_eq!(decode_entities(input), input);
}
