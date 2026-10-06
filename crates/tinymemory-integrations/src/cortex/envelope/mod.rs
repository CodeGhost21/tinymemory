//! How a TinyMemory item is laid out as CortexDB events.
//!
//! # Scopes
//!
//! Every item lives in one scope per kind under its namespace node, below the
//! TinyMemory root [`ROOT_SCOPE`] ([`scope_path`]):
//!
//! ```text
//! app:tinymemory/app:{documents,conversations,learnings}                 the root node
//! app:tinymemory/agent:researcher/app:{documents,conversations,learnings} an agent
//! app:tinymemory/team:acme/agent:writer/app:learnings                    a team member
//! ```
//!
//! So within every node, learnings, documents and conversations are separate
//! scopes, and CortexDB can recall, retain and erase each on its own. The
//! namespace segments map onto CortexDB's built-in scope types (`agent`,
//! `team`, `user`, `ws`, `project`, `source`), which every shipped
//! deployment preset allows. A brain's per-source documents therefore live in
//! `app:tinymemory/source:pdf/app:documents`. The hosted backend additionally re-roots every scope under the
//! caller's tenant, which is invisible here. A
//! [`tinymemory_api::MetaFilter`]'s `kinds` and `reach` pick which scopes are
//! read (see `engine::scopes`).
//!
//! # Events
//!
//! A learning is one event. A conversation is one event per turn, appended
//! in order. A document is one event, or, when its envelope would be too big
//! for one, one event per piece of its body ([`chunks`]), appended in order;
//! each piece carries its index, and the pages and section it covers, and
//! the pieces concatenate back to the body.
//!
//! No event is sent whose encoded envelope is over
//! [`chunks::MAX_EVENT_TEXT_BYTES`] ([`Envelope::encode_checked`]): CortexDB
//! refuses an experience over 1 MiB of text, and a conversation turn or a
//! learning cannot be split. Each event's `content.text` is a JSON
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

pub(crate) mod chunks;
pub(crate) mod labels;
mod rebuild;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tinymemory_api::chrono::{DateTime, Utc};
use tinymemory_api::{
    DocumentBody, ItemKind, LearningKind, MemoryMeta, Namespace, Role, StoreItem, ToolCallRef,
};

use crate::cortex::error::{Error, Result};

pub(crate) use rebuild::{Decoded, decode_event, rebuild};

/// The TinyMemory root every kind scope sits under.
pub(crate) const ROOT_SCOPE: &str = "app:tinymemory";

/// The envelope version this crate writes and reads.
const VERSION: u8 = 2;

/// The leaf segment of `kind`'s scope inside a namespace node.
pub(crate) fn kind_leaf(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Document => "app:documents",
        ItemKind::Conversation => "app:conversations",
        ItemKind::Learning => "app:learnings",
    }
}

/// The scope items of `kind` at `namespace` live in.
pub(crate) fn scope_path(namespace: &Namespace, kind: ItemKind) -> String {
    let mut path = String::from(ROOT_SCOPE);
    for segment in namespace.segments() {
        path.push('/');
        path.push_str(&segment.to_string());
    }
    path.push('/');
    path.push_str(kind_leaf(kind));
    path
}

/// The namespace and kind of a TinyMemory scope path, wherever it is rooted
/// (the hosted backend prefixes the caller's tenant); `None` for any other
/// scope.
pub(crate) fn parse_scope(path: &str) -> Option<(Namespace, ItemKind)> {
    let mut parts = path.split('/');
    parts.by_ref().find(|part| *part == ROOT_SCOPE)?;
    let rest: Vec<&str> = parts.collect();
    let (leaf, nodes) = rest.split_last()?;
    let kind = ItemKind::ALL
        .into_iter()
        .find(|kind| kind_leaf(*kind) == *leaf)?;
    let namespace = nodes.join("/").parse().ok()?;
    Some((namespace, kind))
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
    /// Which piece of a chunked document this event is; absent for a
    /// document written as one event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) chunk: Option<ChunkInfo>,
}

/// A piece of a chunked document: its place, and what it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChunkInfo {
    /// Zero-based position.
    pub(crate) index: u32,
    /// How many pieces the document has.
    pub(crate) count: u32,
    /// The first and last page the piece covers, counted from 1, when the
    /// document marks its pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pages: Option<[u32; 2]>,
    /// The title of the section the piece starts in, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<String>,
}

/// Room left in a piece's envelope for its section title: the longest title
/// at the worst JSON escaping, plus its key.
const SECTION_RESERVE: usize = chunks::MAX_SECTION_CHARS * 6 + 32;

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
            chunk: None,
        }
    }

    /// The position of this event within its item: a conversation turn's
    /// index, a document piece's index, or `None` for an item written as
    /// one event.
    pub(crate) fn part(&self) -> Option<u32> {
        self.turn
            .as_ref()
            .map(|turn| turn.index)
            .or_else(|| self.chunk.as_ref().map(|chunk| chunk.index))
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
                let mut whole = Self::new(id, ItemKind::Document, String::new(), meta);
                whole.title.clone_from(title);
                whole.mime.clone_from(mime);
                // No room for a piece (metadata alone near the limit) leaves
                // the document whole: it is written if it fits and refused
                // by `encode_checked` if not, never cut into pieces that are
                // each over the limit.
                let pieces = chunks::split(
                    text,
                    whole.piece_overhead()?,
                    chunks::DOCUMENT_CHUNK_TARGET_BYTES,
                    chunks::MAX_EVENT_TEXT_BYTES,
                )
                .unwrap_or_default();
                if pieces.len() <= 1 {
                    whole.text.clone_from(text);
                    return Ok(vec![whole]);
                }
                let count = u32::try_from(pieces.len()).map_err(|_| {
                    Error::InvalidRequest("document has too many pieces".to_string())
                })?;
                Ok((0..count)
                    .zip(pieces)
                    .map(|(index, piece)| {
                        let mut envelope = whole.clone();
                        envelope.text = piece.text.to_string();
                        envelope.chunk = Some(ChunkInfo {
                            index,
                            count,
                            pages: piece.pages.map(|(first, last)| [first, last]),
                            section: piece.section,
                        });
                        envelope
                    })
                    .collect())
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

    /// The stored text, refused when it is over
    /// [`chunks::MAX_EVENT_TEXT_BYTES`]: CortexDB would refuse the event,
    /// so nothing of the item is sent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for an envelope over the limit (a
    /// conversation turn or a learning that long, or metadata too large to
    /// leave room for a document piece); as [`Envelope::encode`] otherwise.
    pub(crate) fn encode_checked(&self) -> Result<String> {
        let encoded = self.encode()?;
        if encoded.len() > chunks::MAX_EVENT_TEXT_BYTES {
            return Err(Error::InvalidRequest(format!(
                "a {:?} event would be {} bytes; CortexDB refuses an event over 1 MiB, so at \
                 most {} are sent",
                self.kind,
                encoded.len(),
                chunks::MAX_EVENT_TEXT_BYTES
            )));
        }
        Ok(encoded)
    }

    /// The encoded size of this envelope as a document piece with an empty
    /// text: the room every piece's own text is added to.
    fn piece_overhead(&self) -> Result<usize> {
        let mut probe = self.clone();
        probe.chunk = Some(ChunkInfo {
            index: u32::MAX,
            count: u32::MAX,
            pages: Some([u32::MAX, u32::MAX]),
            section: None,
        });
        Ok(probe.encode()?.len() + SECTION_RESERVE)
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
            "scope": scope_path(&self.meta.namespace, self.kind),
            "modality": modality,
            "idempotency_key": crate::cortex::transport::fresh_idempotency_key(),
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
