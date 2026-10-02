//! Forget: find the items' events, then remove them by `memory_ids`.
//!
//! - **By id**: each kind scope is searched for the ids' labels (one listing
//!   per batch of ids), and every event of a found item is removed.
//! - **By filter**: the filter must not be empty. Every admitted kind scope
//!   is walked (narrowed by the filter's label when it has one) and the full
//!   filter decides which items match; their events are then removed.
//!
//! An empty selector is never sent: a scope with nothing to remove sends no
//! request at all (see `log::forget`). `forgotten` counts items, not events.

use std::collections::{HashMap, HashSet};

use tinymemory_api::{ForgetReport, ForgetTarget, ItemKind, MetaFilter};

use super::CortexEngine;
use super::items::{admitted, keeps};
use crate::envelope::{decode_event, labels, scope_of};
use crate::error::Result;

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn forget_items(&self, target: ForgetTarget) -> Result<ForgetReport> {
        target.validate()?;
        let mut forgotten = HashSet::new();
        match target {
            ForgetTarget::Ids(ids) => {
                let ids: Vec<String> = ids
                    .into_iter()
                    .map(|id| id.0)
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                for kind in ItemKind::ALL {
                    let grouped = self.item_events(kind, &ids).await?;
                    let mut events = Vec::new();
                    for (id, decoded) in grouped {
                        events.extend(decoded.into_iter().map(|d| d.event_id));
                        forgotten.insert(id);
                    }
                    self.log.forget_events(scope_of(kind), &events).await?;
                }
            }
            ForgetTarget::Filter(filter) => {
                for kind in admitted(&filter) {
                    let matched = self.matching_events(kind, &filter).await?;
                    let mut events = Vec::new();
                    for (id, ids) in matched {
                        events.extend(ids);
                        forgotten.insert(id);
                    }
                    self.log.forget_events(scope_of(kind), &events).await?;
                }
            }
        }
        Ok(ForgetReport {
            forgotten: forgotten.len(),
        })
    }

    /// Every event of every item of `kind` that `filter` matches, grouped by
    /// item id.
    async fn matching_events(
        &self,
        kind: ItemKind,
        filter: &MetaFilter,
    ) -> Result<HashMap<String, Vec<String>>> {
        let narrowing = labels::narrowing(filter);
        let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
        for event in self.log.walk(scope_of(kind), narrowing.as_deref()).await? {
            let Some(decoded) = decode_event(&event) else {
                continue;
            };
            if keeps(filter, kind, &decoded.envelope) {
                grouped
                    .entry(decoded.envelope.id)
                    .or_default()
                    .push(decoded.event_id);
            }
        }
        Ok(grouped)
    }
}
