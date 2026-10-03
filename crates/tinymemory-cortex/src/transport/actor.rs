//! The `X-Cortex-Actor` header a direct CortexDB expects beside the bearer.
//!
//! CortexDB serves every request as an *actor*. A minted token (the hosted
//! CortexDB cloud signs one per account) is only accepted when the request
//! also names its subject in `X-Cortex-Actor`; without it the server answers
//! `401 ACTOR_MISMATCH`. A static operator key is served as `user:local` and
//! accepts the header too. The actor is whatever `GET v1/auth/whoami` reports
//! as `caller`, so a client learns it there once and sends it on every call,
//! the same flow CortexDB's own console uses.
//!
//! [`ActorCache`] holds what was learned, shared by the clones of one client:
//!
//! - **Known**: `whoami` answered; the caller is sent on every request.
//! - **Absent**: the server has no `whoami` route (404/405, servers before
//!   the actor model); no header is sent and `whoami` is not asked again.
//! - **Unknown**: nothing learned yet, or a credential was just rejected; the
//!   next request asks `whoami` again. A failed lookup is not cached: the
//!   request goes out without the header and reports its own failure.
//!
//! Only the direct wire uses this; the TinyHumans backend names the actor
//! itself.

use std::sync::{Arc, Mutex, PoisonError};

use reqwest::StatusCode;
use reqwest::header::HeaderValue;
use serde_json::Value;

/// The header name.
pub(crate) const ACTOR_HEADER: &str = "X-Cortex-Actor";

/// The route that reports the presented token's actor.
pub(crate) const WHOAMI_PATH: &str = "v1/auth/whoami";

#[derive(Clone, Debug, Default)]
enum Learned {
    #[default]
    Unknown,
    Known(HeaderValue),
    Absent,
}

/// What this client has learned about its actor; clones share it.
#[derive(Clone, Debug, Default)]
pub(crate) struct ActorCache(Arc<Mutex<Learned>>);

/// What [`ActorCache::lookup`] found.
#[derive(Debug, PartialEq)]
pub(crate) enum Lookup {
    /// Send this header value.
    Send(HeaderValue),
    /// Send no header.
    Skip,
    /// Nothing learned yet: ask `whoami`.
    Ask,
}

impl ActorCache {
    pub(crate) fn lookup(&self) -> Lookup {
        match &*self.0.lock().unwrap_or_else(PoisonError::into_inner) {
            Learned::Known(value) => Lookup::Send(value.clone()),
            Learned::Absent => Lookup::Skip,
            Learned::Unknown => Lookup::Ask,
        }
    }

    /// Records a `whoami` answer and returns the header to send, if any.
    pub(crate) fn learn(&self, status: StatusCode, body: &[u8]) -> Option<HeaderValue> {
        let learned = if status.is_success() {
            match caller_header(body) {
                Some(value) => Learned::Known(value),
                None => Learned::Absent,
            }
        } else if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            Learned::Absent
        } else {
            // Expired or wrong key, or the server is struggling: the request
            // itself will say so, and the next one asks again.
            return None;
        };
        log_learned(&learned);
        let header = match &learned {
            Learned::Known(value) => Some(value.clone()),
            _ => None,
        };
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = learned;
        header
    }

    /// Forgets the actor after a rejected credential, so a replaced key is
    /// looked up again.
    pub(crate) fn forget(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Learned::Unknown;
    }
}

/// `caller` from a `whoami` body, as a header value.
fn caller_header(body: &[u8]) -> Option<HeaderValue> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let caller = value.get("caller")?.as_str()?.trim();
    if caller.is_empty() {
        return None;
    }
    HeaderValue::from_str(caller).ok()
}

fn log_learned(learned: &Learned) {
    match learned {
        // The actor is an account id, not a secret, but keep it out of logs.
        Learned::Known(_) => log::debug!("[tinymemory-cortex] actor learned from whoami"),
        Learned::Absent => log::debug!("[tinymemory-cortex] server reports no actor; none sent"),
        Learned::Unknown => {}
    }
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
