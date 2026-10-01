//! Putting a converted document into whichever engine is bound.
//!
//! The contract offers three places a document could land, and which of them
//! exists depends on the driver:
//!
//! 1. [`MemoryIngest`] — hand over raw content and let the driver chunk and
//!    embed it. The right answer when it is available, because chunking a
//!    document is exactly what that family is for.
//! 2. [`MemoryDocuments`] — store the document whole, addressed by
//!    `(namespace, key)`.
//! 3. [`MemoryCore::store`] — the mandatory family, always present.
//!
//! A caller that had to work that out itself would work it out differently in
//! every host, and a document uploaded to a Mem0 deployment would end up
//! somewhere else than the same document uploaded to TinyCortex.
//! [`DocumentIntake`] makes the choice once, reports which route it took in
//! [`IntakeReceipt::route`], and gives every host the same behaviour.
//!
//! ## What intake does not decide
//!
//! Taint. [`tinymemory_api::types::MemoryTaint`] is on [`IntakeRequest`] and is
//! passed through untouched, because the contract is explicit that the host
//! stamps provenance and a driver — or a helper sitting in front of one — never
//! assigns it. Intake that defaulted an upload to `Internal` would launder
//! whatever a user handed it.

mod types;

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::types::IngestItem;
use tinymemory_api::provider::MemoryProvider;
use tinymemory_api::types::NamespaceDocumentInput;

use crate::convert::{ConvertedDocument, DocumentConverter, RawDocument};
use crate::error::Result;

pub use types::{IntakeReceipt, IntakeRequest, IntakeRoute};

/// Converts documents and writes them into a bound provider.
///
/// Borrows both halves rather than owning them: a host has exactly one provider
/// and one converter chain for the life of the process, and an intake that
/// cloned an `Arc` per upload would suggest otherwise.
pub struct DocumentIntake<'a> {
    provider: &'a dyn MemoryProvider,
    converter: &'a dyn DocumentConverter,
}

impl std::fmt::Debug for DocumentIntake<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentIntake")
            .field("provider", &self.provider.driver_id())
            .field("converter", &self.converter.name())
            .finish()
    }
}

impl<'a> DocumentIntake<'a> {
    /// Pair a provider with the converter chain that feeds it.
    pub fn new(provider: &'a dyn MemoryProvider, converter: &'a dyn DocumentConverter) -> Self {
        Self {
            provider,
            converter,
        }
    }

    /// Which route [`Self::accept`] would take against this provider.
    ///
    /// Exposed so a host can tell a user what will happen — and so a diagnostic
    /// endpoint can report it — without performing a write to find out.
    pub fn route(&self) -> IntakeRoute {
        if self.provider.as_ingest().is_some() {
            IntakeRoute::Ingest
        } else if self.provider.as_documents().is_some() {
            IntakeRoute::Documents
        } else {
            IntakeRoute::Core
        }
    }

    /// Convert `document` and store it under `request`.
    ///
    /// # Errors
    ///
    /// Whatever the converter returns for an unconvertible document, and
    /// whatever the driver returns for a rejected write.
    pub async fn accept(
        &self,
        document: &RawDocument,
        request: &IntakeRequest,
    ) -> Result<IntakeReceipt> {
        request.validate()?;
        let converted = self.converter.convert(document).await?;
        self.store(document, &converted, request).await
    }

    /// Store an already-converted document.
    ///
    /// Separate from [`Self::accept`] so a caller that converted elsewhere — or
    /// that wants to show the markdown to a user before committing it — does
    /// not have to convert twice.
    ///
    /// # Errors
    ///
    /// Whatever the driver returns for a rejected write.
    pub async fn store(
        &self,
        document: &RawDocument,
        converted: &ConvertedDocument,
        request: &IntakeRequest,
    ) -> Result<IntakeReceipt> {
        request.validate()?;
        let title = converted.title_or(&request.fallback_title(document));
        let key = request.key(document, &title);

        match self.route() {
            IntakeRoute::Ingest => {
                let ingest = self.provider.as_ingest().ok_or_else(|| {
                    MemoryError::Backend("provider withdrew its ingest family mid-call".to_string())
                })?;
                let item = IngestItem {
                    namespace: Some(request.namespace.clone()),
                    source: request.source,
                    source_id: key.clone(),
                    owner: request.owner.clone(),
                    source_ref: request.source_ref(document),
                    content: converted.markdown.clone(),
                    // The body is markdown now whatever it started as, and a
                    // driver that chunks on headings needs to be told that
                    // rather than shown the original `application/pdf`.
                    mime: Some("text/markdown".to_string()),
                    timestamp: request.timestamp,
                    tags: request.tags.clone(),
                    taint: request.taint,
                    path_scope: None,
                    author: None,
                    channel_label: None,
                    platform: None,
                    to: Vec::new(),
                    cc: Vec::new(),
                    subject: None,
                    list_unsubscribe: None,
                };
                let outcome = ingest.ingest_document(item).await?;
                Ok(IntakeReceipt {
                    route: IntakeRoute::Ingest,
                    namespace: request.namespace.clone(),
                    key,
                    title,
                    format: converted.format,
                    markdown_bytes: converted.markdown.len(),
                    source_bytes: converted.source_bytes,
                    ids: outcome.ids,
                    written: outcome.written,
                    skipped: outcome.skipped,
                })
            }
            IntakeRoute::Documents => {
                let documents = self.provider.as_documents().ok_or_else(|| {
                    MemoryError::Backend(
                        "provider withdrew its documents family mid-call".to_string(),
                    )
                })?;
                let input = NamespaceDocumentInput {
                    namespace: request.namespace.clone(),
                    key: key.clone(),
                    title: title.clone(),
                    content: converted.markdown.clone(),
                    source_type: request.source.as_str().to_string(),
                    priority: request.priority.clone(),
                    tags: request.tags.clone(),
                    metadata: request.document_metadata(document, converted),
                    category: request.category.to_string(),
                    session_id: request.session_id.clone(),
                    document_id: None,
                    taint: request.taint,
                };
                let id = documents.put_document(input).await?;
                Ok(IntakeReceipt {
                    route: IntakeRoute::Documents,
                    namespace: request.namespace.clone(),
                    key,
                    title,
                    format: converted.format,
                    markdown_bytes: converted.markdown.len(),
                    source_bytes: converted.source_bytes,
                    ids: vec![id],
                    written: 1,
                    skipped: 0,
                })
            }
            IntakeRoute::Core => {
                self.provider
                    .store(
                        &request.namespace,
                        &key,
                        &converted.markdown,
                        request.category.clone(),
                        request.session_id.as_deref(),
                        request.taint,
                    )
                    .await?;
                Ok(IntakeReceipt {
                    route: IntakeRoute::Core,
                    namespace: request.namespace.clone(),
                    key,
                    title,
                    format: converted.format,
                    markdown_bytes: converted.markdown.len(),
                    source_bytes: converted.source_bytes,
                    ids: Vec::new(),
                    written: 1,
                    skipped: 0,
                })
            }
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
