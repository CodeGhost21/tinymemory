//! Reading events back into envelopes and envelopes back into items.

use std::collections::HashSet;

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
/// A learning, or a document written as one event (no `chunk` field, as
/// every document was before chunking), takes the first envelope. A chunked
/// document (every piece carries `chunk`) orders its pieces by index, keeps one per
/// index and concatenates their text, so the full set gives back the body
/// exactly (and one piece alone gives that piece). A conversation orders
/// its turns by index and keeps one envelope per index, so a duplicated or
/// re-written turn does not repeat; turns that were never written (a store
/// that failed part-way) are simply absent. `None` for an empty set.
pub(crate) fn rebuild(envelopes: &[Envelope]) -> Option<StoreItem> {
    let first = envelopes.first()?;
    Some(match first.kind {
        ItemKind::Document => StoreItem::Document {
            title: first.title.clone(),
            body: DocumentBody::Text(document_text(envelopes)),
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

/// [`rebuild`] for a read that returns whole items (`get`, `list`): `None`
/// for a chunked document missing a piece, from a store that failed part-way
/// (the next store of the item writes the missing ones) or pieces the engine
/// does not list yet. A `fetch` hit or a `recall` citation is one piece and
/// uses [`rebuild`].
pub(crate) fn rebuild_whole(envelopes: &[Envelope]) -> Option<StoreItem> {
    if !pieces_complete(envelopes) {
        log::debug!(
            "[cortex] chunked document {:?} is missing pieces or disagrees on its layout; \
             not returned whole",
            envelopes.first().map(|envelope| &envelope.id)
        );
        return None;
    }
    rebuild(envelopes)
}

/// Whether `envelopes` hold a whole item: one written as a single event
/// (a document's unchunked envelope is its whole body), or every piece of a
/// chunked document, all agreeing on one positive count with every index
/// below it and each index present.
fn pieces_complete(envelopes: &[Envelope]) -> bool {
    if envelopes.iter().any(|envelope| envelope.chunk.is_none()) {
        return true;
    }
    let Some(count) = envelopes.iter().find_map(|e| Some(e.chunk.as_ref()?.count)) else {
        return true;
    };
    let mut held = HashSet::new();
    for chunk in envelopes
        .iter()
        .filter_map(|envelope| envelope.chunk.as_ref())
    {
        if chunk.count != count || chunk.index >= count {
            return false;
        }
        held.insert(chunk.index);
    }
    count > 0 && held.len() == count as usize
}

/// A document's text from its envelopes: an unchunked envelope's (the whole
/// body: the same item written before chunking, since an id is a content
/// digest), else the pieces of a chunked document in index order, each once.
fn document_text(envelopes: &[Envelope]) -> String {
    if let Some(whole) = envelopes.iter().find(|envelope| envelope.chunk.is_none()) {
        return whole.text.clone();
    }
    let mut pieces: Vec<(u32, &str)> = envelopes
        .iter()
        .filter_map(|envelope| Some((envelope.chunk.as_ref()?.index, envelope.text.as_str())))
        .collect();
    pieces.sort_by_key(|(index, _)| *index);
    pieces.dedup_by_key(|(index, _)| *index);
    pieces.into_iter().map(|(_, text)| text).collect()
}
