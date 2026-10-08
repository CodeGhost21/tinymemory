//! Explore: facet counts read from each item's first event.
//!
//! The contract's default ([`tinymemory_api::explore`]'s listing walk) pages
//! through `list`, which assembles every conversation and chunked document
//! whole (one labelled walk per page and kind) only for explore to read its
//! kind and metadata. Every event carries the item's full metadata (see
//! `envelope::Envelope::meta`), and an item is counted from the event that
//! starts it (turn 0, piece 0, or its only event), so nothing is assembled.
//!
//! The scopes the filter reads are walked [`SCOPES_AT_ONCE`] at a time and
//! merged in listing order, so the same items are counted, deduplicated by
//! id, and cut at [`ExploreRequest::scan_limit`] as the listing walk would.
//! One difference: a chunked document missing a piece (a store that failed
//! part-way) is counted, where `list` leaves it out until it is whole.
//!
//! A scope whose walk reaches the page cap stops there and marks the page
//! [`ExplorePage::truncated`], rather than refusing the whole explore.

use std::collections::{BTreeMap, HashSet};

use futures::{StreamExt, TryStreamExt, stream};
use serde_json::Value;
use tinymemory_api::{
    ExplorePage, ExploreRequest, ItemKind, MemoryMeta, MetaFilter, explore_page_of,
};

use super::CortexEngine;
use super::items::keeps;
use super::scopes::KindScope;
use crate::cortex::envelope::{decode_event, labels};
use crate::cortex::error::Result;
use crate::cortex::log::{MAX_PAGES, PAGE_SIZE};

/// Scopes walked concurrently by one explore.
const SCOPES_AT_ONCE: usize = 4;

/// The items one scope's events start, newest first.
struct ScopeTally {
    /// Each item's id, kind and metadata, once per id.
    starts: Vec<(String, ItemKind, MemoryMeta)>,
    /// Whether the walk stopped (at the scan limit or the page cap) before
    /// the scope's end.
    truncated: bool,
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn explore_items(&self, req: ExploreRequest) -> Result<ExplorePage> {
        req.validate()?;
        let scopes = self.scopes_for(&req.filter).await?;
        let narrowing = labels::narrowing(&req.filter);
        log::debug!(
            "[cortex] explore facet={} scopes={} scan_limit={}",
            req.facet.as_str(),
            scopes.len(),
            req.scan_limit
        );
        let tallies: Vec<ScopeTally> = stream::iter(scopes.iter())
            .map(|scope| {
                self.tally_scope(scope, &req.filter, narrowing.as_deref(), req.scan_limit)
            })
            .buffered(SCOPES_AT_ONCE)
            .try_collect()
            .await?;

        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut total = 0_u64;
        let mut missing = 0_u64;
        let mut read = 0_usize;
        let mut truncated = false;
        'scopes: for tally in tallies {
            truncated |= tally.truncated;
            for (id, kind, meta) in tally.starts {
                if !seen.insert(id) {
                    continue;
                }
                if read == req.scan_limit {
                    truncated = true;
                    break 'scopes;
                }
                read += 1;
                total += 1;
                let values = req.facet.values(kind, &meta);
                if values.is_empty() {
                    missing += 1;
                }
                for value in values {
                    *counts.entry(value).or_default() += 1;
                }
            }
        }
        log::debug!(
            "[cortex] explore facet={} total={total} buckets={} truncated={truncated}",
            req.facet.as_str(),
            counts.len()
        );
        Ok(explore_page_of(
            req.facet, counts, req.limit, total, missing, truncated,
        ))
    }

    /// The items `scope`'s events start that `filter` keeps, at most `cap`.
    async fn tally_scope(
        &self,
        scope: &KindScope,
        filter: &MetaFilter,
        narrowing: Option<&[String]>,
        cap: usize,
    ) -> Result<ScopeTally> {
        let mut starts = Vec::new();
        let mut seen = HashSet::new();
        let mut last: Option<String> = None;
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self
                .log
                .page(&scope.path, narrowing, cursor.as_deref(), PAGE_SIZE)
                .await?;
            for event in &page.items {
                // The engine emits each event twice in a row.
                let id = event.get("id").and_then(Value::as_str);
                if id.is_some() && id == last.as_deref() {
                    continue;
                }
                last = id.map(str::to_owned);
                let Some(decoded) = decode_event(event) else {
                    continue;
                };
                let envelope = decoded.envelope;
                if !keeps(filter, scope.kind, &envelope)
                    || !envelope.part().is_none_or(|index| index == 0)
                    || !seen.insert(envelope.id.clone())
                {
                    continue;
                }
                if starts.len() == cap {
                    return Ok(ScopeTally {
                        starts,
                        truncated: true,
                    });
                }
                starts.push((envelope.id, scope.kind, envelope.meta));
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => {
                    return Ok(ScopeTally {
                        starts,
                        truncated: false,
                    });
                }
            }
        }
        log::warn!(
            "[cortex] explore read {MAX_PAGES} pages of {} without reaching its end; \
             counting what was read",
            scope.path
        );
        Ok(ScopeTally {
            starts,
            truncated: true,
        })
    }
}

#[cfg(test)]
#[path = "explore_tests.rs"]
mod tests;
