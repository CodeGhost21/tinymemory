//! The TinyHumans-hosted CortexDB dialect, against a `/memory/*` double.
//!
//! The double wraps the same append-only CortexDB log the conformance suite
//! uses in the backend's `{success,data}` envelope, checks the bearer, records
//! every request, and enforces the strict `answer` schema.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tinymemory_api::error::MemoryError;
use tinymemory_api::evidence::EvidenceRef;
use tinymemory_api::health::MemoryHealth;
use tinymemory_api::learning::{CueFamily, FacetClass, LearningCandidate};
use tinymemory_api::provider::types::IngestItem;
use tinymemory_api::provider::{
    AnswerRequest, MemoryConversationIngest, MemoryCore, MemoryDocumentIngest, MemoryEventIngest,
    MemoryLearningIngest, MemoryProvider, RawMemoryEvent,
};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use crate::conformance_test::{
    cortex_events, cortex_experience, cortex_forget, cortex_recall, cortex_scopes, serve,
    CortexLog, CortexStore,
};
use crate::{
    error_code, is_insufficient_credits, tinyhumans_provider, BearerSource, StaticBearer,
    TINYHUMANS_DRIVER_ID,
};

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
struct Seen {
    /// `"METHOD /path?query"` for every request, in order.
    requests: Vec<String>,
    /// The `Authorization` header of every request.
    auth: Vec<String>,
    /// `(body idempotency_key, Idempotency-Key header)` of every experience write.
    idempotency: Vec<(Option<String>, Option<String>)>,
}

struct Hosted {
    log: CortexStore,
    seen: Mutex<Seen>,
    /// Event listings hide the newest event for this many requests.
    hide_listing_for: AtomicUsize,
    /// Fail this many event listings with 429 before answering.
    rate_limit_events: AtomicUsize,
    /// When set, every request fails with this status + code.
    fail_all: Mutex<Option<(u16, &'static str)>>,
    /// The token the double accepts; `None` accepts any non-empty bearer.
    accept_token: Mutex<Option<String>>,
}

type Shared = Arc<Hosted>;

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
    state.seen.lock().expect("seen").idempotency.push((
        body.get("idempotency_key")
            .and_then(Value::as_str)
            .map(str::to_owned),
        headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    ));
    let (status, Json(value)) = cortex_experience(State(state.log.clone()), Json(body)).await;
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

async fn recall(
    State(state): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(early) = gate(&state, "POST", &uri, &headers) {
        return early;
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
    let Json(value) = cortex_scopes(State(state.log.clone()), Query(params)).await;
    envelope(StatusCode::OK, value)
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

async fn hosted_backend() -> (String, Shared) {
    let state: Shared = Arc::new(Hosted {
        log: Arc::new(Mutex::new(CortexLog::default())),
        seen: Mutex::new(Seen::default()),
        hide_listing_for: AtomicUsize::new(0),
        rate_limit_events: AtomicUsize::new(0),
        fail_all: Mutex::new(None),
        accept_token: Mutex::new(None),
    });
    let app = Router::new()
        .route("/memory/experience", post(experience))
        .route("/memory/events", get(events))
        .route("/memory/recall", post(recall))
        .route("/memory/forget", post(forget))
        .route("/memory/scopes", get(scopes))
        .route("/memory/answer", post(answer))
        .with_state(state.clone());
    (serve(app).await, state)
}

fn provider(endpoint: &str) -> crate::CortexProvider {
    tinyhumans_provider(endpoint, Arc::new(StaticBearer::new("tiny_live_test")))
        .expect("loopback endpoints are allowed")
}

fn item(source: &str, content: &str) -> IngestItem {
    IngestItem {
        namespace: None,
        source: tinymemory_api::chunks::DataSource::Conversation,
        source_id: source.to_string(),
        owner: "test-user".to_string(),
        source_ref: None,
        content: content.to_string(),
        mime: Some("text/plain".to_string()),
        timestamp: None,
        tags: vec![],
        author: None,
        channel_label: None,
        platform: Some("tinymemory-test".to_string()),
        to: Vec::new(),
        cc: Vec::new(),
        subject: None,
        list_unsubscribe: None,
        taint: MemoryTaint::Internal,
        path_scope: None,
    }
}

#[tokio::test]
async fn hosted_provider_upholds_the_contract() {
    let (endpoint, _) = hosted_backend().await;
    tinymemory_conformance::assert_provider(Arc::new(provider(&endpoint))).await;
}

#[tokio::test]
async fn every_operation_maps_to_a_memory_path_and_never_a_v1_one() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    assert_eq!(p.driver_id(), TINYHUMANS_DRIVER_ID);

    p.store(
        "ns",
        "k",
        "hello world",
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await
    .expect("store");
    assert!(p.get("ns", "k").await.expect("get").is_some());
    p.list(Some("ns"), None, None).await.expect("list");
    p.namespaces().await.expect("namespaces");
    assert!(p.forget("ns", "k").await.expect("forget"));
    assert!(
        matches!(p.health().await, MemoryHealth::Ready),
        "health is a cheap scopes listing"
    );
    p.ingest_document(item("doc-1", "a document"))
        .await
        .expect("document");
    p.ingest_learning(LearningCandidate {
        class: FacetClass::Tooling,
        key: "tone".to_string(),
        value: "terse".to_string(),
        cue_family: CueFamily::Explicit,
        evidence: EvidenceRef::ToolCall {
            tool_name: "shell".to_string(),
            episodic_id: 7,
        },
        initial_confidence: 0.9,
        observed_at: 1_700_000_000.0,
    })
    .await
    .expect("learning");
    p.ingest_event(RawMemoryEvent {
        id: "e1".into(),
        namespace: "events".into(),
        event_type: "note".into(),
        content: "something happened".into(),
        session_id: None,
        occurred_at: None,
        metadata: json!({}),
        taint: MemoryTaint::Internal,
    })
    .await
    .expect("event");
    let answered = p
        .as_answer()
        .expect("answer capability")
        .answer(AnswerRequest::new("what happened?"))
        .await
        .expect("answer");
    assert_eq!(answered.answer, "grounded");

    let seen = state.seen.lock().expect("seen");
    let paths: Vec<&str> = seen
        .requests
        .iter()
        .map(|r| r.split_once(' ').expect("method path").1)
        .collect();
    for expected in [
        "/memory/experience",
        "/memory/events?",
        "/memory/recall",
        "/memory/forget",
        "/memory/scopes",
        "/memory/scopes?prefix=__health__",
        "/memory/answer",
    ] {
        assert!(
            paths.iter().any(|p| p.starts_with(expected)),
            "no request to {expected}; saw {paths:?}"
        );
    }
    assert!(
        paths.iter().all(|p| p.starts_with("/memory/")),
        "a request left the /memory/ surface: {paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("wait=") || p.contains("bulk")),
        "wait=indexed and bulk are not forwarded by the backend: {paths:?}"
    );
}

#[tokio::test]
async fn the_bearer_is_resolved_on_every_request() {
    struct Rotating(AtomicUsize);
    #[async_trait]
    impl BearerSource for Rotating {
        async fn bearer(&self) -> anyhow::Result<String> {
            Ok(format!("jwt-{}", self.0.fetch_add(1, Ordering::SeqCst)))
        }
    }
    let (endpoint, state) = hosted_backend().await;
    let p =
        tinyhumans_provider(&endpoint, Arc::new(Rotating(AtomicUsize::new(0)))).expect("builds");
    p.list(Some("ns"), None, None).await.expect("first");
    p.list(Some("ns"), None, None).await.expect("second");
    p.namespaces().await.expect("third");

    let seen = state.seen.lock().expect("seen");
    assert!(seen.auth.len() >= 3);
    let distinct: std::collections::BTreeSet<_> = seen.auth.iter().collect();
    assert_eq!(
        distinct.len(),
        seen.auth.len(),
        "every request must carry a freshly resolved token: {:?}",
        seen.auth.len()
    );
    assert_eq!(seen.auth[0], "Bearer jwt-0");
    assert_eq!(seen.auth[1], "Bearer jwt-1");
}

#[tokio::test]
async fn a_source_failure_or_blank_token_is_unauthorized_without_a_request() {
    struct Blank;
    #[async_trait]
    impl BearerSource for Blank {
        async fn bearer(&self) -> anyhow::Result<String> {
            Ok("  ".into())
        }
    }
    struct Broken;
    #[async_trait]
    impl BearerSource for Broken {
        async fn bearer(&self) -> anyhow::Result<String> {
            anyhow::bail!("signed out")
        }
    }
    let (endpoint, state) = hosted_backend().await;
    for source in [Arc::new(Blank) as Arc<dyn BearerSource>, Arc::new(Broken)] {
        let p = tinyhumans_provider(&endpoint, source).expect("builds");
        let error = p.namespaces().await.expect_err("no credential");
        assert!(matches!(error, MemoryError::Unauthorized(_)), "{error:?}");
    }
    assert!(state.seen.lock().expect("seen").requests.is_empty());
}

#[tokio::test]
async fn credentialed_cleartext_is_refused_off_loopback() {
    let source = || Arc::new(StaticBearer::new("t")) as Arc<dyn BearerSource>;
    assert!(tinyhumans_provider("http://api.example.com", source()).is_err());
    assert!(tinyhumans_provider("https://api.example.com", source()).is_ok());
    assert!(tinyhumans_provider("http://127.0.0.1:1", source()).is_ok());
}

#[tokio::test]
async fn a_402_is_insufficient_credits_with_its_code() {
    let (endpoint, state) = hosted_backend().await;
    *state.fail_all.lock().expect("fail") = Some((402, "USER_INSUFFICIENT_CREDITS"));
    let p = provider(&endpoint);
    let error = p
        .store(
            "ns",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("out of credits");
    assert!(matches!(error, MemoryError::BudgetExceeded(_)), "{error:?}");
    assert!(is_insufficient_credits(&error));
    assert_eq!(error_code(&error), Some("USER_INSUFFICIENT_CREDITS"));
    assert!(
        !error.to_string().contains("tiny_live_test"),
        "token leaked"
    );
}

#[tokio::test]
async fn a_401_is_unauthorized_and_a_400_is_invalid_with_codes() {
    let (endpoint, state) = hosted_backend().await;
    *state.accept_token.lock().expect("token") = Some("a-different-token".into());
    let error = provider(&endpoint)
        .namespaces()
        .await
        .expect_err("rejected");
    assert!(matches!(error, MemoryError::Unauthorized(_)), "{error:?}");
    assert_eq!(error_code(&error), Some("UNAUTHORIZED"));

    *state.accept_token.lock().expect("token") = None;
    *state.fail_all.lock().expect("fail") = Some((400, "VALIDATION_ERROR"));
    let error = provider(&endpoint).namespaces().await.expect_err("invalid");
    assert!(matches!(error, MemoryError::Invalid(_)), "{error:?}");
    assert_eq!(error_code(&error), Some("VALIDATION_ERROR"));
    assert!(!is_insufficient_credits(&error));
}

#[tokio::test]
async fn a_429_on_a_read_is_retried_and_then_succeeds() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    p.store(
        "ns",
        "k",
        "v",
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await
    .expect("store");
    state.rate_limit_events.store(2, Ordering::SeqCst);
    assert!(p
        .get("ns", "k")
        .await
        .expect("retried past the 429s")
        .is_some());
}

#[tokio::test]
async fn a_persistent_429_surfaces_as_unavailable() {
    let (endpoint, state) = hosted_backend().await;
    *state.fail_all.lock().expect("fail") = Some((429, "RATE_LIMITED"));
    let error = provider(&endpoint).namespaces().await.expect_err("limited");
    assert!(matches!(error, MemoryError::Unavailable(_)), "{error:?}");
    assert_eq!(error_code(&error), Some("RATE_LIMITED"));
}

#[tokio::test]
async fn a_response_without_the_envelope_is_a_backend_error() {
    let app = Router::new().route(
        "/memory/scopes",
        get(|| async { Json(json!({ "items": [] })) }),
    );
    let endpoint = serve(app).await;
    let error = provider(&endpoint)
        .namespaces()
        .await
        .expect_err("bare body");
    assert!(matches!(error, MemoryError::Backend(_)), "{error:?}");
}

#[tokio::test]
async fn writes_carry_an_idempotency_key_header_matching_the_body() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    p.store(
        "ns",
        "k",
        "v",
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await
    .expect("store");
    p.ingest_document(item("doc", "text")).await.expect("doc");
    let seen = state.seen.lock().expect("seen");
    assert!(seen.idempotency.len() >= 2);
    for (body_key, header) in &seen.idempotency {
        let header = header.as_deref().expect("Idempotency-Key header present");
        assert_eq!(Some(header), body_key.as_deref());
        assert!(header.len() <= 128);
        assert!(header
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')));
    }
}

#[tokio::test]
async fn a_conversation_batch_falls_back_to_ordered_per_item_writes() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let messages: Vec<IngestItem> = ["one", "two", "three"]
        .iter()
        .map(|text| {
            let mut m = item("thread-1", text);
            m.author = Some("user".into());
            m
        })
        .collect();
    let outcome = p.ingest_conversation(messages).await.expect("conversation");
    assert_eq!(outcome.written, 3);
    assert_eq!(outcome.ids.len(), 3);

    let seen = state.seen.lock().expect("seen");
    let writes: Vec<&String> = seen
        .requests
        .iter()
        .filter(|r| r.starts_with("POST /memory/experience"))
        .collect();
    assert_eq!(
        writes.len(),
        3,
        "one write per message: {:?}",
        seen.requests
    );
    assert!(!seen.requests.iter().any(|r| r.contains("bulk")));
    // Order is preserved: the log's ids are minted in arrival order.
    assert_eq!(outcome.ids, vec!["evt_1", "evt_2", "evt_3"]);
}

#[tokio::test]
async fn a_write_polls_until_the_event_is_visible() {
    let (endpoint, state) = hosted_backend().await;
    state.hide_listing_for.store(3, Ordering::SeqCst);
    let p = provider(&endpoint);
    p.ingest_document(item("doc-poll", "poll me"))
        .await
        .expect("waits for indexing rather than failing");
    let seen = state.seen.lock().expect("seen");
    let listings = seen
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /memory/events"))
        .count();
    assert!(listings >= 4, "expected >=4 polls, saw {listings}");
    assert!(
        !seen.requests.iter().any(|r| r.contains("wait=indexed")),
        "the hosted backend drops ?wait=indexed; do not send it"
    );
}

#[tokio::test]
async fn the_answer_body_holds_only_keys_the_strict_schema_allows() {
    // The double rejects unknown keys and null instructions with a 400, so a
    // successful answer with and without instructions proves the body shape.
    let (endpoint, _) = hosted_backend().await;
    let p = provider(&endpoint);
    let answerer = p.as_answer().expect("answer");
    answerer
        .answer(AnswerRequest::new("q1"))
        .await
        .expect("no instructions");
    let mut with = AnswerRequest::new("q2");
    with.instructions = Some("be brief".into());
    answerer.answer(with).await.expect("with instructions");
}
