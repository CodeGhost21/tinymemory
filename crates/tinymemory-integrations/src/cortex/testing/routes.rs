//! The doubles' routes: the CortexDB `/v1/*` surface and the TinyHumans
//! `/memory/*` surface over the same handlers.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::{Shared, take_one};

type Reply = (StatusCode, Json<Value>);

/// Keys the hosted answer schema allows; anything else is a 400.
const ANSWER_KEYS: [&str; 11] = [
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
    "use_pack_id",
];

/// Segments a hosted scope may hold: the memory API re-roots it under the
/// tenant and the engine holds 32.
const TENANT_SCOPE_SEGMENTS: usize = 31;

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

/// A success in the wire's shape.
fn ok(state: &Shared, code: u16, body: Value) -> Reply {
    if state.hosted {
        (status(code), Json(json!({ "success": true, "data": body })))
    } else {
        (status(code), Json(body))
    }
}

/// A failure in the wire's shape.
fn fail(state: &Shared, code: u16, error_code: &str) -> Reply {
    if state.hosted {
        (
            status(code),
            Json(
                json!({ "success": false, "error": format!("failed: {error_code}"), "errorCode": error_code }),
            ),
        )
    } else {
        (status(code), Json(json!({ "error_code": error_code })))
    }
}

/// A log result (status, body) in the wire's shape.
fn relay(state: &Shared, (code, body): (u16, Value)) -> Reply {
    if code < 300 {
        ok(state, code, body)
    } else {
        let error_code = body
            .get("error_code")
            .and_then(Value::as_str)
            .unwrap_or("VALIDATION_ERROR")
            .to_string();
        fail(state, code, &error_code)
    }
}

/// The memory API's scope grammar: `type:id` segments of `[A-Za-z0-9_-]`,
/// at most [`TENANT_SCOPE_SEGMENTS`]. Hosted only.
fn refuse_scope(state: &Shared, scope: &str) -> Option<Reply> {
    if !state.hosted {
        return None;
    }
    let id_chars = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    let segments: Vec<&str> = scope.split('/').collect();
    let well_formed = segments.len() <= TENANT_SCOPE_SEGMENTS
        && segments.iter().all(|s| {
            s.split_once(':')
                .is_some_and(|(k, i)| id_chars(k) && id_chars(i))
        });
    (!well_formed).then(|| fail(state, 400, "BAD_REQUEST"))
}

/// Records the request, checks the bearer, applies `fail_all`.
fn gate(state: &Shared, method: &str, uri: &Uri, headers: &HeaderMap) -> Option<Reply> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    {
        let mut seen = state.seen.lock().unwrap();
        seen.requests.push(format!("{method} {uri}"));
        seen.auth.push(auth.clone());
    }
    let token = auth.strip_prefix("Bearer ").unwrap_or_default();
    let expected = state.accept_token.lock().unwrap().clone();
    if token.is_empty() || expected.is_some_and(|e| e != token) {
        return Some(fail(state, 401, "UNAUTHORIZED"));
    }
    if let Some((code, error_code)) = *state.fail_all.lock().unwrap() {
        return Some(fail(state, code, error_code));
    }
    None
}

/// Applies one write with every write knob.
fn write_one(state: &Shared, headers: &HeaderMap, body: &Value) -> Reply {
    if let Some(refused) = refuse_scope(state, body["scope"].as_str().unwrap_or_default()) {
        return refused;
    }
    // The backend's rate limiter answers before the memory API: no claim.
    if take_one(&state.rate_limit_experience) {
        return fail(state, 429, "RATE_LIMITED");
    }
    let claim = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    state.seen.lock().unwrap().idempotency.push((
        body["idempotency_key"].as_str().map(str::to_owned),
        claim.clone(),
    ));
    if state.hosted
        && let Some(claim) = claim
        && !state.claimed.lock().unwrap().insert(claim)
    {
        return fail(state, 409, "CONFLICT");
    }
    if take_one(&state.claim_then_fail) {
        return fail(state, 502, "BAD_GATEWAY");
    }
    let call = state.experience_calls.fetch_add(1, Ordering::SeqCst) + 1;
    if state.fail_nth_experience.load(Ordering::SeqCst) == call {
        return fail(state, 400, "VALIDATION_ERROR");
    }
    let applied = state.log.lock().unwrap().append(body);
    if let Some((limited, hidden)) = state.arm_after_write.lock().unwrap().take() {
        state.rate_limit_events.store(limited, Ordering::SeqCst);
        state.hide_listing_for.store(hidden, Ordering::SeqCst);
    }
    if take_one(&state.apply_then_fail) {
        return fail(state, 503, "UNAVAILABLE");
    }
    relay(state, applied)
}

async fn experience(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    state.seen.lock().unwrap().writes.push(body.clone());
    write_one(&state, &headers, &body)
}

async fn bulk(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    state.seen.lock().unwrap().writes.push(body.clone());
    let mut results = Vec::new();
    for (index, item) in body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let (code, Json(receipt)) = write_one(&state, &headers, item);
        if !code.is_success() {
            return (code, Json(receipt));
        }
        results.push(json!({
            "index": index,
            "event_id": receipt["event_id"],
            "replayed_from_idempotency": receipt["replayed_from_idempotency"],
        }));
    }
    ok(
        &state,
        200,
        json!({ "accepted": results.len(), "results": results }),
    )
}

/// One delayed listing counted in `listings_in_flight` while it is held,
/// and uncounted when dropped, even when the request is cancelled mid-sleep.
struct InFlight<'a>(&'a Shared);

impl<'a> InFlight<'a> {
    fn enter(state: &'a Shared) -> Self {
        let now = state.listings_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        state.listings_peak.fetch_max(now, Ordering::SeqCst);
        Self(state)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.listings_in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn events(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<BTreeMap<String, String>>,
) -> Reply {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_scope(&state, params.get("scope").map_or("", String::as_str)) {
        return refused;
    }
    let repeated = uri
        .query()
        .is_some_and(|q| q.split('&').filter(|p| p.starts_with("labels=")).count() > 1);
    if state.hosted && repeated {
        return fail(&state, 400, "VALIDATION_ERROR");
    }
    if take_one(&state.rate_limit_events) {
        return fail(&state, 429, "RATE_LIMITED");
    }
    let delay = state.listing_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        let _held = InFlight::enter(&state);
        tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
    }
    let mut page = state.log.lock().unwrap().page(&params);
    if take_one(&state.hide_listing_for) {
        page["items"] = json!([]);
        page["has_more"] = json!(false);
    }
    ok(&state, 200, page)
}

async fn recall(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    state.seen.lock().unwrap().recalls.push(body.clone());
    if let Some(refused) = refuse_scope(&state, body["scope"].as_str().unwrap_or_default()) {
        return refused;
    }
    if state.recall_down.load(Ordering::SeqCst) {
        return fail(&state, 500, "INTERNAL");
    }
    let pack = state.log.lock().unwrap().recall(&body);
    ok(&state, 200, pack)
}

async fn forget(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    if let Some(refused) = refuse_scope(&state, body["scope"].as_str().unwrap_or_default()) {
        return refused;
    }
    if take_one(&state.rate_limit_forget) {
        return fail(&state, 429, "RATE_LIMITED");
    }
    state.seen.lock().unwrap().forgets.push(body.clone());
    let result = state.log.lock().unwrap().forget(&body);
    relay(&state, result)
}

async fn answer(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    state.seen.lock().unwrap().answers.push(body.clone());
    if let Some(refused) = refuse_scope(&state, body["scope"].as_str().unwrap_or_default()) {
        return refused;
    }
    let object = body.as_object().cloned().unwrap_or_default();
    let strict_violation = object.keys().any(|k| !ANSWER_KEYS.contains(&k.as_str()))
        || object
            .get("answer_instructions")
            .is_some_and(Value::is_null);
    if state.hosted && strict_violation {
        return fail(&state, 400, "VALIDATION_ERROR");
    }
    if body["use_pack_id"].as_str() != Some("pack_test") {
        return fail(&state, 400, "MISSING_PACK");
    }
    if take_one(&state.expire_packs) {
        return fail(&state, 404, "NOT_FOUND");
    }
    ok(
        &state,
        200,
        json!({
            "answer": format!("grounded answer for {}", body["question"].as_str().unwrap_or_default()),
            "citations": [],
            "diagnostics": { "answer_model": "reasoning" }
        }),
    )
}

async fn health(State(state): State<Shared>, uri: Uri, headers: HeaderMap) -> Reply {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    ok(&state, 200, json!({ "status": "healthy" }))
}

async fn scopes(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<BTreeMap<String, String>>,
) -> Reply {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    if let Some(prefix) = params.get("prefix")
        && let Some(refused) = refuse_scope(&state, prefix)
    {
        return refused;
    }
    let prefix = params.get("prefix").cloned().unwrap_or_default();
    let mut scopes = state.log.lock().unwrap().scopes(&prefix);
    let padding = state.padding_scopes.load(Ordering::SeqCst);
    scopes.extend(
        (0..padding)
            .map(|n| format!("app:tinymemory/agent:pad-{n:04}/app:learnings"))
            .filter(|path| prefix.is_empty() || path.starts_with(&prefix)),
    );
    scopes.sort();
    // As CortexDB: `limit` defaults to 50, is clamped to 1000, no cursor.
    let limit = params
        .get("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(50)
        .min(1000);
    scopes.truncate(limit);
    if state.hosted {
        ok(&state, 200, json!({ "scopes": scopes }))
    } else {
        let items: Vec<Value> = scopes.iter().map(|path| json!({ "path": path })).collect();
        ok(&state, 200, json!({ "items": items }))
    }
}

async fn build_beliefs(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Reply {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
    }
    let Some(scope) = body["scope"].as_str().filter(|scope| !scope.is_empty()) else {
        return fail(&state, 422, "VALIDATION_ERROR");
    };
    if let Some(refused) = refuse_scope(&state, scope) {
        return refused;
    }
    state.seen.lock().unwrap().builds.push(body.clone());
    // CortexDB v0.10 builds within the request and reports the count.
    let built = state.log.lock().unwrap().build(scope);
    ok(
        &state,
        200,
        json!({ "built": built, "items": [], "facts_scanned": built, "events_scanned": built }),
    )
}

async fn beliefs(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> Reply {
    if let Some(early) = gate(&state, "GET", &uri, &headers) {
        return early;
    }
    let scope = query.get("scope").cloned().unwrap_or_default();
    if let Some(refused) = refuse_scope(&state, &scope) {
        return refused;
    }
    let items = state.log.lock().unwrap().list_beliefs(&scope);
    ok(&state, 200, json!({ "items": items, "has_more": false }))
}

/// CortexDB's own routes.
pub(super) fn direct(state: Shared) -> Router {
    Router::new()
        .route("/v1/experience", post(experience))
        .route("/v1/experience/bulk", post(bulk))
        .route("/v1/events", get(events))
        .route("/v1/recall", post(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/answer", post(answer))
        .route("/v1/admin/health", get(health))
        .route("/v1/scopes/list", get(scopes))
        .route("/v1/beliefs/build", post(build_beliefs))
        .route("/v1/beliefs", get(beliefs))
        .with_state(state)
}

/// The TinyHumans backend's routes.
pub(super) fn hosted(state: Shared) -> Router {
    Router::new()
        .route("/memory/experience", post(experience))
        .route("/memory/events", get(events))
        .route("/memory/recall", post(recall))
        .route("/memory/forget", post(forget))
        .route("/memory/answer", post(answer))
        .route("/memory/scopes", get(scopes))
        .with_state(state)
}
