//! The TinyHumans-hosted CortexDB dialect, against a `/memory/*` double.
//!
//! The double wraps the same append-only CortexDB log the conformance suite
//! uses in the backend's `{success,data}` envelope, checks the bearer, records
//! every request, and enforces the strict `answer` schema.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use tinymemory_api::error::MemoryError;
use tinymemory_api::evidence::EvidenceRef;
use tinymemory_api::health::MemoryHealth;
use tinymemory_api::learning::{CueFamily, FacetClass, LearningCandidate};
use tinymemory_api::provider::types::ExportRecord;
use tinymemory_api::provider::types::IngestItem;
use tinymemory_api::provider::{
    AnswerRequest, MemoryConversationIngest, MemoryCore, MemoryDocumentIngest, MemoryEventIngest,
    MemoryLearningIngest, MemoryPortability, MemoryProvider, RawMemoryEvent,
};
use tinymemory_api::types::{MemoryCategory, MemoryEntry, MemoryTaint};

use crate::conformance_test::serve;
use crate::hosted_test_support::{hosted_backend, provider, provider_with_budget, Shared};
use crate::{
    error_code, is_insufficient_credits, tinyhumans_provider, BearerSource, StaticBearer,
    TINYHUMANS_DRIVER_ID,
};

/// Every event currently in the double's log for `key`, oldest first, as the
/// envelope content each one carries.
fn versions_of(state: &Shared, key: &str) -> Vec<String> {
    state
        .log
        .lock()
        .expect("log")
        .events
        .iter()
        .filter_map(|event| {
            let text = event.pointer("/content/text")?.as_str()?;
            let envelope: Value = serde_json::from_str(text).ok()?;
            (envelope.get("k")?.as_str()? == key)
                .then(|| envelope.get("c")?.as_str().map(str::to_owned))?
        })
        .collect()
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
    // "METHOD /path?sorted,query,keys": the exact set of routes and query
    // parameters used, with values (scopes, cursors) left out.
    let shapes: std::collections::BTreeSet<String> = seen
        .requests
        .iter()
        .map(|request| {
            let (method, target) = request.split_once(' ').expect("method target");
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let mut keys: Vec<&str> = query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| pair.split_once('=').map_or(pair, |(k, _)| k))
                .collect();
            keys.sort_unstable();
            format!("{method} {path}?{}", keys.join(","))
        })
        .collect();
    let expected: std::collections::BTreeSet<String> = [
        "POST /memory/experience?",
        "GET /memory/events?limit,scope",
        "POST /memory/recall?",
        "POST /memory/forget?",
        "GET /memory/scopes?limit",
        "GET /memory/scopes?limit,prefix",
        "POST /memory/answer?",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        shapes, expected,
        "the hosted routes and query parameters used"
    );
}

#[tokio::test]
async fn the_health_probe_names_a_scope_the_memory_api_accepts() {
    // The double refuses a malformed prefix exactly as the backend relays the
    // memory API's refusal, so `Ready` here proves the probe's prefix is one
    // the real stack lists rather than rejects.
    let (endpoint, state) = hosted_backend().await;
    let health = provider(&endpoint).health().await;
    assert!(matches!(health, MemoryHealth::Ready), "{health:?}");

    let seen = state.seen.lock().expect("seen");
    let probe = seen
        .requests
        .iter()
        .find(|r| r.starts_with("GET /memory/scopes"))
        .expect("the probe lists scopes");
    assert!(probe.contains("prefix="), "{probe}");
    assert!(
        probe.contains("limit=1"),
        "the probe asks for one entry: {probe}"
    );
}

#[tokio::test]
async fn a_namespace_too_deep_once_rooted_is_refused_before_it_is_sent() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let namespace = |depth: usize| {
        (0..depth)
            .map(|i| format!("s{i}"))
            .collect::<Vec<_>>()
            .join("/")
    };
    p.store(
        &namespace(31),
        "k",
        "fits under the tenant root",
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await
    .expect("31 segments plus the tenant root is the engine's 32");
    let sent = state.seen.lock().expect("seen").requests.len();

    let error = p
        .store(
            &namespace(32),
            "k",
            "one segment too many",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("32 segments cannot be rooted under the tenant");
    assert!(error.to_string().contains("31"), "{error}");
    assert_eq!(
        state.seen.lock().expect("seen").requests.len(),
        sent,
        "refused locally, not by the backend"
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
async fn write_headers_are_random_per_call_and_never_the_content_key() {
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
    let mut headers = HashSet::new();
    for (body_key, header) in &seen.idempotency {
        let header = header.as_deref().expect("Idempotency-Key header present");
        assert_ne!(
            Some(header),
            body_key.as_deref(),
            "header must not be the content key"
        );
        assert!(header.starts_with("tm-"));
        assert!(header.len() <= 128);
        assert!(header
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')));
        assert!(
            headers.insert(header.to_string()),
            "header reused: {header}"
        );
    }
}

#[tokio::test]
async fn reingesting_identical_content_succeeds_in_hosted_mode() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let first = p
        .ingest_document(item("doc-same", "same text"))
        .await
        .expect("first");
    assert_eq!(first.written, 1);
    let second = p
        .ingest_document(item("doc-same", "same text"))
        .await
        .expect("an identical re-ingest must not 409 on the metering claim");
    assert!(
        second.already_ingested,
        "the engine dedupes on the body key: {second:?}"
    );
    assert_eq!(state.log.lock().expect("log").events.len(), 1);
}

#[tokio::test]
async fn a_write_applied_before_a_transport_fault_is_recovered_not_failed() {
    let (endpoint, state) = hosted_backend().await;
    state.apply_then_fail.store(1, Ordering::SeqCst);
    let p = provider(&endpoint);
    let outcome = p
        .ingest_document(item("doc-lost", "applied then the response was lost"))
        .await
        .expect("the retry's 409 is success-unknown, resolved by looking the event up");
    assert_eq!(outcome.ids.len() + usize::from(outcome.already_ingested), 1);
    assert_eq!(
        state.log.lock().expect("log").events.len(),
        1,
        "exactly one event: the retry was never forwarded"
    );
    let seen = state.seen.lock().expect("seen");
    let headers: Vec<_> = seen.idempotency.iter().map(|(_, h)| h.clone()).collect();
    assert_eq!(headers.len(), 2);
    assert_eq!(
        headers[0], headers[1],
        "one logical call reuses its claim across retries"
    );
}

#[tokio::test]
async fn a_keyed_store_rides_out_a_rate_limit() {
    let (endpoint, state) = hosted_backend().await;
    state.rate_limit_experience.store(2, Ordering::SeqCst);
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
    .expect("a 429 on a keyed write is retried, not surfaced");
    assert_eq!(versions_of(&state, "k"), vec!["v"]);
    let writes = state
        .seen
        .lock()
        .expect("seen")
        .requests
        .iter()
        .filter(|r| r.starts_with("POST /memory/experience"))
        .count();
    assert_eq!(writes, 3, "two refusals, then the accepted write");
}

#[tokio::test]
async fn a_keyed_store_applied_before_its_response_was_lost_is_recovered() {
    let (endpoint, state) = hosted_backend().await;
    state.apply_then_fail.store(1, Ordering::SeqCst);
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
    .expect("the retry's 409 is resolved by finding the write");
    assert_eq!(
        versions_of(&state, "k"),
        vec!["v"],
        "exactly one version: the retry was never forwarded"
    );
    let seen = state.seen.lock().expect("seen");
    let headers: Vec<_> = seen.idempotency.iter().map(|(_, h)| h.clone()).collect();
    assert_eq!(headers.len(), 2);
    assert_eq!(
        headers[0], headers[1],
        "one logical write reuses its claim across retries"
    );
}

#[tokio::test]
async fn recovery_never_takes_an_older_identical_version_for_the_write() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider_with_budget(&endpoint, std::time::Duration::from_secs(2));
    for value in ["first", "second"] {
        p.store(
            "ns",
            "k",
            value,
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    }
    // The third store repeats the first value. Its claim is taken and nothing
    // is applied, so the retry meets its own claim; the old "first" event has
    // the same text but is not the key's newest version.
    state.claim_then_fail.store(1, Ordering::SeqCst);
    let error = p
        .store(
            "ns",
            "k",
            "first",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("an unfound write is outcome-unknown, never success");
    assert!(error.to_string().contains("unknown"), "{error}");
    let current = p.get("ns", "k").await.expect("get").expect("present");
    assert_eq!(current.content, "second");
}

#[tokio::test]
async fn a_forget_rides_out_rate_limits_on_both_of_its_moves() {
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
    state.rate_limit_experience.store(1, Ordering::SeqCst);
    state.rate_limit_forget.store(1, Ordering::SeqCst);
    assert!(p.forget("ns", "k").await.expect("both moves are retried"));
    assert!(p.get("ns", "k").await.expect("get").is_none());
    let forgets = state
        .seen
        .lock()
        .expect("seen")
        .requests
        .iter()
        .filter(|r| r.starts_with("POST /memory/forget"))
        .count();
    assert_eq!(forgets, 2, "the rate-limited removal was sent again");
}

#[tokio::test]
async fn recovery_waits_out_a_slow_listing_and_a_rate_limit() {
    // tinymemory#169 review: recovery gave up after 2s and on the first 429,
    // so a write that had succeeded could be reported as failed.
    let (endpoint, state) = hosted_backend().await;
    state.apply_then_fail.store(1, Ordering::SeqCst);
    // The first recovery listing is rate limited past the transport's own
    // three attempts, and the next four do not show the event yet.
    state.rate_limit_events.store(3, Ordering::SeqCst);
    state.hide_listing_for.store(4, Ordering::SeqCst);
    let outcome = provider(&endpoint)
        .ingest_document(item("doc-slow", "applied, then slow to list"))
        .await
        .expect("recovered within the visibility budget");
    assert_eq!(outcome.ids.len() + usize::from(outcome.already_ingested), 1);
    assert_eq!(state.log.lock().expect("log").events.len(), 1);
}

/// An export record for a keyed entry, as another engine's export would
/// produce it.
fn record(namespace: &str, key: &str, content: &str) -> ExportRecord {
    record_as(
        namespace,
        key,
        content,
        (MemoryCategory::Core, None, MemoryTaint::Internal),
    )
}

/// [`record`], with the category, session and taint given.
fn record_as(
    namespace: &str,
    key: &str,
    content: &str,
    (category, session_id, taint): (MemoryCategory, Option<&str>, MemoryTaint),
) -> ExportRecord {
    tinymemory_api::mandatory::to_record(MemoryEntry {
        id: format!("rec-{key}"),
        key: key.to_string(),
        content: content.to_string(),
        namespace: Some(namespace.to_string()),
        category,
        timestamp: String::new(),
        session_id: session_id.map(str::to_string),
        score: None,
        taint,
    })
}

/// How many requests `seen` holds that start with `prefix`.
fn count_requests(state: &Shared, prefix: &str) -> usize {
    state
        .seen
        .lock()
        .expect("seen")
        .requests
        .iter()
        .filter(|r| r.starts_with(prefix))
        .count()
}

#[tokio::test]
async fn a_hosted_export_lists_each_namespace_once() {
    // The mandatory export folds the whole account on every page. With a
    // namespace per ingested document that is quadratic, and every listing
    // is billed against a 300-a-minute limit.
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    for i in 0..6 {
        p.store(
            &format!("ns{i}"),
            "k",
            &format!("value {i}"),
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    }
    // A namespace whose only key was forgotten folds to nothing and must be
    // stepped over, not handed back as an empty page with a cursor.
    assert!(p.forget("ns3", "k").await.expect("forget"));
    state.seen.lock().expect("seen").requests.clear();

    let mut cursor: Option<String> = None;
    let mut exported = Vec::new();
    loop {
        let page = p
            .export_page(cursor.as_deref(), 500)
            .await
            .expect("export page");
        assert!(
            !page.records.is_empty() || page.next_cursor.is_none(),
            "an empty page must end the export"
        );
        exported.extend(page.records);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let mut namespaces: Vec<String> = exported
        .iter()
        .filter_map(|r| r.namespace.clone())
        .collect();
    namespaces.sort();
    assert_eq!(namespaces, ["ns0", "ns1", "ns2", "ns4", "ns5"]);
    assert_eq!(
        count_requests(&state, "GET /memory/events"),
        6,
        "one listing per namespace, including the empty one: {:?}",
        state.seen.lock().expect("seen").requests
    );
}

#[tokio::test]
async fn a_hosted_import_waits_once_per_scope_and_never_probes_recall() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let records = (0..5)
        .map(|i| record("imported", &format!("k{i}"), &format!("value {i}")))
        .collect();
    let outcome = p.import_records(records).await.expect("import");
    assert_eq!((outcome.imported, outcome.failed), (5, 0), "{outcome:?}");
    assert_eq!(count_requests(&state, "POST /memory/experience"), 5);
    assert_eq!(
        count_requests(&state, "POST /memory/recall"),
        0,
        "a keyed read does not need the recall index, and each probe is billed"
    );
    assert_eq!(
        count_requests(&state, "GET /memory/events"),
        2,
        "one read of what the scope holds, one visibility wait for its last record"
    );
    for i in 0..5 {
        let back = p
            .get("imported", &format!("k{i}"))
            .await
            .expect("get")
            .expect("imported record is readable");
        assert_eq!(back.content, format!("value {i}"));
    }
}

/// An import run again writes only what changed. A record held with the same
/// content, category, session and taint is skipped; a change to any of them
/// is written.
#[tokio::test]
async fn a_hosted_import_run_again_writes_only_what_changed() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let keys = ["same", "edited", "category", "session", "taint"];
    let first = || keys.iter().map(|key| record("again", key, "v1")).collect();
    let outcome = p.import_records(first()).await.expect("import");
    assert_eq!(
        (outcome.imported, outcome.skipped, outcome.failed),
        (5, 0, 0)
    );
    let outcome = p.import_records(first()).await.expect("import again");
    assert_eq!(
        (outcome.imported, outcome.skipped, outcome.failed),
        (0, 5, 0),
        "{outcome:?}"
    );
    assert_eq!(count_requests(&state, "POST /memory/experience"), 5);

    let core = MemoryCategory::Core;
    let internal = MemoryTaint::Internal;
    let changed = vec![
        record("again", "same", "v1"),
        record("again", "edited", "v2"),
        record_as(
            "again",
            "category",
            "v1",
            (MemoryCategory::Custom("pinned".into()), None, internal),
        ),
        record_as(
            "again",
            "session",
            "v1",
            (core.clone(), Some("s1"), internal),
        ),
        record_as(
            "again",
            "taint",
            "v1",
            (core, None, MemoryTaint::ExternalSync),
        ),
        record("again", "new", "v1"),
    ];
    let outcome = p.import_records(changed).await.expect("import changes");
    assert_eq!(
        (outcome.imported, outcome.skipped, outcome.failed),
        (5, 1, 0),
        "{outcome:?}"
    );
    assert_eq!(count_requests(&state, "POST /memory/experience"), 10);
    let edited = p.get("again", "edited").await.expect("get").expect("held");
    assert_eq!(edited.content, "v2");

    // Custom categories round-trip, so an unchanged one is still skipped.
    let pinned = record_as(
        "again",
        "category",
        "v1",
        (MemoryCategory::Custom("pinned".into()), None, internal),
    );
    let outcome = p.import_records(vec![pinned]).await.expect("import");
    assert_eq!((outcome.imported, outcome.skipped), (0, 1), "{outcome:?}");
}

/// A record repeated in one batch is written once: the batch counts what it
/// wrote as held.
#[tokio::test]
async fn a_record_repeated_in_one_batch_is_written_once() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let outcome = p
        .import_records(vec![record("twice", "k", "v"), record("twice", "k", "v")])
        .await
        .expect("import");
    assert_eq!((outcome.imported, outcome.skipped), (1, 1), "{outcome:?}");
    assert_eq!(count_requests(&state, "POST /memory/experience"), 1);
}

/// The read of what a namespace holds rides out a rate-limit window like a
/// write does, and fails the batch only once every pause is spent.
#[tokio::test]
async fn a_hosted_import_rides_out_a_rate_limited_read() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint).with_import_patience(vec![std::time::Duration::from_millis(20)]);
    // Each round is three quick attempts, so seven refusals outlast two
    // rounds (one pause) and are ridden out by a third (two pauses).
    state.rate_limit_events.store(7, Ordering::SeqCst);
    let error = p
        .import_records(vec![record("listed", "k", "v")])
        .await
        .expect_err("one pause is not enough for seven refusals");
    assert!(matches!(error, MemoryError::Unavailable(_)), "{error:?}");
    assert_eq!(count_requests(&state, "POST /memory/experience"), 0);

    state.rate_limit_events.store(7, Ordering::SeqCst);
    let p = p.with_import_patience(vec![std::time::Duration::from_millis(20); 2]);
    let outcome = p
        .import_records(vec![record("listed", "k", "v")])
        .await
        .expect("two pauses ride it out");
    assert_eq!((outcome.imported, outcome.failed), (1, 0), "{outcome:?}");
}

#[tokio::test]
async fn a_hosted_import_rides_out_a_rate_limit_window() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint).with_import_patience(vec![std::time::Duration::from_millis(20)]);
    // Each round is three quick attempts, so seven refusals outlast two
    // rounds (one pause) and are ridden out by a third (two pauses).
    state.rate_limit_experience.store(7, Ordering::SeqCst);
    let records = (0..3)
        .map(|i| record("patient", &format!("k{i}"), "v"))
        .collect();
    let error = p
        .import_records(records)
        .await
        .expect_err("one pause is not enough for seven refusals");
    assert!(matches!(error, MemoryError::Unavailable(_)), "{error:?}");

    state.rate_limit_experience.store(7, Ordering::SeqCst);
    let p = p.with_import_patience(vec![std::time::Duration::from_millis(20); 2]);
    let records = (0..3)
        .map(|i| record("patient", &format!("k{i}"), "v"))
        .collect();
    let outcome = p
        .import_records(records)
        .await
        .expect("two pauses ride it out");
    assert_eq!((outcome.imported, outcome.failed), (3, 0), "{outcome:?}");
}

#[tokio::test]
async fn a_record_the_backend_refuses_is_counted_not_fatal() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    state.fail_nth_experience.store(2, Ordering::SeqCst);
    let records = vec![
        record("mixed", "a", "first"),
        record("mixed", "b", "a secret the log must not see"),
        record("mixed", "c", "third"),
    ];
    let outcome = p.import_records(records).await.expect("import");
    assert_eq!((outcome.imported, outcome.failed), (2, 1), "{outcome:?}");
    let reason = outcome.errors.first().expect("a reason for the failure");
    assert!(reason.contains("rec-b"), "names the record: {reason}");
    assert!(
        reason.contains("VALIDATION_ERROR"),
        "names the code: {reason}"
    );
    assert!(!reason.contains("secret"), "never the content: {reason}");
}

#[tokio::test]
async fn a_namespace_no_hosted_scope_can_hold_is_refused_per_record() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let deep = (0..32)
        .map(|i| format!("s{i}"))
        .collect::<Vec<_>>()
        .join("/");
    let outcome = p
        .import_records(vec![record(&deep, "k", "v"), record("shallow", "k", "v")])
        .await
        .expect("import");
    assert_eq!((outcome.imported, outcome.failed), (1, 1), "{outcome:?}");
    assert_eq!(count_requests(&state, "POST /memory/experience"), 1);
}

#[tokio::test]
async fn an_account_out_of_credit_fails_the_import_batch() {
    let (endpoint, state) = hosted_backend().await;
    *state.fail_all.lock().expect("fail") = Some((402, "USER_INSUFFICIENT_CREDITS"));
    let error = provider(&endpoint)
        .import_records(vec![record("ns", "k", "v")])
        .await
        .expect_err("no record can be written");
    assert!(is_insufficient_credits(&error), "{error:?}");
}

#[tokio::test]
async fn a_partially_applied_conversation_completes_on_retry() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let messages = || -> Vec<IngestItem> {
        ["one", "two", "three"]
            .iter()
            .map(|text| {
                let mut m = item("thread-retry", text);
                m.author = Some("user".into());
                m
            })
            .collect()
    };
    state.fail_nth_experience.store(3, Ordering::SeqCst);
    p.ingest_conversation(messages())
        .await
        .expect_err("third message rejected");
    assert_eq!(state.log.lock().expect("log").events.len(), 2);

    state.fail_nth_experience.store(0, Ordering::SeqCst);
    let outcome = p
        .ingest_conversation(messages())
        .await
        .expect("the retry completes the batch");
    assert_eq!(
        outcome.written, 1,
        "only the missing message is new: {outcome:?}"
    );
    assert_eq!(state.log.lock().expect("log").events.len(), 3);
}

#[tokio::test]
async fn a_conversation_waits_on_its_last_event_only() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    let messages: Vec<IngestItem> = ["a", "b", "c", "d"]
        .iter()
        .map(|text| {
            let mut m = item("thread-wait", text);
            m.author = Some("user".into());
            m
        })
        .collect();
    p.ingest_conversation(messages).await.expect("conversation");
    let seen = state.seen.lock().expect("seen");
    let listings = seen
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /memory/events"))
        .count();
    assert_eq!(
        listings, 1,
        "one visibility wait for the whole batch: {:?}",
        seen.requests
    );
}

#[tokio::test]
async fn a_429_while_waiting_for_visibility_does_not_fail_the_write() {
    let (endpoint, state) = hosted_backend().await;
    // Each listing is retried three times inside the transport, so four 429s
    // exhaust the first poll and leak into the second.
    state.rate_limit_events.store(4, Ordering::SeqCst);
    provider(&endpoint)
        .ingest_document(item("doc-429", "rate limited while waiting"))
        .await
        .expect("the write was accepted; keep waiting until the deadline");
}

#[tokio::test]
async fn a_full_default_page_of_scopes_is_refused_not_silently_truncated() {
    let (endpoint, state) = hosted_backend().await;
    let p = provider(&endpoint);
    for i in 0..51 {
        p.store(
            &format!("ns{i}"),
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    }
    state.ignore_scope_limit.store(true, Ordering::SeqCst);
    let error = p
        .namespaces()
        .await
        .expect_err("a 50-entry page may be a truncation");
    assert!(matches!(error, MemoryError::Backend(_)), "{error:?}");
    assert!(error.to_string().contains("truncated"), "{error}");
}

#[tokio::test]
async fn scopes_are_all_returned_once_the_backend_honours_limit() {
    let (endpoint, _state) = hosted_backend().await;
    let p = provider(&endpoint);
    for i in 0..51 {
        p.store(
            &format!("ns{i}"),
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    }
    assert_eq!(p.namespaces().await.expect("all 51").len(), 51);
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

#[tokio::test]
async fn a_token_that_is_not_a_valid_header_is_unauthorized_without_a_request() {
    let (endpoint, state) = hosted_backend().await;
    let p = tinyhumans_provider(
        &endpoint,
        Arc::new(StaticBearer::new("abc\r\nX-Injected: 1")),
    )
    .expect("builds");
    let error = p.namespaces().await.expect_err("CR/LF must be refused");
    assert!(matches!(error, MemoryError::Unauthorized(_)), "{error:?}");
    assert!(!error.to_string().contains("X-Injected"), "{error}");
    assert!(state.seen.lock().expect("seen").requests.is_empty());
}

#[tokio::test]
async fn a_success_envelope_without_data_or_with_success_false_is_an_error() {
    let missing = Router::new().route(
        "/memory/scopes",
        get(|| async { Json(json!({ "success": true })) }),
    );
    let error = provider(&serve(missing).await)
        .namespaces()
        .await
        .expect_err("no data field");
    assert!(matches!(error, MemoryError::Backend(_)), "{error:?}");

    // A 2xx health probe whose body says `success:false` is not healthy.
    let refused = Router::new().route(
        "/memory/scopes",
        get(|| async {
            Json(json!({ "success": false, "error": "no", "errorCode": "VALIDATION_ERROR" }))
        }),
    );
    let health = provider(&serve(refused).await).health().await;
    assert!(!matches!(health, MemoryHealth::Ready), "{health:?}");
}

#[tokio::test]
async fn a_500_is_retryable_unavailable() {
    let (endpoint, state) = hosted_backend().await;
    *state.fail_all.lock().expect("fail") = Some((500, "INTERNAL"));
    let error = provider(&endpoint).namespaces().await.expect_err("500");
    assert!(matches!(error, MemoryError::Unavailable(_)), "{error:?}");
    let attempts = state.seen.lock().expect("seen").requests.len();
    assert_eq!(attempts, 3, "a read is retried on 500");
}
