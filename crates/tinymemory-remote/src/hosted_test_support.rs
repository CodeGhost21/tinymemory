//! The TinyHumans backend's `/memory/*` routes, as a test double.
//!
//! The double wraps the same append-only CortexDB log the conformance suite
//! uses in the backend's `{success,data}` envelope, checks the bearer, records
//! every request, enforces the memory API's scope grammar and the strict
//! `answer` schema, and can be told to fail in the ways the real stack fails.
//! `hosted_test.rs` drives the dialect through it, and the hosted families'
//! tests drive their records through it.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::conformance_test::{
    cortex_events, cortex_experience, cortex_forget, cortex_recall, cortex_scopes, serve,
    CortexLog, CortexStore,
};
use crate::{tinyhumans_provider, StaticBearer};

/// Keys the hosted `answer` schema allows; anything else is a 400.
const ANSWER_KEYS: [&str; 10] = [
    "scope",
    "question",
    "question_type",
    "question_date",
    "temporal",
    "filters",
    "answer_max_tokens",
    "answer_instructions",
    "cite_sources",
    "include_context",
];

#[derive(Default)]
pub(crate) struct Seen {
    /// `"METHOD /path?query"` for every request, in order.
    pub(crate) requests: Vec<String>,
    /// The `Authorization` header of every request.
    pub(crate) auth: Vec<String>,
    /// `(body idempotency_key, Idempotency-Key header)` of every experience write.
    pub(crate) idempotency: Vec<(Option<String>, Option<String>)>,
}

pub(crate) struct Hosted {
    pub(crate) log: CortexStore,
    pub(crate) seen: Mutex<Seen>,
    /// Event listings hide the newest event for this many requests.
    pub(crate) hide_listing_for: AtomicUsize,
    /// Fail this many event listings with 429 before answering.
    pub(crate) rate_limit_events: AtomicUsize,
    /// When set, every request fails with this status + code.
    pub(crate) fail_all: Mutex<Option<(u16, &'static str)>>,
    /// The token the double accepts; `None` accepts any non-empty bearer.
    pub(crate) accept_token: Mutex<Option<String>>,
    /// `Idempotency-Key` values already claimed. Like the memory API, any
    /// replay of a claimed key is a 409 and is never forwarded.
    pub(crate) claimed: Mutex<HashSet<String>>,
    /// Apply the next N experience writes, then answer 503 (a transport fault
    /// after the claim was taken and the work done).
    pub(crate) apply_then_fail: AtomicUsize,
    /// Answer the Nth experience request (1-based) with a 400, unapplied.
    pub(crate) fail_nth_experience: AtomicUsize,
    pub(crate) experience_calls: AtomicUsize,
    /// Answer the next N experience writes with the backend's own 429, before
    /// the memory API ever sees them — so their claims are never taken.
    pub(crate) rate_limit_experience: AtomicUsize,
    /// Take the next N experience writes' claims, apply nothing, and answer
    /// 502: the engine refused after the memory API had claimed the key.
    pub(crate) claim_then_fail: AtomicUsize,
    /// Answer the next N forgets with the backend's own 429.
    pub(crate) rate_limit_forget: AtomicUsize,
    /// Behave like a backend that strips `limit` from `/memory/scopes`.
    pub(crate) ignore_scope_limit: AtomicBool,
    /// Event ids `GET /memory/events/{id}` answers with a null scope.
    pub(crate) foreign: Mutex<HashSet<String>>,
    /// What the derived-layer routes answer, by `(layer, scope)`: `facts`,
    /// `beliefs` or `understanding`, and the scope the request names.
    pub(crate) layers: Mutex<HashMap<(String, String), Vec<Value>>>,
}

pub(crate) type Shared = Arc<Hosted>;

fn envelope(status: StatusCode, body: Value) -> (StatusCode, Json<Value>) {
    if status.is_success() {
        (status, Json(json!({ "success": true, "data": body })))
    } else {
        let code = body
            .get("error_code")
            .and_then(Value::as_str)
            .unwrap_or("VALIDATION_ERROR");
        (
            status,
            Json(
                json!({ "success": false, "error": format!("failed: {code}"), "errorCode": code }),
            ),
        )
    }
}

/// Segments the memory API lets a caller name: it re-roots every scope under
/// the tenant (`oc:u-<id>/…`) and the engine holds 32, so one is spent.
const TENANT_SCOPE_SEGMENTS: usize = 31;

/// The memory API's scope check, as the backend reports it.
///
/// memory-api pins every `scope` and `prefix` under the caller's root and
/// refuses one that is not `type:id` segments of `[A-Za-z0-9_-]`, or that
/// would be too deep once rooted, with a 422 — which the backend turns into a
/// 400 `BAD_REQUEST`. A double that accepted any scope is how a probe with a
/// bare `zz_health` prefix passed here and failed against the real stack.
fn refuse_scope(scope: &str) -> Option<(StatusCode, Json<Value>)> {
    let segments: Vec<&str> = scope.split('/').filter(|s| !s.is_empty()).collect();
    let id_chars = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    let well_formed = !segments.is_empty()
        && segments.len() <= TENANT_SCOPE_SEGMENTS
        && segments.iter().all(|segment| {
            segment
                .split_once(':')
                .is_some_and(|(kind, id)| id_chars(kind) && id_chars(id))
        });
    (!well_formed).then(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("invalid scope `{scope}`"),
                "errorCode": "BAD_REQUEST"
            })),
        )
    })
}

/// [`refuse_scope`] over a JSON body's `scope`.
fn refuse_body_scope(body: &Value) -> Option<(StatusCode, Json<Value>)> {
    refuse_scope(
        body.get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

/// Records the request and applies auth / forced failure. `Some` short-circuits.
fn gate(
    state: &Shared,
    method: &str,
    uri: &Uri,
    headers: &HeaderMap,
) -> Option<(StatusCode, Json<Value>)> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    {
        let mut seen = state.seen.lock().expect("seen");
        seen.requests.push(format!("{method} {uri}"));
        seen.auth.push(auth.clone());
    }
    let token = auth.strip_prefix("Bearer ").unwrap_or_default();
    let expected = state.accept_token.lock().expect("token").clone();
    if token.is_empty() || expected.as_deref().is_some_and(|e| e != token) {
        return Some((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "success": false, "error": "expired", "errorCode": "UNAUTHORIZED" })),
        ));
    }
    if let Some((status, code)) = *state.fail_all.lock().expect("fail") {
        return Some((
            StatusCode::from_u16(status).expect("status"),
            Json(json!({ "success": false, "error": "forced", "errorCode": code })),
        ));
    }
    None
}

async fn experience(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_body_scope(&body) {
        return refused;
    }
    // The backend's rate limiter answers before the memory API sees the
    // request, so no claim is taken.
    if state
        .rate_limit_experience
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "success": false, "error": "slow down", "errorCode": "RATE_LIMITED" })),
        );
    }
    let claim = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    state.seen.lock().expect("seen").idempotency.push((
        body.get("idempotency_key")
            .and_then(Value::as_str)
            .map(str::to_owned),
        claim.clone(),
    ));
    if let Some(claim) = claim {
        if !state.claimed.lock().expect("claimed").insert(claim) {
            return (
                StatusCode::CONFLICT,
                Json(
                    json!({ "success": false, "error": "already claimed", "errorCode": "CONFLICT" }),
                ),
            );
        }
    }
    if state
        .claim_then_fail
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return (
            StatusCode::BAD_GATEWAY,
            Json(
                json!({ "success": false, "error": "engine refused", "errorCode": "BAD_GATEWAY" }),
            ),
        );
    }
    let call = state.experience_calls.fetch_add(1, Ordering::SeqCst) + 1;
    if state.fail_nth_experience.load(Ordering::SeqCst) == call {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "rejected", "errorCode": "VALIDATION_ERROR" })),
        );
    }
    let (status, Json(value)) = cortex_experience(State(state.log.clone()), Json(body)).await;
    if state
        .apply_then_fail
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "success": false, "error": "lost", "errorCode": "UNAVAILABLE" })),
        );
    }
    envelope(status, value)
}

async fn events(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<std::collections::BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_scope(params.get("scope").map_or("", String::as_str)) {
        return refused;
    }
    // Express parses a repeated key into an array, which the backend's schema
    // refuses; several labels must share one comma-separated parameter.
    let repeated_labels = uri.query().is_some_and(|query| {
        query
            .split('&')
            .filter(|p| p.starts_with("labels="))
            .count()
            > 1
    });
    if repeated_labels {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "labels must be a string",
                "errorCode": "VALIDATION_ERROR"
            })),
        );
    }
    if state
        .rate_limit_events
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "success": false, "error": "slow down", "errorCode": "RATE_LIMITED" })),
        );
    }
    let Json(mut page) = cortex_events(State(state.log.clone()), Query(params)).await;
    if state
        .hide_listing_for
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        page["items"] = json!([]);
        page["has_more"] = json!(false);
    }
    envelope(StatusCode::OK, page)
}

/// One event by id, or 404.
async fn event(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    let found = state
        .log
        .lock()
        .expect("log")
        .events
        .iter()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .cloned();
    match found {
        Some(mut event) => {
            if state.foreign.lock().expect("foreign").contains(&id) {
                event["scope"] = Value::Null;
            }
            envelope(StatusCode::OK, event)
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "error": "no such event", "errorCode": "NOT_FOUND" })),
        ),
    }
}

async fn recall(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_body_scope(&body) {
        return refused;
    }
    let Json(value) = cortex_recall(State(state.log.clone()), Json(body)).await;
    envelope(StatusCode::OK, value)
}

async fn forget(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_body_scope(&body) {
        return refused;
    }
    if state
        .rate_limit_forget
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "success": false, "error": "slow down", "errorCode": "RATE_LIMITED" })),
        );
    }
    let (status, Json(value)) = cortex_forget(State(state.log.clone()), Json(body)).await;
    envelope(status, value)
}

async fn scopes(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    // No prefix is fine: the memory API then lists the tenant's own root.
    if let Some(prefix) = params.get("prefix") {
        if let Some(refused) = refuse_scope(prefix) {
            return refused;
        }
    }
    let mut params = params;
    if state.ignore_scope_limit.load(Ordering::SeqCst) {
        params.remove("limit");
    }
    let Json(value) = cortex_scopes(State(state.log.clone()), Query(params)).await;
    envelope(StatusCode::OK, value)
}

/// One page of a derived layer: `GET /memory/{facts,beliefs,understanding}`,
/// paged by `limit` and an offset `cursor` as the engine pages them.
async fn layer(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<std::collections::BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    let scope = params.get("scope").cloned().unwrap_or_default();
    if let Some(refused) = refuse_scope(&scope) {
        return refused;
    }
    let name = uri
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let items = state
        .layers
        .lock()
        .expect("layers")
        .get(&(name, scope))
        .cloned()
        .unwrap_or_default();
    let cursor: usize = params
        .get("cursor")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let limit: usize = params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(50);
    let page: Vec<Value> = items.iter().skip(cursor).take(limit).cloned().collect();
    let next = cursor + page.len();
    let mut body = json!({ "items": page, "has_more": next < items.len() });
    if next < items.len() {
        body["next_cursor"] = json!(next.to_string());
    }
    envelope(StatusCode::OK, body)
}

async fn answer(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_body_scope(&body) {
        return refused;
    }
    let object = body.as_object().expect("object body");
    let allowed = |k: &str| ANSWER_KEYS.contains(&k) || k == "use_pack_id";
    if let Some(unknown) = object.keys().find(|k| !allowed(k)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("unknown key {unknown}"),
                "errorCode": "VALIDATION_ERROR"
            })),
        );
    }
    if object
        .get("answer_instructions")
        .is_some_and(Value::is_null)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "success": false, "error": "null instructions", "errorCode": "VALIDATION_ERROR" }),
            ),
        );
    }
    envelope(
        StatusCode::OK,
        json!({
            "answer": "grounded",
            "citations": [{ "id": "evt_1", "key": "k", "content": "c", "score": 0.5 }],
            "context_block": "c",
            "diagnostics": { "answer_model": "m" }
        }),
    )
}

pub(crate) async fn hosted_backend() -> (String, Shared) {
    let state: Shared = Arc::new(Hosted {
        log: Arc::new(Mutex::new(CortexLog::default())),
        seen: Mutex::new(Seen::default()),
        hide_listing_for: AtomicUsize::new(0),
        rate_limit_events: AtomicUsize::new(0),
        fail_all: Mutex::new(None),
        accept_token: Mutex::new(None),
        claimed: Mutex::new(HashSet::new()),
        apply_then_fail: AtomicUsize::new(0),
        fail_nth_experience: AtomicUsize::new(0),
        experience_calls: AtomicUsize::new(0),
        ignore_scope_limit: AtomicBool::new(false),
        rate_limit_experience: AtomicUsize::new(0),
        claim_then_fail: AtomicUsize::new(0),
        rate_limit_forget: AtomicUsize::new(0),
        foreign: Mutex::new(HashSet::new()),
        layers: Mutex::new(HashMap::new()),
    });
    let app = Router::new()
        .route("/memory/experience", post(experience))
        .route("/memory/events", get(events))
        .route("/memory/events/{id}", get(event))
        .route("/memory/recall", post(recall))
        .route("/memory/forget", post(forget))
        .route("/memory/scopes", get(scopes))
        .route("/memory/answer", post(answer))
        .route("/memory/facts", get(layer))
        .route("/memory/beliefs", get(layer))
        .route("/memory/understanding", get(layer))
        .with_state(state.clone());
    (serve(app).await, state)
}

pub(crate) fn provider(endpoint: &str) -> crate::CortexProvider {
    tinyhumans_provider(endpoint, Arc::new(StaticBearer::new("tiny_live_test")))
        .expect("loopback endpoints are allowed")
}

/// [`provider`] with a short visibility budget, for tests that must see a
/// write's outcome declared unknown without waiting the full 30s.
pub(crate) fn provider_with_budget(
    endpoint: &str,
    budget: std::time::Duration,
) -> crate::CortexProvider {
    let memory =
        crate::CortexMemory::tinyhumans(endpoint, Arc::new(StaticBearer::new("tiny_live_test")))
            .expect("loopback endpoints are allowed")
            .with_visibility_timeout(budget);
    crate::cortex_provider(memory)
}
