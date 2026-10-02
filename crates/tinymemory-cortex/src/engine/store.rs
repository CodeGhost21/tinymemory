//! Store: replay detection, then the item's missing events, then the wait.
//!
//! The item id is the item's fingerprint, so the engine first looks up the
//! events already carrying that id's label in the kind's scope:
//!
//! - every event present (the whole document, learning, or every turn) —
//!   a replay: nothing is written and the receipt says so;
//! - some turns of a conversation present — a previous store failed part
//!   way, and only the missing turns are written, in order;
//! - nothing present — every event is written.
//!
//! Writes use fresh idempotency keys (see `transport::fresh_idempotency_key`)
//! rather than ones derived from content: CortexDB never releases a key on
//! forget, so a content key would make re-storing a forgotten item a silent
//! no-op.

use std::collections::HashSet;

use tinymemory_api::{ItemId, StoreItem, StoreReceipt};

use super::CortexEngine;
use crate::envelope::Envelope;
use crate::error::Result;

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn store_item(&self, item: StoreItem) -> Result<StoreReceipt> {
        item.validate()?;
        let id = item.fingerprint();
        let envelopes = Envelope::for_item(&item, &id)?;
        let held = self
            .item_events(item.kind(), std::slice::from_ref(&id))
            .await?
            .remove(&id)
            .unwrap_or_default();
        let present: HashSet<Option<u32>> = held
            .iter()
            .map(|decoded| decoded.envelope.turn.as_ref().map(|turn| turn.index))
            .collect();
        let mut requests = Vec::new();
        for envelope in &envelopes {
            if present.contains(&envelope.turn.as_ref().map(|turn| turn.index)) {
                continue;
            }
            requests.push(envelope.request(&envelope.encode()?));
        }
        let replayed = requests.is_empty();
        if !replayed {
            self.log.append(&requests).await?;
        }
        Ok(StoreReceipt {
            id: ItemId::new(id),
            replayed,
        })
    }
}
