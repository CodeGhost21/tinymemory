//! Helpers every operation shares: which kinds a filter admits, finding an
//! item's events by its label, and turning a rebuilt item into a `Hit`.

use std::collections::{BTreeMap, HashMap};

use futures::{StreamExt, TryStreamExt, stream};

use tinymemory_api::explore::in_request_order;
use tinymemory_api::{
    GetRequest, Hit, ItemId, ItemKind, MemoryMeta, MetaFilter, Namespace, StoreItem,
};

use super::CortexEngine;
use super::scopes::KindScope;
use crate::cortex::envelope::{Decoded, Envelope, decode_event, labels, rebuild, rebuild_whole};
use crate::cortex::error::Result;

/// The kinds `filter` admits, in the fixed order
/// [`ItemKind::ALL`] lists them.
pub(super) fn admitted(filter: &MetaFilter) -> Vec<ItemKind> {
    ItemKind::ALL
        .into_iter()
        .filter(|kind| filter.admits_kind(*kind))
        .collect()
}

/// Whether a decoded event is an item of `kind` that `filter` keeps. A
/// path the envelope left out (see `envelope`) is matched by its digests as
/// well as by what the envelope kept.
pub(super) fn keeps(filter: &MetaFilter, kind: ItemKind, envelope: &Envelope) -> bool {
    if envelope.kind != kind {
        return false;
    }
    let paths = &envelope.paths;
    let mut rest = filter.clone();
    if let (Some(wanted), Some(held)) = (&filter.workspace, &paths.workspace) {
        if labels::path_digest(wanted) != *held {
            return false;
        }
        rest.workspace = None;
    }
    for (wanted, held, cleared) in [
        (&filter.file_path, &paths.file_path, &mut rest.file_path),
        (&filter.folder, &paths.folder, &mut rest.folder),
    ] {
        // Matched by the full path's digests, or else by what the envelope
        // kept (a file's name, which reads give back).
        if wanted
            .as_deref()
            .is_some_and(|wanted| names_a_prefix(wanted, held))
        {
            *cleared = None;
        }
    }
    rest.matches(kind, &envelope.meta)
}

/// Whether the path filter `wanted` matches a path whose prefixes have the
/// digests `held`: as the path itself, or as a folder above it.
fn names_a_prefix(wanted: &str, held: &[String]) -> bool {
    [wanted, wanted.trim_end_matches('/')]
        .iter()
        .any(|value| held.contains(&labels::path_digest(value)))
}

/// Namespaces whose item events are looked up at once when assembling
/// whole items: each is one listing, and reading them one after the other
/// made a read's latency grow with the number of namespaces it hit.
pub(super) const LOOKUPS_AT_ONCE: usize = 4;

/// The whole conversation `envelope` holds when it is the conversation's
/// only turn (`turn.count == 1`), as every turn a host logs per item is:
/// nothing more is stored, so no lookup can add to it. `None` otherwise.
pub(super) fn one_turn_conversation(envelope: &Envelope) -> Option<StoreItem> {
    let one_turn = envelope.kind == ItemKind::Conversation
        && envelope.turn.as_ref().is_some_and(|turn| turn.count == 1);
    if one_turn {
        rebuild_whole(std::slice::from_ref(envelope))
    } else {
        None
    }
}

/// An envelope's metadata, located: for a piece of a chunked document, the
/// item's metadata plus a `page:<n>` (or `page:<first>-<last>`) tag and a
/// `section:<title>` tag for what the piece covers. Read-side only: the
/// stored item carries neither.
pub(super) fn located_meta(envelope: &Envelope) -> MemoryMeta {
    let mut meta = envelope.meta.clone();
    if let Some(chunk) = &envelope.chunk {
        match chunk.pages {
            Some([first, last]) if first == last => meta.tags.push(format!("page:{first}")),
            Some([first, last]) => meta.tags.push(format!("page:{first}-{last}")),
            None => {}
        }
        if let Some(section) = &chunk.section {
            meta.tags.push(format!("section:{section}"));
        }
    }
    meta
}

/// A ranked hit for one event's envelope: a learning or a whole document as
/// it was stored, or, for a piece of a chunked document, that piece (the
/// item's id, the piece's text, its located metadata).
pub(super) fn event_hit(envelope: &Envelope, score: f32) -> Option<Hit> {
    let item = rebuild(std::slice::from_ref(envelope))?;
    let mut found = hit(&envelope.id, &item, score);
    found.meta = located_meta(envelope);
    Some(found)
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
    /// Every event of each item in `ids` held in `scope`, grouped by item id.
    /// Found by the items' labels (one listing per batch of ids), then
    /// re-checked against the envelope, because a label is a digest.
    pub(super) async fn item_events(
        &self,
        scope: &KindScope,
        ids: &[String],
    ) -> Result<HashMap<String, Vec<Decoded>>> {
        let kind = scope.kind;
        let mut grouped: HashMap<String, Vec<Decoded>> = HashMap::new();
        if ids.is_empty() {
            return Ok(grouped);
        }
        let wanted: Vec<String> = ids.iter().map(|id| labels::item(id)).collect();
        for event in self.log.walk_labels(&scope.path, &wanted).await? {
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

    /// `get`: every named item, rebuilt from its events in each scope the
    /// request's reach reads (an id names one item, so one scope holds it).
    pub(super) async fn get_items(&self, req: GetRequest) -> Result<Vec<Hit>> {
        req.validate()?;
        let ids: Vec<String> = req.ids.iter().map(|id| id.as_str().to_string()).collect();
        let filter = MetaFilter {
            reach: req.reach.clone(),
            ..MetaFilter::default()
        };
        let mut found = BTreeMap::new();
        for scope in self.scopes_for(&filter).await? {
            if found.len() == ids.len() {
                break;
            }
            for (id, events) in self.item_events(&scope, &ids).await? {
                let envelopes: Vec<Envelope> = events.into_iter().map(|d| d.envelope).collect();
                if let Some(item) = rebuild_whole(&envelopes) {
                    found.insert(ItemId::new(id.clone()), hit(&id, &item, 0.0));
                }
            }
        }
        Ok(in_request_order(&req.ids, found))
    }

    /// The whole conversations named by `ids`, each at its namespace,
    /// rebuilt from all their turns (one lookup per namespace).
    pub(super) async fn conversations(
        &self,
        ids: &[(String, Namespace)],
    ) -> Result<HashMap<String, StoreItem>> {
        self.assembled(ItemKind::Conversation, ids).await
    }

    /// The whole items of `kind` named by `ids`, each at its namespace,
    /// rebuilt from all their events: a conversation's turns, a chunked
    /// document's pieces (one lookup per namespace, [`LOOKUPS_AT_ONCE`] at
    /// a time).
    pub(super) async fn assembled(
        &self,
        kind: ItemKind,
        ids: &[(String, Namespace)],
    ) -> Result<HashMap<String, StoreItem>> {
        let mut by_node: BTreeMap<&Namespace, Vec<String>> = BTreeMap::new();
        for (id, namespace) in ids {
            by_node.entry(namespace).or_default().push(id.clone());
        }
        let lookups: Vec<(KindScope, Vec<String>)> = by_node
            .into_iter()
            .map(|(namespace, ids)| (KindScope::new(&self.layout, namespace.clone(), kind), ids))
            .collect();
        let found: Vec<HashMap<String, Vec<Decoded>>> = stream::iter(lookups)
            .map(|(scope, ids)| async move { self.item_events(&scope, &ids).await })
            .buffer_unordered(LOOKUPS_AT_ONCE)
            .try_collect()
            .await?;
        let mut out = HashMap::new();
        for (id, events) in found.into_iter().flatten() {
            let envelopes: Vec<Envelope> = events.into_iter().map(|d| d.envelope).collect();
            if let Some(item) = rebuild_whole(&envelopes) {
                out.insert(id, item);
            }
        }
        Ok(out)
    }
}
