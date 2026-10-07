//! Date hints on recall: [`tinymemory_api::TimeHint`] as CortexDB's
//! `temporal.refers_during` (capability `refers_to_v1`).
//!
//! `refers_during` is boost-only on the server: it moves the candidate events
//! from the hinted days ahead of the rest and drops nothing (API reference
//! §12.2). It is never `natural` or `valid_during`, which filter by capture
//! time and lose a memory recorded on one day about another.
//!
//! **Gate.** A direct engine asks `v1/admin/version` once, and sends the hint
//! only when the server lists `refers_to_v1` (or when the route cannot be
//! read). The hosted wire has no version route, so it sends optimistically. Either way, a hinted read refused as
//! invalid is retried once without the hint; when that succeeds the server
//! is marked as not taking hints for this engine's lifetime. A refusal that
//! the retry repeats was not about the hint, so it is returned unchanged and
//! the mark is left alone. A hint can cost one extra request, never a failed
//! fetch.

use std::sync::atomic::{AtomicU8, Ordering};

use reqwest::Method;
use serde_json::{Value, json};
use tinymemory_api::TimeHint;

use super::CortexEngine;
use crate::cortex::descriptor::{CortexWire, Route};
use crate::cortex::error::Error;
use crate::cortex::transport::Attempts;

/// The capability CortexDB lists for `temporal.refers_during`.
const CAPABILITY: &str = "refers_to_v1";

const UNKNOWN: u8 = 0;
const YES: u8 = 1;
const NO: u8 = 2;

/// Whether this engine's server takes `refers_during`, shared by clones:
/// `UNKNOWN` (the default) until known, then `YES` or `NO`.
#[derive(Debug, Default)]
pub(crate) struct RefersSupport(AtomicU8);

/// The `temporal` block for `hint`.
pub(super) fn temporal(hint: &TimeHint) -> Value {
    let mut block = json!({
        "refers_during": { "from": hint.from.to_string(), "to": hint.to.to_string() },
    });
    if let Some(zone) = &hint.zone {
        block["timezone"] = json!(zone);
    }
    block
}

impl CortexEngine {
    /// Whether to send a date hint now (see the module docs).
    pub(super) async fn sends_refers(&self) -> bool {
        match self.refers.0.load(Ordering::Relaxed) {
            YES => return true,
            NO => return false,
            _ => {}
        }
        if self.log.client.wire() != CortexWire::Direct {
            return true;
        }
        let version = self
            .log
            .client
            .json(
                Method::GET,
                self.log.client.wire().path(Route::Version),
                None,
                Attempts::Once,
            )
            .await;
        // An unreadable version route is treated like the hosted wire: send,
        // and let a refusal settle it. Either way the probe runs once.
        let listed = version.map_or(true, |version| {
            version
                .get("capabilities")
                .and_then(Value::as_array)
                .is_some_and(|caps| caps.iter().any(|cap| cap == CAPABILITY))
        });
        log::debug!("[cortex] {CAPABILITY} listed={listed}");
        self.record_probe(listed)
    }

    /// Takes a probe's answer only while the state is unknown, so a refusal
    /// recorded while the probe was in flight is never overwritten with YES.
    /// Returns whether hints are sent.
    fn record_probe(&self, listed: bool) -> bool {
        let _ = self.refers.0.compare_exchange(
            UNKNOWN,
            if listed { YES } else { NO },
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.refers.0.load(Ordering::Relaxed) == YES
    }

    /// Records that the server refused a date hint with `error`.
    pub(super) fn refers_refused(&self, error: &Error) {
        if self.refers.0.swap(NO, Ordering::Relaxed) != NO {
            log::warn!("[cortex] date hint refused, recalling without it from now on: {error}");
        }
    }
}

#[cfg(test)]
#[path = "refers_tests.rs"]
mod tests;
