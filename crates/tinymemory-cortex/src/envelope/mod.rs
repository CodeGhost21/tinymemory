//! How a TinyMemory item is laid out as CortexDB events.
//!
//! # Scopes
//!
//! Every item lives in one scope per kind under the TinyMemory root
//! [`ROOT_SCOPE`]: `tm:memory/tm:documents`, `tm:memory/tm:conversations`
//! and `tm:memory/tm:learnings` ([`scope_of`]). The hosted backend
//! additionally re-roots every scope under the caller's tenant, which is
//! invisible here. A [`tinymemory_api::MetaFilter`]'s `kinds` picks which of
//! the three are read.
//!
//! # Events
//!
//! A document or a learning is one event. A conversation is one event per
//! turn, appended in order. Each event's `content.text` is a JSON
//! [`Envelope`] (`"v": 2`) carrying the item id, kind, the event's own text
//! (the body, the turn's text, or the learning's statement), the item's full
//! [`MemoryMeta`], and the kind's extra fields. CortexDB's experience schema
//! is closed (an unknown field is a 422), so the envelope rides in the one
//! free-form field there is; anything that does not parse as a v2 envelope is
//! somebody else's event and is ignored.
//!
//! Each event also carries lookup labels (see [`labels`]) and, when the item
//! has one, `context.observed_at`.
//!
//! # Identity
//!
//! The item id is [`StoreItem::fingerprint`]: a content digest, so storing
//! an identical item again resolves to the same id and is detected as a
//! replay by looking that id's label up.

pub(crate) mod labels;
mod rebuild;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tinymemory_api::chrono::{DateTime, Utc};
use tinymemory_api::{
    DocumentBody, ItemKind, LearningKind, MemoryMeta, Role, StoreItem, ToolCallRef,
};

use crate::error::{Error, Result};

pub(crate) use rebuild::{Decoded, decode_event, rebuild};

/// The TinyMemory root every kind scope sits under.
pub(crate) const ROOT_SCOPE: &str = "tm:memory";

/// The envelope version this crate writes and reads.
const VERSION: u8 = 2;

/// The scope items of `kind` live in.
pub(crate) fn scope_of(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Document => "tm:memory/tm:documents",
        ItemKind::Conversation => "tm:memory/tm:conversations",
        ItemKind::Learning => "tm:memory/tm:learnings",
    }
}

/// One event's payload: the item it belongs to and the event's share of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Envelope {
    /// Always [`VERSION`].
    pub(crate) v: u8,
    /// The item id ([`StoreItem::fingerprint`]).
    pub(crate) id: String,
    /// The item's kind.
    pub(crate) kind: ItemKind,
    /// The document body, the turn's text, or the learning's statement.
    pub(crate) text: String,
    /// The item's metadata, whole, on every event.
    pub(crate) meta: MemoryMeta,
    /// Document title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    /// Document MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mime: Option<String>,
    /// Learning kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) learning_kind: Option<LearningKind>,
    /// Learning confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) confidence: Option<f32>,
    /// Learning evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) evidence: Option<String>,
    /// Which turn of a conversation this event is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn: Option<TurnInfo>,
}

/// A conversation turn's place and attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TurnInfo {
    /// Zero-based position.
    pub(crate) index: u32,
    /// How many turns the conversation has.
    pub(crate) count: u32,
    /// Who spoke.
    pub(crate) role: Role,
    /// When, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) at: Option<DateTime<Utc>>,
    /// Tool calls the turn made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_calls: Vec<ToolCallRef>,
}

impl Envelope {
    /// A bare envelope for item `id` of `kind`.
    fn new(id: &str, kind: ItemKind, text: String, meta: &MemoryMeta) -> Self {
        Self {
            v: VERSION,
            id: id.to_string(),
            kind,
            text,
            meta: meta.clone(),
            title: None,
            mime: None,
            learning_kind: None,
            confidence: None,
            evidence: None,
            turn: None,
        }
    }

    /// The envelopes `item` is written as, one per event, in write order.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a document whose body is still a URI,
    /// or a conversation with more turns than a `u32` counts.
    pub(crate) fn for_item(item: &StoreItem, id: &str) -> Result<Vec<Self>> {
        match item {
            StoreItem::Document {
                title,
                body,
                mime,
                meta,
            } => {
                let DocumentBody::Text(text) = body else {
                    return Err(Error::InvalidRequest(
                        "document body is an unresolved uri".to_string(),
                    ));
                };
                let mut envelope = Self::new(id, ItemKind::Document, text.clone(), meta);
                envelope.title.clone_from(title);
                envelope.mime.clone_from(mime);
                Ok(vec![envelope])
            }
            StoreItem::Learning {
                text,
                kind,
                confidence,
                evidence,
                meta,
            } => {
                let mut envelope = Self::new(id, ItemKind::Learning, text.clone(), meta);
                envelope.learning_kind = Some(*kind);
                envelope.confidence = Some(*confidence);
                envelope.evidence.clone_from(evidence);
                Ok(vec![envelope])
            }
            StoreItem::Conversation { turns, meta } => {
                let count = u32::try_from(turns.len()).map_err(|_| {
                    Error::InvalidRequest("conversation has too many turns".to_string())
                })?;
                let mut out = Vec::with_capacity(turns.len());
                for (index, turn) in (0..count).zip(turns) {
                    let mut envelope =
                        Self::new(id, ItemKind::Conversation, turn.text.clone(), meta);
                    envelope.turn = Some(TurnInfo {
                        index,
                        count,
                        role: turn.role,
                        at: turn.at,
                        tool_calls: turn.tool_calls.clone(),
                    });
                    out.push(envelope);
                }
                Ok(out)
            }
        }
    }

    /// Reads an envelope from an event's text, whichever read path it came
    /// from.
    ///
    /// The two read paths disagree on the bytes: `/v1/events` returns the
    /// text as stored, while `/v1/recall` renders it for a reader and
    /// prefixes the speaker (`[user] {...}`). The prefix is stripped only
    /// when the text does not parse without it. Anything that is not a v2
    /// envelope is `None`.
    pub(crate) fn decode(text: &str) -> Option<Self> {
        let parsed = serde_json::from_str::<Self>(text).ok().or_else(|| {
            let rendered = text.strip_prefix('[')?;
            let (_role, rest) = rendered.split_once("] ")?;
            serde_json::from_str::<Self>(rest).ok()
        })?;
        (parsed.v == VERSION).then_some(parsed)
    }

    /// The stored text.
    ///
    /// # Errors
    ///
    /// [`Error::Engine`] if serialisation fails, which plain data cannot.
    pub(crate) fn encode(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|_| Error::Engine("an item envelope could not be serialised".to_string()))
    }

    /// The experience request appending this envelope, with a fresh body
    /// idempotency key.
    pub(crate) fn request(&self, text: &str) -> Value {
        let (modality, role) = match (&self.kind, &self.turn) {
            (ItemKind::Conversation, Some(turn)) => ("conversation", role_of(turn.role)),
            (ItemKind::Document, _) => ("document", "user"),
            _ => ("observation", "user"),
        };
        let observed_at = self
            .turn
            .as_ref()
            .and_then(|turn| turn.at)
            .or(self.meta.observed_at);
        let mut context = serde_json::Map::new();
        context.insert(
            "labels".to_string(),
            json!(labels::for_item(&self.id, &self.meta)),
        );
        if let Some(at) = observed_at {
            context.insert("observed_at".to_string(), json!(at.to_rfc3339()));
        }
        json!({
            "scope": scope_of(self.kind),
            "modality": modality,
            "idempotency_key": crate::transport::fresh_idempotency_key(),
            "content": { "kind": "message", "role": role, "text": text },
            "context": Value::Object(context),
        })
    }
}

/// CortexDB's four-value message role for a turn's speaker.
fn role_of(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
