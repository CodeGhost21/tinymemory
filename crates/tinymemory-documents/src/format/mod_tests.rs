//! Tests for document format detection.

use super::*;

#[test]
fn magic_bytes_beat_a_wrong_mime_type_and_a_wrong_extension() {
    let pdf = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";
    assert_eq!(
        DocumentFormat::sniff(pdf, Some("notes.txt"), Some("text/plain")),
        DocumentFormat::Pdf
    );
}

#[test]
fn a_zip_container_is_reported_as_docx() {
    // Every OOXML file is a zip; telling docx from xlsx needs a zip reader
    // intake does not have, so the container is what gets reported.
    let zip = b"PK\x03\x04\x14\x00\x06\x00";
    assert_eq!(DocumentFormat::from_magic(zip), Some(DocumentFormat::Docx));
}

#[test]
fn from_magic_says_keep_looking_rather_than_unknown() {
    assert_eq!(DocumentFormat::from_magic(b"# A heading"), None);
    assert_eq!(DocumentFormat::from_magic(b""), None);
}

#[test]
fn a_declared_mime_type_beats_the_filename() {
    assert_eq!(
        DocumentFormat::sniff(b"hello", Some("notes.txt"), Some("text/markdown")),
        DocumentFormat::Markdown
    );
}

#[test]
fn mime_parameters_and_casing_are_ignored() {
    assert_eq!(
        DocumentFormat::from_mime("Text/HTML; charset=UTF-8"),
        Some(DocumentFormat::Html)
    );
    assert_eq!(
        DocumentFormat::from_mime("text/plain ; charset=utf-8"),
        Some(DocumentFormat::PlainText)
    );
}

#[test]
fn an_octet_stream_mime_falls_through_to_the_filename() {
    assert_eq!(
        DocumentFormat::sniff(
            b"hello there",
            Some("notes.md"),
            Some("application/octet-stream")
        ),
        DocumentFormat::Markdown
    );
}

#[test]
fn every_recognised_extension_maps_to_a_format() {
    for (filename, expected) in [
        ("a.md", DocumentFormat::Markdown),
        ("a.markdown", DocumentFormat::Markdown),
        ("a.txt", DocumentFormat::PlainText),
        ("a.log", DocumentFormat::PlainText),
        ("a.html", DocumentFormat::Html),
        ("a.htm", DocumentFormat::Html),
        ("a.pdf", DocumentFormat::Pdf),
        ("a.docx", DocumentFormat::Docx),
        ("path/to/report.PDF", DocumentFormat::Pdf),
    ] {
        assert_eq!(
            DocumentFormat::from_filename(filename),
            Some(expected),
            "{filename}"
        );
    }
}

#[test]
fn a_filename_with_no_extension_maps_to_nothing() {
    assert_eq!(DocumentFormat::from_filename("README"), None);
    assert_eq!(DocumentFormat::from_filename(""), None);
}

#[test]
fn legacy_doc_is_not_claimed_as_docx() {
    // `.doc` and `application/msword` name the legacy binary Word format, not
    // the Open XML `.docx` package the `Docx` converter targets.
    assert_eq!(DocumentFormat::from_filename("report.doc"), None);
    assert_eq!(DocumentFormat::from_mime("application/msword"), None);
}

#[test]
fn html_is_recognised_from_its_opening_alone() {
    assert_eq!(
        DocumentFormat::sniff(b"<!DOCTYPE html><html><body>hi</body></html>", None, None),
        DocumentFormat::Html
    );
    assert_eq!(
        DocumentFormat::sniff(b"  <html lang=\"en\">hi</html>", None, None),
        DocumentFormat::Html
    );
}

#[test]
fn unlabelled_text_is_plain_text() {
    assert_eq!(
        DocumentFormat::sniff(b"just some prose", None, None),
        DocumentFormat::PlainText
    );
}

#[test]
fn unlabelled_binary_is_unknown() {
    assert_eq!(
        DocumentFormat::sniff(&[0x00, 0x01, 0x02, 0xFF], None, None),
        DocumentFormat::Unknown
    );
}

#[test]
fn an_empty_buffer_is_unknown() {
    assert_eq!(
        DocumentFormat::sniff(b"", None, None),
        DocumentFormat::Unknown
    );
}

#[test]
fn textual_formats_are_the_ones_intake_can_decode_itself() {
    assert!(DocumentFormat::Markdown.is_textual());
    assert!(DocumentFormat::PlainText.is_textual());
    assert!(DocumentFormat::Html.is_textual());
    assert!(!DocumentFormat::Pdf.is_textual());
    assert!(!DocumentFormat::Docx.is_textual());
    assert!(!DocumentFormat::Unknown.is_textual());
}

#[test]
fn a_canonical_mime_round_trips_back_to_its_format() {
    for format in [
        DocumentFormat::Markdown,
        DocumentFormat::PlainText,
        DocumentFormat::Html,
        DocumentFormat::Pdf,
        DocumentFormat::Docx,
    ] {
        assert_eq!(DocumentFormat::from_mime(format.mime()), Some(format));
    }
}

#[test]
fn a_canonical_extension_round_trips_back_to_its_format() {
    for format in [
        DocumentFormat::Markdown,
        DocumentFormat::PlainText,
        DocumentFormat::Html,
        DocumentFormat::Pdf,
        DocumentFormat::Docx,
    ] {
        assert_eq!(
            DocumentFormat::from_filename(&format!("file.{}", format.extension())),
            Some(format)
        );
    }
}

#[test]
fn a_format_round_trips_through_json() {
    for format in [
        DocumentFormat::Markdown,
        DocumentFormat::PlainText,
        DocumentFormat::Html,
        DocumentFormat::Pdf,
        DocumentFormat::Docx,
        DocumentFormat::Unknown,
    ] {
        let wire = serde_json::to_string(&format).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentFormat>(&wire).unwrap(),
            format
        );
    }
}

#[test]
fn display_matches_the_wire_spelling() {
    assert_eq!(DocumentFormat::PlainText.to_string(), "plain_text");
    assert_eq!(DocumentFormat::Html.to_string(), "html");
}

#[test]
fn invalid_utf8_without_magic_bytes_is_unknown_not_text() {
    assert_eq!(
        DocumentFormat::sniff(&[0xFF, 0xFE, 0xFD], None, None),
        DocumentFormat::Unknown
    );
}
