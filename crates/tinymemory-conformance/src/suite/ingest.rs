//! The granular ingestion families and grounded answers.
//!
//! A driver that serves `DocumentIngest`, `ConversationIngest`,
//! `LearningIngest`, `EventIngest` or `Answer` is held to what those contracts
//! state: valid material is accepted, sending the same material again is not an
//! error, the outcome it reports is self-consistent, and malformed input is
//! refused as [`MemoryError::Invalid`] — neither accepted nor refused as a
//! backend fault. How ingested material is read back is each engine's own
//! business (a chunk tier, an event log, a keyed store), so nothing here reads
//! it back. [`assert_answer_is_grounded`] is the opt-in check that a real
//! engine answers from what it was given.
//!
//! Source ids carry a per-run nonce. Ingestion has no generic delete, so
//! against a live service that kept an earlier run's material a fixed id
//! would legitimately read as "already ingested" and hide a driver that
//! writes nothing.

use tinymemory_api::chunks::DataSource;
use tinymemory_api::error::MemoryError;
use tinymemory_api::evidence::EvidenceRef;
use tinymemory_api::learning::{CueFamily, FacetClass, LearningCandidate};
use tinymemory_api::provider::types::{IngestItem, IngestOutcome};
use tinymemory_api::provider::{AnswerRequest, MemoryProvider, RawMemoryEvent};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::{cleanup, ns};

/// Runs every ingestion assertion for the families `provider` serves, and the
/// answer family's refusal of an empty question.
///
/// # Panics
///
/// Panics on the first violation, naming the driver.
pub async fn assert_ingest_families(provider: &dyn MemoryProvider) {
    assert_document_ingest(provider).await;
    assert_conversation_ingest(provider).await;
    assert_learning_ingest(provider).await;
    assert_event_ingest(provider).await;
    assert_answer_refuses_an_empty_question(provider).await;
}

/// A new document is written, the same document again is not an error, and
/// an empty one is `Invalid`.
///
/// # Panics
///
/// Panics when a valid document is refused or not written, when repeating it
/// fails, or when an empty document is accepted or refused as anything but
/// [`MemoryError::Invalid`].
pub async fn assert_document_ingest(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(ingest) = provider.as_document_ingest() else {
        return;
    };
    let source = run_unique("document");
    let document = item(
        provider,
        "ingest-document",
        DataSource::Upload,
        &source,
        "The conformance lighthouse is painted in red and white bands.",
    );
    let first = ingest
        .ingest_document(document.clone())
        .await
        .unwrap_or_else(|e| panic!("{who}: a valid document was refused: {e}"));
    assert_written(who, "document", &first);

    let again = ingest.ingest_document(document).await.unwrap_or_else(|e| {
        panic!(
            "{who}: sending the same document again must be a replay or a no-op, \
             never an error: {e}"
        )
    });
    assert_consistent(who, "a repeated document", &again);

    let empty = item(
        provider,
        "ingest-document",
        DataSource::Upload,
        &run_unique("document"),
        "   ",
    );
    let refused = ingest.ingest_document(empty).await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{who}: a document with no content must be refused as Invalid, got {refused:?}"
    );
}

/// One conversation is written in one call, repeating it is not an error, and
/// a batch that mixes two conversations is `Invalid`.
///
/// # Panics
///
/// Panics when a valid conversation is refused or not written, when repeating
/// it fails, or when a mixed batch is not refused as [`MemoryError::Invalid`].
pub async fn assert_conversation_ingest(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(ingest) = provider.as_conversation_ingest() else {
        return;
    };
    let source = run_unique("conversation");
    let messages: Vec<IngestItem> = [
        ("user", "Where should the conformance picnic be?"),
        ("assistant", "By the lighthouse, on the north lawn."),
        ("user", "Then bring the blue blanket."),
    ]
    .into_iter()
    .map(|(author, text)| {
        let mut message = item(
            provider,
            "ingest-conversation",
            DataSource::Conversation,
            &source,
            text,
        );
        message.author = Some(author.to_string());
        message
    })
    .collect();
    let first = ingest
        .ingest_conversation(messages.clone())
        .await
        .unwrap_or_else(|e| panic!("{who}: a valid conversation was refused: {e}"));
    assert_written(who, "conversation", &first);

    let again = ingest
        .ingest_conversation(messages)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{who}: sending the same conversation again must be a replay or a no-op, \
             never an error: {e}"
            )
        });
    assert_consistent(who, "a repeated conversation", &again);

    let mixed = ["thread-a", "thread-b"]
        .into_iter()
        .map(|thread| {
            item(
                provider,
                "ingest-conversation",
                DataSource::Conversation,
                &run_unique(thread),
                "a message",
            )
        })
        .collect();
    let refused = ingest.ingest_conversation(mixed).await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{who}: a batch mixing two conversations must be refused as Invalid, got {refused:?}"
    );
}

/// A learning is written, and a confidence outside `0.0..=1.0` is `Invalid`.
///
/// # Panics
///
/// Panics when a valid learning is refused or not written, or when an
/// out-of-range confidence is not refused as [`MemoryError::Invalid`].
pub async fn assert_learning_ingest(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(ingest) = provider.as_learning_ingest() else {
        return;
    };
    let written = ingest
        .ingest_learning(learning(&run_unique("tone"), 0.8))
        .await
        .unwrap_or_else(|e| panic!("{who}: a valid learning was refused: {e}"));
    assert_written(who, "learning", &written);

    for confidence in [1.5, -0.1, f64::NAN] {
        let refused = ingest
            .ingest_learning(learning(&run_unique("tone"), confidence))
            .await;
        assert!(
            matches!(refused, Err(MemoryError::Invalid(_))),
            "{who}: a learning with confidence {confidence} must be refused as Invalid, \
             got {refused:?}"
        );
    }
}

/// An event is written, the same event again is not an error, and an event
/// with no content is `Invalid`.
///
/// # Panics
///
/// Panics when a valid event is refused or not written, when repeating it
/// fails, or when an empty event is not refused as [`MemoryError::Invalid`].
pub async fn assert_event_ingest(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(ingest) = provider.as_event_ingest() else {
        return;
    };
    let event = |id: &str, content: &str| RawMemoryEvent {
        id: id.to_string(),
        namespace: ns(provider, "ingest-event"),
        event_type: "conformance_note".to_string(),
        content: content.to_string(),
        session_id: None,
        occurred_at: None,
        metadata: serde_json::json!({ "suite": "tinymemory-conformance" }),
        taint: MemoryTaint::default(),
    };
    let id = run_unique("event");
    let first = ingest
        .ingest_event(event(&id, "The lighthouse lamp was serviced."))
        .await
        .unwrap_or_else(|e| panic!("{who}: a valid event was refused: {e}"));
    assert_written(who, "event", &first);

    let again = ingest
        .ingest_event(event(&id, "The lighthouse lamp was serviced."))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{who}: sending the same event again must be a replay or a no-op, \
                 never an error: {e}"
            )
        });
    assert_consistent(who, "a repeated event", &again);

    let refused = ingest.ingest_event(event(&run_unique("event"), "  ")).await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{who}: an event with no content must be refused as Invalid, got {refused:?}"
    );
}

/// An empty question is `Invalid`.
///
/// # Panics
///
/// Panics when an empty question is answered, or refused as anything but
/// [`MemoryError::Invalid`].
pub async fn assert_answer_refuses_an_empty_question(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(answerer) = provider.as_answer() else {
        return;
    };
    let refused = answerer.answer(AnswerRequest::new("   ")).await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{who}: an empty question must be refused as Invalid, got {refused:?}"
    );
}

/// Opt-in: the engine answers a question from a fact it was just given, and
/// cites evidence.
///
/// Not part of [`assert_provider`](super::assert_provider): a grounded answer
/// needs a model behind the engine, which an offline double or an embedded
/// engine without inference does not have. A live run calls it.
///
/// # Panics
///
/// Panics when the fact cannot be stored, the question is not answered, the
/// answer does not use the fact, or it cites nothing.
pub async fn assert_answer_is_grounded(provider: &dyn MemoryProvider) {
    let who = provider.driver_id();
    let Some(answerer) = provider.as_answer() else {
        return;
    };
    let namespace = format!("{}/{}", ns(provider, "answer"), run_nonce());
    provider
        .store(
            &namespace,
            "keeper",
            "The conformance lighthouse keeper is named Ottoline Varga.",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .unwrap_or_else(|e| panic!("{who}: storing the answer's fact failed: {e}"));
    let mut request = AnswerRequest::new("What is the name of the lighthouse keeper?");
    request.recall.namespace = Some(namespace.clone());
    let answered = answerer
        .answer(request)
        .await
        .unwrap_or_else(|e| panic!("{who}: a question about a stored fact failed: {e}"));
    cleanup(provider, &namespace, &["keeper"]).await;
    assert!(
        answered.answer.to_lowercase().contains("ottoline"),
        "{who}: the answer did not use the stored fact: {:?}",
        answered.answer
    );
    assert!(
        !answered.citations.is_empty(),
        "{who}: a grounded answer must cite its evidence"
    );
}

/// A new source was written, and the outcome agrees with itself.
fn assert_written(who: &str, what: &str, outcome: &IngestOutcome) {
    assert!(
        outcome.written >= 1 && !outcome.already_ingested,
        "{who}: a {what} never ingested before must be written, got {outcome:?}"
    );
    assert_consistent(who, what, outcome);
}

/// An outcome that cannot be read two ways: no more ids than written units,
/// and not both "written" and "this was a no-op".
fn assert_consistent(who: &str, what: &str, outcome: &IngestOutcome) {
    assert!(
        outcome.ids.len() <= outcome.written as usize,
        "{who}: {what} reported more ids than written units: {outcome:?}"
    );
    assert!(
        !(outcome.already_ingested && outcome.written > 0),
        "{who}: {what} reported both writing and being a no-op: {outcome:?}"
    );
}

/// A plain-text ingest item for this driver's `what` namespace.
fn item(
    provider: &dyn MemoryProvider,
    what: &str,
    source: DataSource,
    source_id: &str,
    text: &str,
) -> IngestItem {
    IngestItem {
        namespace: Some(ns(provider, what)),
        source,
        source_id: source_id.to_string(),
        owner: "tinymemory-conformance".to_string(),
        source_ref: None,
        content: text.to_string(),
        mime: Some("text/plain".to_string()),
        timestamp: None,
        tags: Vec::new(),
        author: None,
        channel_label: None,
        platform: Some("tinymemory-conformance".to_string()),
        to: Vec::new(),
        cc: Vec::new(),
        subject: None,
        list_unsubscribe: None,
        taint: MemoryTaint::default(),
        path_scope: None,
    }
}

/// A learning candidate with the given confidence.
fn learning(key: &str, initial_confidence: f64) -> LearningCandidate {
    LearningCandidate {
        class: FacetClass::Tooling,
        key: key.to_string(),
        value: "terse".to_string(),
        cue_family: CueFamily::Explicit,
        evidence: EvidenceRef::ToolCall {
            tool_name: "tinymemory-conformance".to_string(),
            episodic_id: 1,
        },
        initial_confidence,
        observed_at: 1_700_000_000.0,
    }
}

/// `what` plus this run's nonce, so a live service's earlier material is never
/// mistaken for this run's.
fn run_unique(what: &str) -> String {
    format!("conformance-{what}-{}", run_nonce())
}

/// A token distinct per call within a run and across runs.
fn run_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}
