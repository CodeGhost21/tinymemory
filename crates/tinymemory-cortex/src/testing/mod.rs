//! Loopback HTTP doubles of CortexDB (`/v1/*`) and of the TinyHumans
//! backend (`/memory/*`), shared by every test in the crate.
//!
//! Both serve the same [`CortexLog`]. The hosted double additionally wraps
//! bodies in `{success,data}`, reports failures with `errorCode`, refuses a
//! scope outside the memory API's grammar, takes an `Idempotency-Key` claim
//! per write (any replay of a claimed key is a 409, never forwarded),
//! refuses a repeated `labels=` parameter, and enforces the strict answer
//! schema. Knobs make either fail the ways the real stacks fail.

mod log;
mod routes;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;

pub(crate) use log::CortexLog;

use crate::{CortexCredential, CortexEngine, StaticBearer};

/// The bearer the test engines send.
pub(crate) const TEST_TOKEN: &str = "tiny_live_test";

/// What the double saw.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// `"METHOD /path?query"` of every request, in order.
    pub(crate) requests: Vec<String>,
    /// The `Authorization` header of every request.
    pub(crate) auth: Vec<String>,
    /// `(body idempotency_key, Idempotency-Key header)` of every write.
    pub(crate) idempotency: Vec<(Option<String>, Option<String>)>,
    /// Every recall body.
    pub(crate) recalls: Vec<serde_json::Value>,
    /// Every answer body.
    pub(crate) answers: Vec<serde_json::Value>,
    /// Every forget body.
    pub(crate) forgets: Vec<serde_json::Value>,
}

/// One double's state and knobs.
#[derive(Debug, Default)]
pub(crate) struct Double {
    /// Whether this is the TinyHumans double.
    pub(crate) hosted: bool,
    pub(crate) log: Mutex<CortexLog>,
    pub(crate) seen: Mutex<Seen>,
    /// When set, every request fails with this status and code.
    pub(crate) fail_all: Mutex<Option<(u16, &'static str)>>,
    /// The only token accepted; `None` accepts any non-empty bearer.
    pub(crate) accept_token: Mutex<Option<String>>,
    /// Claimed `Idempotency-Key`s (hosted).
    pub(crate) claimed: Mutex<HashSet<String>>,
    /// Listings come back empty for this many requests.
    pub(crate) hide_listing_for: AtomicUsize,
    /// Listings answer 429 for this many requests.
    pub(crate) rate_limit_events: AtomicUsize,
    /// Writes answer the backend's own 429 (before any claim) this many
    /// times.
    pub(crate) rate_limit_experience: AtomicUsize,
    /// Writes are applied, then answered 503, this many times.
    pub(crate) apply_then_fail: AtomicUsize,
    /// Writes have their claim taken, are not applied, and answer 502.
    pub(crate) claim_then_fail: AtomicUsize,
    /// The Nth write (1-based) is refused with 400, unapplied.
    pub(crate) fail_nth_experience: AtomicUsize,
    pub(crate) experience_calls: AtomicUsize,
    /// Forgets answer 429 this many times.
    pub(crate) rate_limit_forget: AtomicUsize,
    /// Recall answers 500.
    pub(crate) recall_down: AtomicBool,
}

/// The shared handle the routes and tests hold.
pub(crate) type Shared = Arc<Double>;

/// Decrements `counter` if positive; whether a unit was taken.
pub(crate) fn take_one(counter: &AtomicUsize) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
}

impl Double {
    /// How many recorded requests start with `prefix`.
    pub(crate) fn count(&self, prefix: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.starts_with(prefix))
            .count()
    }

    /// Every recorded request.
    pub(crate) fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().requests.clone()
    }

    /// How many events the log holds.
    pub(crate) fn event_count(&self) -> usize {
        self.log.lock().unwrap().events.len()
    }
}

/// Serves `app` on an ephemeral loopback port and returns its base URL.
pub(crate) async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    endpoint
}

/// A running CortexDB double.
pub(crate) async fn direct_double() -> (String, Shared) {
    let state = Arc::new(Double::default());
    (serve(routes::direct(state.clone())).await, state)
}

/// A running TinyHumans double.
pub(crate) async fn hosted_double() -> (String, Shared) {
    let state = Arc::new(Double {
        hosted: true,
        ..Double::default()
    });
    (serve(routes::hosted(state.clone())).await, state)
}

/// The visibility budget test engines wait.
pub(crate) const TEST_VISIBILITY: Duration = Duration::from_secs(2);

/// A Direct engine on `endpoint` with fast test timing.
pub(crate) fn direct_engine(endpoint: &str) -> CortexEngine {
    CortexEngine::direct(endpoint, CortexCredential::api_key(TEST_TOKEN))
        .unwrap()
        .with_test_timing(TEST_VISIBILITY)
}

/// A TinyHumans engine on `endpoint` with fast test timing.
pub(crate) fn hosted_engine(endpoint: &str) -> CortexEngine {
    CortexEngine::tinyhumans(endpoint, Arc::new(StaticBearer::new(TEST_TOKEN)))
        .unwrap()
        .with_test_timing(TEST_VISIBILITY)
}
