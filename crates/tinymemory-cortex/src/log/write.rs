//! Appending an item's events.
//!
//! **Direct** sends one experience (`v1/experience?wait=indexed`) or, for a
//! conversation, one ordered batch (`v1/experience/bulk?wait=indexed` with
//! `ordering: strict_temporal`), once: a timeout on a write leaves whether it
//! applied unknown, and the store's replay check makes a retry by the caller
//! safe.
//!
//! **TinyHumans** has no bulk route, so a conversation is written one event
//! at a time, in order. Each write carries a random `Idempotency-Key` claim,
//! reused across that write's own retries. The memory API takes the claim
//! before forwarding and keeps it once the engine has been contacted, and
//! answers any replay of a claimed key with 409 without forwarding it. So:
//!
//! - a transient fault (429, 5xx, timeout) is retried under the same claim;
//!   a fault raised before the memory API (the backend's own rate limiter)
//!   left the claim free, and the retry is simply forwarded;
//! - a 409 on a *retry* means the earlier attempt reached the engine and may
//!   have been applied: the outcome is unknown rather than failed, and the
//!   event is looked for (same scope, same item label, same stored text)
//!   until the visibility budget runs out.
//!
//! The stored text names the item id and, for a turn, its index, so finding
//! an event with exactly that text is proof this write (or an identical
//! earlier one) landed.
//!
//! Either way the write then waits for its last event to be readable.

use serde_json::{Value, json};

use super::{Log, PAGE_SIZE};
use crate::descriptor::{CortexWire, Route};
use crate::error::{Error, Result};
use crate::transport::{Attempts, fresh_idempotency_key};

/// The last event of a write: what a wait for it needs.
#[derive(Debug, Clone)]
pub(crate) struct Written {
    pub(crate) scope: String,
    pub(crate) label: String,
    pub(crate) text: String,
    pub(crate) event_id: String,
}

/// How many times a hosted write is sent before a transient fault surfaces.
const HOSTED_WRITE_ATTEMPTS: u32 = 3;

/// The scope, stored text and item label of an experience request.
fn parts(request: &Value) -> Result<(&str, &str, &str)> {
    let scope = request.get("scope").and_then(Value::as_str);
    let text = request.pointer("/content/text").and_then(Value::as_str);
    let label = request.pointer("/context/labels/0").and_then(Value::as_str);
    match (scope, text, label) {
        (Some(scope), Some(text), Some(label)) => Ok((scope, text, label)),
        _ => Err(Error::Engine(
            "an experience request lacks its scope, text or item label".to_string(),
        )),
    }
}

/// The event id a write receipt names.
fn receipt(answer: &Value) -> Result<String> {
    answer
        .get("event_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Engine("CortexDB accepted a write but omitted event_id".to_string()))
}

impl Log {
    /// Appends `requests` (one item's events, in order) and waits until the
    /// last is readable. The log is ordered, so the last event being listed
    /// implies the earlier ones are: one wait, not one per event. A bulk
    /// store uses [`Log::write`] and [`Log::await_written`] instead, to wait
    /// once per scope for a whole batch.
    pub(crate) async fn append(&self, requests: &[Value]) -> Result<()> {
        match self.write(requests).await? {
            Some(written) => self.await_written(&written, true).await,
            None => Ok(()),
        }
    }

    /// Writes `requests` (one item's events, in order) without waiting, and
    /// names the last event so a caller can wait for it, or for a later one
    /// in the same scope, which implies it.
    pub(crate) async fn write(&self, requests: &[Value]) -> Result<Option<Written>> {
        let Some(last) = requests.last() else {
            return Ok(None);
        };
        let event_id = match self.client.wire() {
            CortexWire::Direct => self.append_direct(requests).await?,
            CortexWire::TinyHumans => {
                let mut event_id = String::new();
                for request in requests {
                    event_id = self.send_hosted_write(request).await?;
                }
                event_id
            }
        };
        let (scope, text, label) = parts(last)?;
        Ok(Some(Written {
            scope: scope.to_string(),
            label: label.to_string(),
            text: text.to_string(),
            event_id,
        }))
    }

    /// Waits for `written` to be listed and, when `settle`, ranked.
    pub(crate) async fn await_written(&self, written: &Written, settle: bool) -> Result<()> {
        if settle {
            self.await_readable(
                &written.scope,
                &written.label,
                &written.event_id,
                &written.text,
            )
            .await
        } else {
            self.await_listed(&written.scope, &written.label, &written.event_id)
                .await
        }
    }

    /// One Direct write of one event or one ordered batch; the last event's
    /// id.
    async fn append_direct(&self, requests: &[Value]) -> Result<String> {
        let wire = self.client.wire();
        if let [single] = requests {
            let path = format!("{}?wait=indexed", wire.path(Route::Experience));
            let answer = self
                .client
                .json(reqwest::Method::POST, &path, Some(single), Attempts::Once)
                .await?;
            return receipt(&answer);
        }
        let path = format!("{}?wait=indexed", wire.path(Route::Bulk));
        let body = json!({ "items": requests, "ordering": "strict_temporal" });
        let answer = self
            .client
            .json(reqwest::Method::POST, &path, Some(&body), Attempts::Once)
            .await?;
        let results = answer
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Engine("CortexDB omitted bulk results".to_string()))?;
        if results.len() != requests.len() {
            return Err(Error::Engine(format!(
                "CortexDB returned {} bulk results for {} events",
                results.len(),
                requests.len()
            )));
        }
        results.last().map_or_else(
            || Err(Error::Engine("empty bulk results".to_string())),
            receipt,
        )
    }

    /// One hosted write under one claim, with the outcome-unknown recovery.
    async fn send_hosted_write(&self, request: &Value) -> Result<String> {
        let path = self.client.wire().path(Route::Experience);
        let claim = fresh_idempotency_key();
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.client.json_keyed(path, request, &claim).await {
                Ok(answer) => return receipt(&answer),
                Err(Error::Conflict(_)) if attempt > 1 => {
                    return self.recover_unknown_write(request).await;
                }
                Err(error) if attempt < HOSTED_WRITE_ATTEMPTS && error.is_transient() => {
                    tokio::time::sleep(self.timing.poll * 2_u32.pow(attempt - 1)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Finds the event a possibly-applied write produced, polling with the
    /// visibility budget and riding out transient faults: one 429 while
    /// looking must not turn a write that succeeded into a failure.
    async fn recover_unknown_write(&self, request: &Value) -> Result<String> {
        let (scope, text, label) = parts(request)?;
        let labels = [label.to_string()];
        let deadline = tokio::time::Instant::now() + self.timing.visibility;
        let mut delay = self.timing.poll;
        loop {
            match self.page(scope, Some(&labels), None, PAGE_SIZE).await {
                Ok(page) => {
                    let found = page.items.iter().find(|event| {
                        event.pointer("/content/text").and_then(Value::as_str) == Some(text)
                    });
                    if let Some(event) = found {
                        return receipt(&json!({ "event_id": event.get("id") }));
                    }
                }
                Err(error) if error.is_transient() => {}
                Err(error) => return Err(error),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Unavailable(format!(
                    "a retried write was refused as already claimed, but no matching event \
                     appeared in scope `{scope}` within {:?}; its outcome is unknown",
                    self.timing.visibility
                )));
            }
            tokio::time::sleep(delay).await;
            delay = self.next_poll(delay);
        }
    }
}
