//! Waiting for an accepted write to become readable.
//!
//! The contract requires read-after-write, and CortexDB indexes after it
//! accepts, so a write waits twice:
//!
//! 1. **Listed** — until the scope listing (narrowed to the item's label)
//!    carries the event. `list`, `forget` and replay detection read the
//!    listing, so a write that never appears there has not happened as far
//!    as the contract is concerned: timing out is an error.
//! 2. **Settled** — until ranked recall returns the event. Recall lags the
//!    listing by about a second. This wait is best-effort: the record is
//!    durable and listed, so a recall index that has not caught up (or
//!    cannot be reached) ends the wait quietly rather than failing a
//!    successful write.
//!
//! On the TinyHumans wire a 429 or 5xx while waiting means "not yet", not
//! "the write failed": the write was accepted and is durable, so the wait
//! continues to its deadline, backing off to a 2s ceiling.

use serde_json::{Value, json};

use super::{Log, PAGE_SIZE};
use crate::cortex::descriptor::CortexWire;
use crate::cortex::error::{Error, Result};

/// Longest query the settle probe sends: a distinctive prefix of the stored
/// text matches better, and costs less, than a 64 KiB document.
const SETTLE_QUERY_CHARS: usize = 256;

/// Whether `items` (a listing page or a pack's events) holds `event_id`.
fn carries(items: Option<&Value>, event_id: &str) -> bool {
    items.and_then(Value::as_array).is_some_and(|items| {
        items
            .iter()
            .any(|e| e.get("id").and_then(Value::as_str) == Some(event_id))
    })
}

impl Log {
    /// Both waits for one event: listed (fatal on timeout), then settled
    /// (best-effort).
    pub(crate) async fn await_readable(
        &self,
        scope: &str,
        label: &str,
        event_id: &str,
        text: &str,
    ) -> Result<()> {
        self.await_listed(scope, label, event_id).await?;
        self.await_settled(scope, event_id, text).await;
        Ok(())
    }

    /// Blocks until the listing of `scope` narrowed to `label` carries
    /// `event_id`; fails once the visibility budget has passed.
    pub(crate) async fn await_listed(
        &self,
        scope: &str,
        label: &str,
        event_id: &str,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.timing.visibility;
        let mut delay = self.timing.poll;
        let labels = [label.to_string()];
        // What the polls saw, so a timeout says whether the event was missing
        // from the listing or the listing could not be read (rate limited).
        let mut without = 0;
        let mut transient = 0;
        let mut last_fault = String::new();
        loop {
            // Newest first, so one page is enough to see a write just made.
            match self.page(scope, Some(&labels), None, PAGE_SIZE).await {
                Ok(page)
                    if page
                        .items
                        .iter()
                        .any(|e| e.get("id").and_then(Value::as_str) == Some(event_id)) =>
                {
                    return Ok(());
                }
                Ok(_) => without += 1,
                Err(error)
                    if self.client.wire() == CortexWire::TinyHumans && error.is_transient() =>
                {
                    transient += 1;
                    last_fault = error.to_string();
                }
                Err(error) => return Err(error),
            }
            if tokio::time::Instant::now() >= deadline {
                let fault = if last_fault.is_empty() {
                    String::new()
                } else {
                    format!(", the last: {last_fault}")
                };
                return Err(Error::Unavailable(format!(
                    "event `{event_id}` was accepted into scope `{scope}` but did not become \
                     readable within {:?} ({without} polls answered without it, {transient} \
                     failed transiently{fault}); reporting the write as done would break \
                     read-after-write",
                    self.timing.visibility
                )));
            }
            tokio::time::sleep(delay).await;
            delay = self.next_poll(delay);
        }
    }

    /// Waits, best-effort, until ranked recall returns `event_id`.
    async fn await_settled(&self, scope: &str, event_id: &str, text: &str) {
        let query: String = text.chars().take(SETTLE_QUERY_CHARS).collect();
        let body = json!({
            "scope": scope,
            "query": query,
            "view": "granular",
            "include": ["events"],
        });
        let deadline = tokio::time::Instant::now() + self.timing.settle;
        let mut delay = self.timing.poll;
        while tokio::time::Instant::now() < deadline {
            let Ok(pack) = self.recall(&body).await else {
                // A probe that cannot be answered says nothing about the
                // write, which is durable and listed.
                return;
            };
            if carries(pack.pointer("/layers/events"), event_id) {
                return;
            }
            tokio::time::sleep(delay).await;
            delay = self.next_poll(delay);
        }
    }
}
