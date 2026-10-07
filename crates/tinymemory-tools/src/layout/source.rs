//! [`BrainSource`]: the type of source a brain document came from.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tinymemory_api::{Error, Result, SourceKind};

/// Where a brain document came from: the connector or app that supplied it.
/// Each source is its own scope (`source:<id>`), so the brain can be read,
/// rebuilt or erased one source at a time, and disconnecting an app erases
/// one scope.
///
/// Local files of every format share [`BrainSource::Files`]; the format is a
/// property of the document, not a source. A connected app is
/// [`BrainSource::Other`] by its own slug (`gmail`, `slack`), except the two
/// known ones. [`BrainSource::Pdf`] and [`BrainSource::Markdown`] name the
/// per-format nodes documents were filed under before, so they can still be
/// read and erased.
///
/// On the wire a source is its id: `files`, `web`, `notion`, `github`, `pdf`,
/// `markdown`, or any other `[A-Za-z0-9_-]` id for [`BrainSource::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BrainSource {
    /// Local files and uploads, of any format.
    Files,
    /// PDF files (the per-format node used before [`BrainSource::Files`]).
    Pdf,
    /// Markdown and plain-text files (the per-format node used before
    /// [`BrainSource::Files`]).
    Markdown,
    /// Notion pages.
    Notion,
    /// GitHub repositories, issues and pull requests.
    Github,
    /// Web pages, links and feeds.
    Web,
    /// Any other source type, by id.
    Other(String),
}

impl BrainSource {
    /// The source's id: its `source:<id>` segment.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Files => "files",
            Self::Pdf => "pdf",
            Self::Markdown => "markdown",
            Self::Notion => "notion",
            Self::Github => "github",
            Self::Web => "web",
            Self::Other(id) => id,
        }
    }

    /// The contract's [`SourceKind`] a document of this source carries when
    /// its reader named none.
    #[must_use]
    pub fn source_kind(&self) -> SourceKind {
        match self {
            Self::Files | Self::Pdf | Self::Markdown => SourceKind::File,
            Self::Notion => SourceKind::Composio,
            Self::Github => SourceKind::Github,
            Self::Web => SourceKind::Link,
            Self::Other(_) => SourceKind::Import,
        }
    }
}

impl fmt::Display for BrainSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for BrainSource {
    type Err = Error;

    /// Parses a source id; the six known ids map to their variants
    /// (`md` is `markdown`), anything else is [`BrainSource::Other`].
    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim();
        Ok(match value.to_ascii_lowercase().as_str() {
            "" => {
                return Err(Error::InvalidRequest(
                    "a brain source id must not be blank".to_string(),
                ));
            }
            "files" => Self::Files,
            "pdf" => Self::Pdf,
            "markdown" | "md" => Self::Markdown,
            "notion" => Self::Notion,
            "github" => Self::Github,
            "web" => Self::Web,
            _ => Self::Other(value.to_string()),
        })
    }
}

impl Serialize for BrainSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for BrainSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
