//! Helpers every operation shares: which kinds a filter admits, finding an
//! item's events by its label, and turning a rebuilt item into a `Hit`.

use std::collections::HashMap;

use tinymemory_api::{Hit, ItemId, ItemKind, MetaFilter, StoreItem};

use super::CortexEngine;
use crate::envelope::{Decoded, Envelope, decode_event, labels, rebuild, scope_of};
use crate::error::Result;

/// The kinds `filter` admits, in the fixed order
/// [`ItemKind::ALL`] lists them.
pub(super) fn admitted(filter: &MetaFilter) -> Vec<ItemKind> {
    ItemKind::ALL
        .into_iter()
        .filter(|kind| filter.admits_kind(*kind))
        .collect()
}

/// Whether a decoded event is an item of `kind` that `filter` keeps.
pub(super) fn keeps(filter: &MetaFilter, kind: ItemKind, envelope: &Envelope) -> bool {
    envelope.kind == kind && filter.matches(kind, &envelope.meta)
}

/// A hit for `item`.
pub(super) fn hit(id: &str, item: &StoreItem, score: f32) -> Hit {
    Hit {
        id: ItemId::new(id),
        kind: item.kind(),
        text: item.render_text(),
        meta: item.meta().clone(),
        score,
        confidence: item.confidence(),
    }
}

impl CortexEngine {
    /// Every event of each item in `ids` held in `kind`'s scope, grouped by
    /// item id. Found by the items' labels (one listing per batch of ids),
    /// then re-checked against the envelope, because a label is a digest.
    pub(super) async fn item_events(
        &self,
        kind: ItemKind,
        ids: &[String],
    ) -> Result<HashMap<String, Vec<Decoded>>> {
        let mut grouped: HashMap<String, Vec<Decoded>> = HashMap::new();
        if ids.is_empty() {
            return Ok(grouped);
        }
        let wanted: Vec<String> = ids.iter().map(|id| labels::item(id)).collect();
        for event in self.log.walk_labels(scope_of(kind), &wanted).await? {
            let Some(decoded) = decode_event(&event) else {
                continue;
            };
            if decoded.envelope.kind == kind && ids.contains(&decoded.envelope.id) {
                grouped
                    .entry(decoded.envelope.id.clone())
                    .or_default()
                    .push(decoded);
            }
        }
        Ok(grouped)
    }

    /// The whole conversations named by `ids`, rebuilt from all their turns.
    pub(super) async fn conversations(&self, ids: &[String]) -> Result<HashMap<String, StoreItem>> {
        let mut out = HashMap::new();
        for (id, events) in self.item_events(ItemKind::Conversation, ids).await? {
            let envelopes: Vec<Envelope> = events.into_iter().map(|d| d.envelope).collect();
            if let Some(item) = rebuild(&envelopes) {
                out.insert(id, item);
            }
        }
        Ok(out)
    }
}
