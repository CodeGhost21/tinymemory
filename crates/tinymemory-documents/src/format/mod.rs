//! Document format detection.
//!
//! Intake gets a byte buffer and, if it is lucky, a filename and a MIME type.
//! None of the three is reliable on its own: browsers send
//! `application/octet-stream` for files they cannot place, a `.txt` extension
//! says nothing about what is inside, and a buffer alone cannot distinguish
//! markdown from plain text. So [`DocumentFormat::sniff`] consults all three in
//! order of trustworthiness — magic bytes first, because they are the only
//! signal a caller cannot get wrong.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A document format intake can recognise.
///
/// Deliberately short. This is the set that has a defined conversion, not a
/// catalogue of everything that exists: a format nobody converts would be a
/// variant that only ever appears in an error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    /// Markdown. Already the target format; conversion is a passthrough.
    Markdown,
    /// Plain text. Wrapped into markdown without interpretation.
    PlainText,
    /// HTML. Converted structurally — headings, lists, links, code.
    Html,
    /// PDF. Needs a real extractor; see [`crate::convert::DocumentConverter`].
    Pdf,
    /// Office Open XML word processing (`.docx`). Needs a real extractor.
    Docx,
    /// A format detection could not place.
    Unknown,
}

impl DocumentFormat {
    /// The canonical MIME type for this format.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::PlainText => "text/plain",
            Self::Html => "text/html",
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Unknown => "application/octet-stream",
        }
    }

    /// The usual file extension, without a dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::PlainText => "txt",
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Unknown => "bin",
        }
    }

    /// Whether the bytes of this format are text a human could read directly.
    ///
    /// The line that decides whether intake can decode a buffer itself or has
    /// to hand it to an extractor.
    pub fn is_textual(self) -> bool {
        matches!(self, Self::Markdown | Self::PlainText | Self::Html)
    }

    /// Detect the format from every signal available.
    ///
    /// Magic bytes win when present, because they are the one signal a caller
    /// cannot get wrong. A declared MIME type comes next, then the filename,
    /// and a textual buffer with no other evidence is plain text.
    pub fn sniff(bytes: &[u8], filename: Option<&str>, mime: Option<&str>) -> Self {
        if let Some(format) = Self::from_magic(bytes) {
            return format;
        }
        if let Some(format) = mime.and_then(Self::from_mime) {
            return format;
        }
        if let Some(format) = filename.and_then(Self::from_filename) {
            return format;
        }
        // An HTML document served without a type or an extension is common
        // enough — and cheap enough to spot — to be worth one more look.
        if looks_like_html(bytes) {
            return Self::Html;
        }
        if is_probably_text(bytes) {
            Self::PlainText
        } else {
            Self::Unknown
        }
    }

    /// Detect from leading magic bytes alone.
    ///
    /// Returns `None` rather than [`DocumentFormat::Unknown`]: "no magic bytes"
    /// and "magic bytes that match nothing" both mean *keep looking*, and a
    /// caller that got `Unknown` here would stop.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"%PDF-") {
            return Some(Self::Pdf);
        }
        // Every OOXML file is a zip. Which OOXML it is lives in the archive,
        // which needs a zip reader intake does not have — so this reports the
        // container and lets the extractor disagree.
        if bytes.starts_with(b"PK\x03\x04") {
            return Some(Self::Docx);
        }
        None
    }

    /// Map a MIME type onto a format.
    ///
    /// Parameters (`; charset=utf-8`) are stripped, and the type is compared
    /// case-insensitively, because both vary by client and neither carries
    /// meaning here.
    pub fn from_mime(mime: &str) -> Option<Self> {
        let essence = mime
            .split(';')
            .next()
            .unwrap_or(mime)
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "text/markdown" | "text/x-markdown" => Some(Self::Markdown),
            "text/plain" => Some(Self::PlainText),
            "text/html" | "application/xhtml+xml" => Some(Self::Html),
            "application/pdf" => Some(Self::Pdf),
            // Deliberately excludes `application/msword`: that MIME type
            // names the legacy binary `.doc` format, not the Open XML `.docx`
            // package this variant's converter targets. Claiming `Docx` for
            // it would hand a bound DOCX extractor input it cannot read.
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some(Self::Docx)
            }
            _ => None,
        }
    }

    /// Map a filename or path onto a format by its extension.
    pub fn from_filename(filename: &str) -> Option<Self> {
        let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
        match extension.as_str() {
            "md" | "markdown" | "mdown" => Some(Self::Markdown),
            "txt" | "text" | "log" => Some(Self::PlainText),
            "html" | "htm" | "xhtml" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            // `.doc` is the legacy binary Word format, not Open XML `.docx`;
            // see the `application/msword` note in `from_mime`.
            "docx" => Some(Self::Docx),
            _ => None,
        }
    }
}

impl fmt::Display for DocumentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Markdown => "markdown",
            Self::PlainText => "plain_text",
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Unknown => "unknown",
        })
    }
}

/// Whether a buffer opens with something only HTML opens with.
///
/// Only the first bytes are examined, and only for the two openings that are
/// unambiguous. A page whose first tag is a `<div>` is not worth guessing at:
/// it will have arrived with a content type.
fn looks_like_html(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let lower = text.trim_start().to_ascii_lowercase();
    lower.starts_with("<!doctype html") || lower.starts_with("<html")
}

/// Whether a buffer is plausibly UTF-8 text.
///
/// A NUL byte is the giveaway for binary; beyond that this checks that the
/// buffer decodes. Truncating to a prefix would risk splitting a multi-byte
/// character, so the whole buffer is decoded — intake has it in memory anyway.
fn is_probably_text(bytes: &[u8]) -> bool {
    !bytes.is_empty() && !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
