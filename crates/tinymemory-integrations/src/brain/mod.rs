//! Files into the brain: conversion to a [`BrainDocument`] of the right
//! source type.
//!
//! [`tinymemory_tools::Brain`] stores text; this module is the step before
//! it. [`brain_document`] runs a [`RawDocument`] through a
//! [`DocumentConverter`] (the [`crate::documents`] pipeline) and places the
//! markdown under a [`BrainSource`] — the one the caller names, or the one
//! its detected format implies ([`source_for`]): a PDF lands in
//! `source:pdf`, markdown and plain text in `source:markdown`, HTML in
//! `source:web`.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use tinymemory_api::MemoryMeta;
//! use tinymemory_api::conformance::ReferenceEngine;
//! use tinymemory_integrations::brain::brain_document;
//! use tinymemory_integrations::documents::{ConverterChain, RawDocument};
//! use tinymemory_tools::{Brain, BrainSource, MemoryLayout};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let file = RawDocument::new("# Refunds\n\nRefunds take five business days.\n")
//!     .with_filename("handbook/refunds.md");
//! let document = brain_document(&ConverterChain::default(), &file, None, MemoryMeta::default())
//!     .await?;
//! assert_eq!(document.source, BrainSource::Markdown);
//! assert_eq!(document.title.as_deref(), Some("Refunds"));
//!
//! let brain = Brain::new(Arc::new(ReferenceEngine::new()), MemoryLayout::default());
//! brain.ingest(document).await?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use tinymemory_api::{DocumentBody, MemoryMeta, StoreItem};
use tinymemory_tools::{BrainDocument, BrainSource};

use crate::documents::{
    DocumentConverter, DocumentFormat, Error, RawDocument, Result, converted_item,
};

/// The brain source a document of `format` belongs to when the caller names
/// none: PDFs to `pdf`, markdown and plain text to `markdown`, HTML to `web`,
/// and each other format to a source of its own name (`docx`, `xlsx`,
/// `pptx`, `code`, `other`).
#[must_use]
pub fn source_for(format: DocumentFormat) -> BrainSource {
    match format {
        DocumentFormat::Pdf => BrainSource::Pdf,
        DocumentFormat::Markdown | DocumentFormat::PlainText => BrainSource::Markdown,
        DocumentFormat::Html => BrainSource::Web,
        DocumentFormat::Docx => BrainSource::Other("docx".to_string()),
        DocumentFormat::Xlsx => BrainSource::Other("xlsx".to_string()),
        DocumentFormat::Pptx => BrainSource::Other("pptx".to_string()),
        DocumentFormat::Code => BrainSource::Other("code".to_string()),
        DocumentFormat::Unknown => BrainSource::Other("other".to_string()),
    }
}

/// Converts `document` through `converter` into a brain document of
/// `source` (or the source its format implies, see [`source_for`]),
/// carrying `meta`. The title and MIME type come from the conversion, and
/// `meta.language` is filled as [`crate::documents::document_item`] fills
/// it.
///
/// # Errors
///
/// Whatever the converter returns (see
/// [`crate::documents::document_item`]).
pub async fn brain_document(
    converter: &dyn DocumentConverter,
    document: &RawDocument,
    source: Option<BrainSource>,
    meta: MemoryMeta,
) -> Result<BrainDocument> {
    let converted = converter.convert(document).await?;
    let source = source.unwrap_or_else(|| source_for(converted.format));
    match converted_item(converted, document, meta) {
        StoreItem::Document {
            title,
            body: DocumentBody::Text(text),
            mime,
            meta,
        } => Ok(BrainDocument {
            source,
            title,
            text,
            mime,
            meta,
        }),
        _ => Err(Error::Invalid(
            "conversion did not produce a text document".to_string(),
        )),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
