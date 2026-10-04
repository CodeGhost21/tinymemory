//! Turning a document into a [`StoreItem::Document`].
//!
//! Intake ends here: a [`RawDocument`] goes through a [`DocumentConverter`],
//! and what comes out is the item an engine stores — the markdown as
//! [`DocumentBody::Text`], a title, the format's MIME type, and the caller's
//! [`MemoryMeta`]. Where the item is stored is the host's decision, made by
//! whichever engine it bound; this module never writes.
//!
//! The caller owns the metadata. Intake fills exactly one field, and only when
//! the caller left it unset: [`MemoryMeta::language`], from the converter or
//! from the file extension ([`crate::documents::language_for_path`]). Provenance —
//! source, workspace, URL, observation time — is the caller's to state.

use tinymemory_api::{DocumentBody, MemoryMeta, StoreItem};

use crate::documents::convert::{ConvertedDocument, DocumentConverter, RawDocument};
use crate::documents::error::Result;
use crate::documents::format::DocumentFormat;
use crate::documents::language::language_for_path;

/// Convert `document` through `converter` and wrap the result as a
/// [`StoreItem::Document`] carrying `meta`.
///
/// # Errors
///
/// Whatever the converter returns: [`crate::documents::Error::Invalid`] for an empty or
/// undecodable body, [`crate::documents::Error::TooLarge`] over the size cap,
/// [`crate::documents::Error::UnsupportedFormat`] for a format nothing converts, and
/// [`crate::documents::Error::Converter`] for a converter's own failure.
pub async fn document_item(
    converter: &dyn DocumentConverter,
    document: &RawDocument,
    meta: MemoryMeta,
) -> Result<StoreItem> {
    let converted = converter.convert(document).await?;
    Ok(converted_item(converted, document, meta))
}

/// Wrap an already-converted document as a [`StoreItem::Document`].
///
/// The title is the converter's, else (for prose) the first markdown heading,
/// else the document's file name or origin. Code never takes its title from a
/// heading: a `#` line in a script is a comment. `meta.language` is filled
/// from the conversion or the file extension when the caller left it unset.
#[must_use]
pub fn converted_item(
    converted: ConvertedDocument,
    document: &RawDocument,
    mut meta: MemoryMeta,
) -> StoreItem {
    let fallback = fallback_title(document);
    let title = if converted.format == DocumentFormat::Code {
        converted.title.clone().unwrap_or(fallback)
    } else {
        converted.title_or(&fallback)
    };
    if meta.language.is_none() {
        meta.language = converted.language.clone().or_else(|| {
            document
                .filename
                .as_deref()
                .and_then(language_for_path)
                .map(str::to_string)
        });
    }
    StoreItem::Document {
        title: Some(title),
        body: DocumentBody::Text(converted.markdown),
        mime: Some(converted.format.mime().to_string()),
        meta,
    }
}

/// The last path component of the filename, else the origin, else a
/// generated name.
fn fallback_title(document: &RawDocument) -> String {
    match document.filename.as_deref() {
        Some(filename) => filename
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(filename)
            .to_string(),
        None => document.display_name(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
