//! Store: replay detection, then the items' missing events, then the wait.
//!
//! `store` is `store_many` of one item: there is one path, so a single store
//! gets exactly the batch's guarantees (listed on return, and ranked recall
//! awaited for its final event).
//!
//! An item id is the item's fingerprint, so the engine first looks up the
//! events already carrying that id's label in the item's scope (its kind at
//! its namespace node):
//!
//! - every event present (the whole document, learning, or every turn) —
//!   a replay: nothing is written and the receipt says so;
//! - some turns of a conversation, or some pieces of a chunked document,
//!   present — a previous store failed part way, and only the missing ones
//!   are written, in order;
//! - nothing present — every event is written.
//!
//! Each event is keyed by its own body (`transport::body_idempotency_key`),
//! so an identical retry is a replay CortexDB answers without writing
//! (`replayed_from_idempotency`), and forgetting an item releases its keys.
//! A key lasts 24 hours and changes with any byte of the body, so it does
//! not replace the lookup: an unchanged file synced again a day later, or
//! with a new `observed_at`, would be written again without it. The lookup
//! is skipped only on the turn-logging hot path ([`looks_up`]): a Direct,
//! accepted-only store of one single-turn conversation, which the agent
//! lifecycle makes twice per turn, and where a retry is the replay to catch.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use tinymemory_api::{ItemId, StoreItem, StoreReceipt, WaitFor, validate_many};

use super::CortexEngine;
use super::scopes::KindScope;
use crate::cortex::descriptor::CortexWire;
use crate::cortex::envelope::{Encoded, Envelope};
use crate::cortex::error::{Error, Result};
use crate::cortex::log::Written;

impl CortexEngine {
    /// `store_many`, paying per batch rather than per item:
    ///
    /// - one id lookup per scope (kind and namespace) finds what the batch
    ///   already holds;
    /// - every missing event is written, in item order, without waiting;
    /// - then one listing wait per scope, for the last event written there
    ///   (the scope's log is ordered, so it being listed implies the earlier
    ///   ones are), and ranked recall for the batch's final event only.
    ///
    /// An item repeated inside the batch is a replay of its first copy.
    ///
    /// With [`WaitFor::Accepted`] the writes ask for no indexing and the
    /// waits are skipped: the call returns once CortexDB captured every event.
    pub(super) async fn store_items(
        &self,
        items: Vec<StoreItem>,
        wait: WaitFor,
    ) -> Result<Vec<StoreReceipt>> {
        validate_many(&items)?;
        if let Some(item) = items
            .iter()
            .find(|item| self.layout.repeats_root(&item.meta().namespace))
        {
            return Err(Error::InvalidRequest(format!(
                "namespace `{}` repeats the scope root `{}`; nodes go below the root",
                item.meta().namespace,
                self.layout.root()
            )));
        }
        self.register_root().await;
        let ids: Vec<String> = items.iter().map(StoreItem::fingerprint).collect();
        // Every event of the batch is laid out and size-checked before any is
        // sent, so an item CortexDB would refuse leaves nothing half-written.
        let mut planned: Vec<Vec<(Option<u32>, Envelope, Encoded)>> =
            Vec::with_capacity(items.len());
        for (item, id) in items.iter().zip(&ids) {
            let mut events = Vec::new();
            for envelope in Envelope::for_item(item, id)? {
                let encoded = envelope.encode_checked()?;
                events.push((envelope.part(), envelope, encoded));
            }
            planned.push(events);
        }
        let mut held: HashMap<String, HashSet<Option<u32>>> = HashMap::new();
        let mut by_scope: BTreeMap<KindScope, Vec<String>> = BTreeMap::new();
        for (item, id) in items.iter().zip(&ids) {
            by_scope
                .entry(KindScope::new(
                    &self.layout,
                    item.meta().namespace.clone(),
                    item.kind(),
                ))
                .or_default()
                .push(id.clone());
        }
        let lookup = looks_up(self.wire(), &items, wait);
        let looking = Instant::now();
        for (scope, of_scope) in by_scope.iter().filter(|_| lookup) {
            for (id, events) in self.item_events(scope, of_scope).await? {
                held.entry(id)
                    .or_default()
                    .extend(events.iter().map(|decoded| decoded.envelope.part()));
            }
        }
        let lookup_ms = looking.elapsed().as_millis();
        let looked_up = if lookup { by_scope.len() } else { 0 };
        let writing = Instant::now();
        let mut sent = 0;
        let mut receipts = Vec::with_capacity(items.len());
        let mut written_here: HashSet<String> = HashSet::new();
        let mut last_per_scope: Vec<Written> = Vec::new();
        for (events, id) in planned.into_iter().zip(ids) {
            let present = held.get(&id);
            let mut requests = Vec::new();
            if !written_here.contains(&id) {
                for (part, envelope, encoded) in events {
                    if present.is_some_and(|present| present.contains(&part)) {
                        continue;
                    }
                    requests.push(envelope.request(&encoded, &self.layout));
                }
            }
            let mut replayed = requests.is_empty();
            sent += requests.len();
            if let Some(written) = self.log.write(&requests, wait).await? {
                replayed = written.replayed;
                last_per_scope.retain(|w| w.scope != written.scope);
                last_per_scope.push(written);
            }
            written_here.insert(id.clone());
            receipts.push(StoreReceipt {
                id: ItemId::new(id),
                replayed,
            });
        }
        let writes_ms = writing.elapsed().as_millis();
        let stored = receipts.len();
        log::debug!(
            "[cortex] store of {stored} items: lookup {lookup_ms} ms over {looked_up} scopes, \
             {sent} events written in {writes_ms} ms ({wait:?})"
        );
        if wait == WaitFor::Accepted {
            return Ok(receipts);
        }
        let waiting = Instant::now();
        let final_index = last_per_scope.len().saturating_sub(1);
        for (index, written) in last_per_scope.iter().enumerate() {
            self.log
                .await_written(written, index == final_index)
                .await?;
        }
        let await_ms = waiting.elapsed().as_millis();
        let awaited = last_per_scope.len();
        log::debug!("[cortex] store of {stored} items: waited {await_ms} ms for {awaited} scopes");
        Ok(receipts)
    }
}

/// Whether a store looks its items up before writing. Always, except on the
/// turn-logging hot path: a Direct store of one single-turn conversation
/// that waits only for acceptance. There the body key catches a retry
/// (Direct answers it as a replay), and a listing per turn would cost the
/// turn latency. The hosted wire always looks up: its answer need not say
/// it replayed.
fn looks_up(wire: CortexWire, items: &[StoreItem], wait: WaitFor) -> bool {
    let hot = matches!(
        items,
        [StoreItem::Conversation { turns, .. }] if turns.len() == 1
    );
    !(wire == CortexWire::Direct && wait == WaitFor::Accepted && hot)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
