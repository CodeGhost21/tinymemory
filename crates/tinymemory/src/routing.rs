//! One high-level router over a negotiated memory provider.

use tinymemory_api::capabilities::Capability;
use tinymemory_api::error::MemoryError;
use tinymemory_api::learning::LearningCandidate;
use tinymemory_api::operations::{AnswerRequest, AnswerResponse, RawMemoryEvent};
use tinymemory_api::provider::types::{IngestItem, IngestOutcome, SourceScope};
use tinymemory_api::provider::MemoryProvider;
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::types::MemoryEntry;

/// Routes the six product-facing memory operations through one provider.
///
/// Optional operations fail with a typed [`MemoryError::Unsupported`] naming
/// the absent capability. Recall is always callable because it is mandatory on
/// [`MemoryProvider`].
#[derive(Clone, Copy)]
pub struct MemoryApi<'a> {
    provider: &'a dyn MemoryProvider,
}

impl std::fmt::Debug for MemoryApi<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryApi")
            .field("driver_id", &self.provider.driver_id())
            .field("capabilities", &self.provider.capabilities())
            .finish()
    }
}

impl<'a> MemoryApi<'a> {
    /// Bind the router to a provider.
    #[must_use]
    pub fn new(provider: &'a dyn MemoryProvider) -> Self {
        Self { provider }
    }

    /// Ingest a document through the provider's document route.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported(document_ingest)` when the route is absent,
    /// otherwise the provider's validation or backend error.
    pub async fn ingest_document(
        &self,
        document: IngestItem,
    ) -> Result<IngestOutcome, MemoryError> {
        self.provider
            .as_document_ingest()
            .ok_or_else(|| MemoryError::unsupported(Capability::DocumentIngest))?
            .ingest_document(document)
            .await
    }

    /// Ingest an ordered conversation.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported(conversation_ingest)` when absent, otherwise the
    /// provider's error.
    pub async fn ingest_conversation(
        &self,
        messages: Vec<IngestItem>,
    ) -> Result<IngestOutcome, MemoryError> {
        self.provider
            .as_conversation_ingest()
            .ok_or_else(|| MemoryError::unsupported(Capability::ConversationIngest))?
            .ingest_conversation(messages)
            .await
    }

    /// Ingest one extracted learning.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported(learning_ingest)` when absent, otherwise the
    /// provider's error.
    pub async fn ingest_learning(
        &self,
        learning: LearningCandidate,
    ) -> Result<IngestOutcome, MemoryError> {
        self.provider
            .as_learning_ingest()
            .ok_or_else(|| MemoryError::unsupported(Capability::LearningIngest))?
            .ingest_learning(learning)
            .await
    }

    /// Ingest one raw event.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported(event_ingest)` when absent, otherwise the
    /// provider's error.
    pub async fn ingest_event(&self, event: RawMemoryEvent) -> Result<IngestOutcome, MemoryError> {
        self.provider
            .as_event_ingest()
            .ok_or_else(|| MemoryError::unsupported(Capability::EventIngest))?
            .ingest_event(event)
            .await
    }

    /// Run deterministic ranked recall.
    ///
    /// # Errors
    ///
    /// Returns the provider's validation or backend error.
    pub async fn recall(
        &self,
        query: &str,
        limit: usize,
        options: &OwnedRecallOpts,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.provider.recall(query, limit, options, scope).await
    }

    /// Run agentic grounded answer synthesis.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported(answer)` when absent, otherwise the provider's
    /// retrieval or inference error.
    pub async fn answer(&self, request: AnswerRequest) -> Result<AnswerResponse, MemoryError> {
        self.provider
            .as_answer()
            .ok_or_else(|| MemoryError::unsupported(Capability::Answer))?
            .answer(request)
            .await
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
