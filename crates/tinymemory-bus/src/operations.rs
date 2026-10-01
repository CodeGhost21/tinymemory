//! High-level ingestion and answer payloads.
//!
//! The lower-level provider families expose the engine's storage primitives.
//! These values describe the product-facing routes that connectors negotiate:
//! document, conversation, learning, and event ingestion, plus grounded answer
//! synthesis. Recall keeps using [`crate::recall`] and [`crate::types::MemoryEntry`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::provider::types::SourceScope;
use crate::recall::OwnedRecallOpts;
use crate::types::MemoryTaint;

/// One raw event supplied by an application or connector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawMemoryEvent {
    /// Stable idempotency key.
    pub id: String,
    /// Logical event namespace.
    pub namespace: String,
    /// Open event-type vocabulary, such as `calendar_changed` or `tool_call`.
    pub event_type: String,
    /// Human-readable event content indexed for recall.
    pub content: String,
    /// When the event occurred, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<DateTime<Utc>>,
    /// Session associated with the event, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Connector-defined structured data retained with the event.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Provenance assigned by the host.
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// A grounded, agentic answer request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerRequest {
    /// The question to answer.
    pub query: String,
    /// Maximum number of memories the answering agent may retrieve.
    #[serde(default = "default_answer_limit")]
    pub limit: usize,
    /// Recall filters applied before synthesis.
    #[serde(default)]
    pub recall: OwnedRecallOpts,
    /// Optional per-turn source allowlist.
    #[serde(default)]
    pub scope: Option<SourceScope>,
    /// Optional caller guidance for tone, format, or focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

fn default_answer_limit() -> usize {
    12
}

impl AnswerRequest {
    /// Construct a request with conservative retrieval defaults.
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: default_answer_limit(),
            recall: OwnedRecallOpts::default(),
            scope: None,
            instructions: None,
        }
    }
}

/// One memory cited by an answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerCitation {
    /// Stable driver record id.
    pub id: String,
    /// Namespace containing the record, when the backend exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Human-readable key or title.
    pub key: String,
    /// Retrieved text supplied to the answering agent.
    pub content: String,
    /// Backend relevance score, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// Observable retrieval work performed while producing an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerStep {
    /// Stable operation name, such as `recall` or `synthesise`.
    pub operation: String,
    /// Short, content-free description safe for logs and user interfaces.
    pub detail: String,
}

/// A grounded answer and the retrieval evidence behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerResponse {
    /// Synthesised prose answer.
    pub answer: String,
    /// Memories made available to the answering agent, in rank order.
    #[serde(default)]
    pub citations: Vec<AnswerCitation>,
    /// High-level execution trace; never contains prompts or credentials.
    #[serde(default)]
    pub steps: Vec<AnswerStep>,
    /// Model identifier used for synthesis, when the provider exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
