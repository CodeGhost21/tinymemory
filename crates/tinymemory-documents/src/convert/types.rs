//! The two values every conversion moves between: [`RawDocument`] in,
//! [`ConvertedDocument`] out.

use serde::{Deserialize, Serialize};

use crate::format::DocumentFormat;

/// Largest document intake will accept, in bytes.
///
/// A ceiling on what one call may hold in memory, not a judgement about what is
/// worth remembering. It is enforced before conversion rather than after,
/// because a 200 MB PDF costs the same to reject early and far more to decode
/// first.
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;

/// A document as it arrived, before anything has interpreted it.
///
/// Carries the three signals format detection needs plus the origin, so a
/// converter never has to be told separately where the bytes came from.
#[derive(Debug, Clone)]
pub struct RawDocument {
    /// The document body, exactly as received.
    pub bytes: Vec<u8>,
    /// Original filename, when the caller had one.
    pub filename: Option<String>,
    /// MIME type the caller declared. Advisory: detection may overrule it.
    pub declared_mime: Option<String>,
    /// Where the bytes came from — a URL for a fetch, `None` for an upload.
    pub origin: Option<String>,
}

impl RawDocument {
    /// A document from an upload, with no filename or declared type.
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            filename: None,
            declared_mime: None,
            origin: None,
        }
    }

    /// Attach the original filename.
    #[must_use]
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Attach the caller-declared MIME type.
    #[must_use]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.declared_mime = Some(mime.into());
        self
    }

    /// Attach the URL the bytes were fetched from.
    #[must_use]
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    /// Detect this document's format from every signal it carries.
    #[must_use]
    pub fn format(&self) -> DocumentFormat {
        DocumentFormat::sniff(
            &self.bytes,
            self.filename.as_deref(),
            self.declared_mime.as_deref(),
        )
    }

    /// A display name for this document: its filename, else its origin, else a
    /// generated name based on the detected format.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.filename
            .clone()
            .or_else(|| self.origin.clone())
            .unwrap_or_else(|| format!("document.{}", self.format().extension()))
    }
}

/// A document after conversion: markdown, plus what was learned on the way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertedDocument {
    /// The document body as markdown. Never empty — a conversion that produced
    /// nothing is an error, not an empty success, because storing an empty
    /// document silently loses the upload.
    pub markdown: String,
    /// Document title, when one could be recovered.
    pub title: Option<String>,
    /// Format the source was detected as.
    pub format: DocumentFormat,
    /// Programming language, for [`DocumentFormat::Code`] whose filename named
    /// one (see [`crate::language_for_path`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Size of the source document in bytes, before conversion.
    pub source_bytes: usize,
    /// Anything else the converter learned — page counts, author, the
    /// converter's own name. Open on purpose: this crate cannot know what a
    /// host's converter will find worth keeping.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl ConvertedDocument {
    /// A converted document with no title and no metadata.
    #[must_use]
    pub fn new(markdown: impl Into<String>, format: DocumentFormat, source_bytes: usize) -> Self {
        Self {
            markdown: markdown.into(),
            title: None,
            format,
            language: None,
            source_bytes,
            metadata: serde_json::Value::Null,
        }
    }

    /// Attach a title.
    #[must_use]
    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title.filter(|t| !t.trim().is_empty());
        self
    }

    /// Attach a programming language.
    #[must_use]
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language.filter(|l| !l.trim().is_empty());
        self
    }

    /// Attach converter metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// The title if there is one, otherwise the first markdown heading,
    /// otherwise `fallback`.
    ///
    /// Documents that carry no title metadata almost always open with their
    /// title as a heading, and a stored document named `upload.pdf` is one
    /// nobody finds again.
    #[must_use]
    pub fn title_or(&self, fallback: &str) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        self.markdown
            .lines()
            .find_map(|line| {
                let heading = line.trim_start_matches('#').trim();
                (line.starts_with('#') && !heading.is_empty()).then(|| heading.to_string())
            })
            .unwrap_or_else(|| fallback.to_string())
    }
}
