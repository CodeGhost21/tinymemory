//! Fetch: hybrid retrieval through CortexDB recall packs.
//!
//! Only [`FetchMode::Hybrid`] is served: the recall body has no field that
//! chooses lexical or embedding retrieval (see `descriptor`).
//!
//! For each admitted kind scope the engine asks recall for a pack of
//! events (`budgets.per_layer_limits.events`), narrowed by one label filter
//! when the [`tinymemory_api::MetaFilter`] has a labelled field. The events
//! are decoded back to items, the full filter is applied, repeats of an item
//! are dropped keeping its best rank, and the kinds are interleaved rank by
//! rank. CortexDB reports no per-hit score, so the score is the rank's,
//! `1 / (1 + rank)`. A conversation hit carries the whole conversation's
//! text, assembled from all its turns.
//!
//! **Cursor.** Recall is a ranking, not a log, so it has no cursor of its
//! own. The fetch cursor is an offset into the merged ranking; the next page
//! asks again with a budget large enough to reach past it. A page ends the
//! ranking (`next_cursor: None`) when no hit beyond it was found.

use std::collections::HashSet;

use serde_json::{Value, json};
use tinymemory_api::{FetchMode, FetchPage, FetchRequest, Hit, ItemKind, MetaFilter};

use super::CortexEngine;
use super::cursor::{self, FetchCursor};
use super::items::{admitted, hit, keeps};
use crate::envelope::{Envelope, decode_event, labels, rebuild, scope_of};
use crate::error::Result;

/// The cursor tag of a fetch.
const TAG: char = 'f';

/// Events one recall pack may hold. Bounds how deep fetch pages can go.
const MAX_PACK_EVENTS: usize = 1000;

/// Raw events asked for per wanted hit: a conversation contributes several
/// turns, and the client-side filter drops some.
const EVENTS_PER_HIT: usize = 3;

/// A recall body for `query` over `scope`, narrowed by `filter`'s label.
pub(super) fn recall_body(scope: &str, query: &str, events: usize, filter: &MetaFilter) -> Value {
    let mut body = json!({
        "scope": scope,
        "query": query,
        "budgets": { "per_layer_limits": { "events": events } },
    });
    if let Some(labels) = labels::narrowing(filter) {
        body["filters"] = json!({ "metadata": { "labels": labels } });
    }
    body
}

/// The distinct items of `kind` a pack's events decode to, best rank first,
/// keeping only what `filter` matches.
pub(super) fn ranked(pack: &Value, kind: Option<ItemKind>, filter: &MetaFilter) -> Vec<Envelope> {
    let mut seen = HashSet::new();
    pack.pointer("/layers/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(decode_event)
        .map(|decoded| decoded.envelope)
        .filter(|envelope| keeps(filter, kind.unwrap_or(envelope.kind), envelope))
        .filter(|envelope| seen.insert(envelope.id.clone()))
        .collect()
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn fetch_page(&self, req: FetchRequest) -> Result<FetchPage> {
        self.descriptor.ensure_mode(req.mode)?;
        debug_assert_eq!(req.mode, FetchMode::Hybrid);
        req.validate()?;
        let offset = match &req.cursor {
            Some(raw) => cursor::decode::<FetchCursor>(TAG, raw)?.offset,
            None => 0,
        };
        let end = offset.saturating_add(req.limit);
        let events = end
            .saturating_add(1)
            .saturating_mul(EVENTS_PER_HIT)
            .min(MAX_PACK_EVENTS);
        let mut per_kind = Vec::new();
        for kind in admitted(&req.filter) {
            let body = recall_body(scope_of(kind), &req.query, events, &req.filter);
            let pack = self.log.recall(&body).await?;
            per_kind.push(ranked(&pack, Some(kind), &req.filter));
        }
        let merged = interleave(per_kind);
        let more = merged.len() > end;
        let page: Vec<(usize, Envelope)> = merged
            .into_iter()
            .enumerate()
            .skip(offset)
            .take(req.limit)
            .collect();
        let conversations = self
            .conversations(
                &page
                    .iter()
                    .filter(|(_, e)| e.kind == ItemKind::Conversation)
                    .map(|(_, e)| e.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
        let hits: Vec<Hit> = page
            .into_iter()
            .filter_map(|(rank, envelope)| {
                let score = 1.0 / (1.0 + rank as f32);
                let item = match envelope.kind {
                    ItemKind::Conversation => conversations.get(&envelope.id)?.clone(),
                    _ => rebuild(std::slice::from_ref(&envelope))?,
                };
                Some(hit(&envelope.id, &item, score))
            })
            .collect();
        let next_cursor = if more {
            Some(cursor::encode(TAG, &FetchCursor { offset: end })?)
        } else {
            None
        };
        Ok(FetchPage { hits, next_cursor })
    }
}

/// Merges per-kind rankings rank by rank: every kind's best, then every
/// kind's second, and so on.
fn interleave(mut lists: Vec<Vec<Envelope>>) -> Vec<Envelope> {
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    let mut iters: Vec<_> = lists.iter_mut().map(|list| list.drain(..)).collect();
    let mut out = Vec::new();
    for _ in 0..longest {
        for iter in &mut iters {
            if let Some(envelope) = iter.next() {
                out.push(envelope);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
