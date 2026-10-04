//! Reading events back into envelopes and envelopes back into items.

use serde_json::Value;
use tinymemory_api::{DocumentBody, ItemKind, LearningKind, StoreItem, Turn};

use super::Envelope;

/// One of this crate's events, decoded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decoded {
    /// The engine's event id.
    pub(crate) event_id: String,
    /// The envelope it carries.
    pub(crate) envelope: Envelope,
}

/// Decodes one event from a listing or a recall pack. `None` for an event
/// with no id or text, or one this crate did not write.
pub(crate) fn decode_event(event: &Value) -> Option<Decoded> {
    let event_id = event.get("id").and_then(Value::as_str)?.to_string();
    let text = event.pointer("/content/text").and_then(Value::as_str)?;
    let envelope = Envelope::decode(text)?;
    Some(Decoded { event_id, envelope })
}

/// The item a set of one item's envelopes describes.
///
/// A document or learning takes the first envelope. A conversation orders
/// its turns by index and keeps one envelope per index, so a duplicated or
/// re-written turn does not repeat; turns that were never written (a store
/// that failed part-way) are simply absent. `None` for an empty set.
pub(crate) fn rebuild(envelopes: &[Envelope]) -> Option<StoreItem> {
    let first = envelopes.first()?;
    Some(match first.kind {
        ItemKind::Document => StoreItem::Document {
            title: first.title.clone(),
            body: DocumentBody::Text(first.text.clone()),
            mime: first.mime.clone(),
            meta: first.meta.clone(),
        },
        ItemKind::Learning => StoreItem::Learning {
            text: first.text.clone(),
            kind: first.learning_kind.unwrap_or(LearningKind::Other),
            confidence: first.confidence.unwrap_or_default(),
            evidence: first.evidence.clone(),
            meta: first.meta.clone(),
        },
        ItemKind::Conversation => {
            let mut turns: Vec<(u32, Turn)> = envelopes
                .iter()
                .filter_map(|envelope| {
                    let info = envelope.turn.as_ref()?;
                    Some((
                        info.index,
                        Turn {
                            role: info.role,
                            text: envelope.text.clone(),
                            at: info.at,
                            tool_calls: info.tool_calls.clone(),
                        },
                    ))
                })
                .collect();
            turns.sort_by_key(|(index, _)| *index);
            turns.dedup_by_key(|(index, _)| *index);
            StoreItem::Conversation {
                turns: turns.into_iter().map(|(_, turn)| turn).collect(),
                meta: first.meta.clone(),
            }
        }
    })
}
