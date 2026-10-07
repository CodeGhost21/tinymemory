//! Date hints on recall: [`tinymemory_api::TimeHint`] as CortexDB's
//! `temporal.refers_during` (capability `refers_to_v1`).
//!
//! `refers_during` is boost-only on the server: it moves the candidate events
//! from the hinted days ahead of the rest and drops nothing (API reference
//! §12.2). It is never `natural` or `valid_during`, which filter by capture
//! time and lose a memory recorded on one day about another.
//!
//! **Gate.** A direct engine asks `v1/admin/version` once, and sends the hint
//! only when the server lists `refers_to_v1`. The hosted wire has no version
//! route, so it sends optimistically. Either way, a hinted read refused as
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

const YES: u8 = 1;
const NO: u8 = 2;

/// Whether this engine's server takes `refers_during`, shared by clones:
/// `0` (the default) until known, then `YES` or `NO`.
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
        let Ok(version) = version else {
            // Unknown stays unknown: send, and let a refusal settle it.
            return true;
        };
        let listed = version
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|caps| caps.iter().any(|cap| cap == CAPABILITY));
        self.refers
            .0
            .store(if listed { YES } else { NO }, Ordering::Relaxed);
        log::debug!("[cortex] {CAPABILITY} listed={listed}");
        listed
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
