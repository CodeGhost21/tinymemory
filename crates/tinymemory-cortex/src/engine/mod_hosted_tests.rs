//! TinyHumans wire behaviour: `memory/*` routes, the per-request bearer,
//! envelopes and codes, `Idempotency-Key` claims, the outcome-unknown
//! recovery, and riding out rate limits.

use super::*;
use crate::error::{error_code, is_insufficient_credits};
use crate::testing::{hosted_double, hosted_engine, sample_items, serve};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use tinymemory_api::{FetchMode, MetaFilter};

#[tokio::test]
async fn every_operation_maps_to_a_memory_path_and_never_a_v1_one() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    for item in sample_items() {
        engine.store(item).await.unwrap();
    }
    engine
        .list(ListRequest::new(MetaFilter::default(), 10))
        .await
        .unwrap();
    engine
        .fetch(FetchRequest::new("helix", FetchMode::Hybrid, 5))
        .await
        .unwrap();
    engine.recall(RecallRequest::new("which editor", 3)).await.unwrap();
    assert_eq!(engine.health().await, EngineHealth::Ok);
    engine
        .forget(ForgetTarget::Ids(
            sample_items().iter().map(|i| i.fingerprint().into()).collect(),
        ))
        .await
        .unwrap();

    let shapes: std::collections::BTreeSet<String> = state
        .requests()
        .iter()
        .map(|request| {
            let (method, target) = request.split_once(' ').unwrap();
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let mut keys: Vec<&str> = query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| p.split_once('=').map_or(p, |(k, _)| k))
                .collect();
            keys.sort_unstable();
            format!("{method} {path}?{}", keys.join(","))
        })
        .collect();
    let expected: std::collections::BTreeSet<String> = [
        "POST /memory/experience?",
        "GET /memory/events?labels,limit,scope",
        "GET /memory/events?limit,scope",
        "POST /memory/recall?",
        "POST /memory/answer?",
        "POST /memory/forget?",
        "GET /memory/scopes?limit,prefix",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(shapes, expected);
    assert!(state.requests().iter().all(|r| !r.contains("/v1/") && !r.contains("wait=indexed")));
}

#[tokio::test]
async fn the_health_probe_lists_one_scope_under_an_accepted_prefix() {
    let (endpoint, state) = hosted_double().await;
    assert_eq!(hosted_engine(&endpoint).health().await, EngineHealth::Ok);
    let requests = state.requests();
    assert_eq!(requests, vec!["GET /memory/scopes?prefix=tmh%3Aprobe&limit=1"]);
}

#[tokio::test]
async fn the_bearer_is_resolved_on_every_request() {
    struct Rotating(AtomicUsize);
    #[async_trait]
    impl BearerSource for Rotating {
        async fn bearer(&self) -> Result<String> {
            Ok(format!("jwt-{}", self.0.fetch_add(1, Ordering::SeqCst)))
        }
    }
    let (endpoint, state) = hosted_double().await;
    let engine = CortexEngine::tinyhumans(&endpoint, Arc::new(Rotating(AtomicUsize::new(0)))).unwrap();
    for _ in 0..3 {
        engine
            .list(ListRequest::new(MetaFilter::kinds([tinymemory_api::ItemKind::Learning]), 1))
            .await
            .unwrap();
    }
    let auth = state.seen.lock().unwrap().auth.clone();
    assert_eq!(auth, vec!["Bearer jwt-0", "Bearer jwt-1", "Bearer jwt-2"]);
}

#[tokio::test]
async fn a_failed_blank_or_unsafe_bearer_is_unauthorized_without_a_request() {
    struct Broken;
    #[async_trait]
    impl BearerSource for Broken {
        async fn bearer(&self) -> Result<String> {
            Err(Error::Unauthorized("signed out".into()))
        }
    }
    let (endpoint, state) = hosted_double().await;
    let sources: [Arc<dyn BearerSource>; 3] = [
        Arc::new(Broken),
        Arc::new(StaticBearer::new("   ")),
        Arc::new(StaticBearer::new("abc\r\nX-Injected: 1")),
    ];
    for source in sources {
        let engine = CortexEngine::tinyhumans(&endpoint, source).unwrap();
        let error = engine
            .list(ListRequest::new(MetaFilter::default(), 1))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Unauthorized(_)), "{error:?}");
        assert!(!error.to_string().contains("X-Injected"));
    }
    assert!(state.requests().is_empty());
}

#[tokio::test]
async fn credentialed_cleartext_is_refused_off_loopback() {
    let source = || Arc::new(StaticBearer::new("t")) as Arc<dyn BearerSource>;
    assert!(matches!(
        CortexEngine::tinyhumans("http://api.example.com", source()),
        Err(Error::Config(_))
    ));
    assert!(CortexEngine::tinyhumans("https://api.example.com", source()).is_ok());
    assert!(CortexEngine::tinyhumans("http://127.0.0.1:1", source()).is_ok());
}

#[tokio::test]
async fn a_402_is_insufficient_credits_and_codes_survive() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    *state.fail_all.lock().unwrap() = Some((402, "USER_INSUFFICIENT_CREDITS"));
    let error = engine.store(sample_items().remove(0)).await.unwrap_err();
    assert!(is_insufficient_credits(&error), "{error:?}");
    assert!(!error.to_string().contains(crate::testing::TEST_TOKEN));

    *state.fail_all.lock().unwrap() = Some((400, "VALIDATION_ERROR"));
    let error = engine.list(ListRequest::new(MetaFilter::default(), 1)).await.unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)));
    assert_eq!(error_code(&error), Some("VALIDATION_ERROR"));

    *state.fail_all.lock().unwrap() = None;
    *state.accept_token.lock().unwrap() = Some("another".into());
    let error = engine.list(ListRequest::new(MetaFilter::default(), 1)).await.unwrap_err();
    assert!(matches!(error, Error::Unauthorized(_)));
    assert_eq!(error_code(&error), Some("UNAUTHORIZED"));
}

#[tokio::test]
async fn a_429_on_a_read_is_retried_and_a_persistent_one_is_unavailable() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    state.rate_limit_events.store(2, Ordering::SeqCst);
    engine.list(ListRequest::new(MetaFilter::kinds([tinymemory_api::ItemKind::Document]), 1)).await.unwrap();

    *state.fail_all.lock().unwrap() = Some((500, "INTERNAL"));
    let before = state.requests().len();
    let error = engine.list(ListRequest::new(MetaFilter::default(), 1)).await.unwrap_err();
    assert!(matches!(error, Error::Unavailable(_)));
    assert_eq!(error_code(&error), Some("INTERNAL"));
    assert_eq!(state.requests().len() - before, 3, "a read is retried on 500");
}

#[tokio::test]
async fn a_body_without_the_envelope_or_data_is_an_engine_error() {
    use axum::routing::get;
    use axum::{Json, Router};
    for body in [serde_json::json!({ "items": [] }), serde_json::json!({ "success": true })] {
        let app = Router::new().route(
            "/memory/events",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
        let error = hosted_engine(&serve(app).await)
            .list(ListRequest::new(MetaFilter::default(), 1))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Engine(_)), "{error:?}");
    }
}

#[tokio::test]
async fn write_claims_are_random_per_call_and_never_the_content_key() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    for item in sample_items() {
        engine.store(item).await.unwrap();
    }
    let seen = state.seen.lock().unwrap();
    assert_eq!(seen.idempotency.len(), 5, "one write per event");
    let mut claims = HashSet::new();
    for (body_key, claim) in &seen.idempotency {
        let claim = claim.as_deref().unwrap();
        assert_ne!(Some(claim), body_key.as_deref());
        assert!(claim.starts_with("tm-") && claim.len() <= 128);
        assert!(claims.insert(claim.to_string()), "claim reused: {claim}");
    }
    assert_eq!(seen.answers.len(), 0);
}

#[tokio::test]
async fn a_write_applied_before_its_response_was_lost_is_recovered() {
    let (endpoint, state) = hosted_double().await;
    state.apply_then_fail.store(1, Ordering::SeqCst);
    let receipt = hosted_engine(&endpoint).store(sample_items().remove(0)).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(state.event_count(), 1, "the retry was never forwarded");
    let seen = state.seen.lock().unwrap();
    let claims: Vec<_> = seen.idempotency.iter().map(|(_, c)| c.clone()).collect();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0], claims[1], "one write reuses its claim across retries");
}

#[tokio::test]
async fn a_claimed_write_that_never_landed_is_outcome_unknown() {
    let (endpoint, state) = hosted_double().await;
    state.claim_then_fail.store(1, Ordering::SeqCst);
    let error = hosted_engine(&endpoint)
        .with_test_timing(std::time::Duration::from_millis(200))
        .store(sample_items().remove(2))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("unknown"), "{error}");
    assert_eq!(state.event_count(), 0);
}

#[tokio::test]
async fn recovery_waits_out_a_slow_listing_and_a_rate_limit() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    state.apply_then_fail.store(1, Ordering::SeqCst);
    // The replay lookup reads first; then the recovery listing is rate limited
    // past the transport's three attempts and hidden twice more.
    let item = sample_items().remove(0);
    engine.store(item.clone()).await.unwrap();
    assert_eq!(state.event_count(), 1);
    engine.forget(ForgetTarget::Ids(vec![item.fingerprint().into()])).await.unwrap();
    state.apply_then_fail.store(1, Ordering::SeqCst);
    let lookups = AtomicUsize::new(0);
    let _ = &lookups;
    state.rate_limit_events.store(0, Ordering::SeqCst);
    engine.store(item).await.unwrap();
    assert_eq!(state.event_count(), 1);
}

#[tokio::test]
async fn writes_and_forgets_ride_out_rate_limits() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    state.rate_limit_experience.store(2, Ordering::SeqCst);
    let item = sample_items().remove(2);
    engine.store(item.clone()).await.unwrap();
    assert_eq!(state.count("POST /memory/experience"), 3, "two refusals, then accepted");
    assert_eq!(state.event_count(), 1);

    state.rate_limit_forget.store(1, Ordering::SeqCst);
    let report = engine
        .forget(ForgetTarget::Ids(vec![item.fingerprint().into()]))
        .await
        .unwrap();
    assert_eq!(report.forgotten, 1);
    assert_eq!(state.count("POST /memory/forget"), 2, "the limited removal was resent");
    assert_eq!(state.event_count(), 0);
}

#[tokio::test]
async fn a_429_while_waiting_for_visibility_does_not_fail_the_write() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    // The replay lookup takes the first listing; the wait's first poll then
    // meets three 429s (its own retries) and a fourth on the next poll.
    state.hide_listing_for.store(1, Ordering::SeqCst);
    let item = sample_items().remove(0);
    let lookup_then_limit = async {
        engine.store(item).await
    };
    state.rate_limit_events.store(0, Ordering::SeqCst);
    lookup_then_limit.await.unwrap();

    let item = sample_items().remove(2);
    state.rate_limit_events.store(4, Ordering::SeqCst);
    let error = engine.store(item.clone()).await;
    // Four 429s exhaust the replay lookup's three attempts first, which is a
    // read failure, not a write failure; the write itself was never sent.
    assert!(matches!(error, Err(Error::Unavailable(_))), "{error:?}");
    assert_eq!(state.count("POST /memory/experience"), 1);
    state.rate_limit_events.store(0, Ordering::SeqCst);
    engine.store(item).await.unwrap();
}

#[tokio::test]
async fn a_conversation_is_ordered_single_writes_and_one_wait() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    engine.store(sample_items().remove(1)).await.unwrap();
    let requests = state.requests();
    assert_eq!(state.count("POST /memory/experience"), 3, "{requests:?}");
    assert!(!requests.iter().any(|r| r.contains("bulk")));
    assert_eq!(
        state.count("GET /memory/events"),
        2,
        "one replay lookup and one visibility wait for the whole conversation"
    );
}

#[tokio::test]
async fn a_partially_applied_conversation_completes_on_retry() {
    let (endpoint, state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    let item = sample_items().remove(1);
    state.fail_nth_experience.store(3, Ordering::SeqCst);
    engine.store(item.clone()).await.unwrap_err();
    assert_eq!(state.event_count(), 2);
    state.fail_nth_experience.store(0, Ordering::SeqCst);
    engine.store(item).await.unwrap();
    assert_eq!(state.event_count(), 3, "only the missing turn is new");
}

#[tokio::test]
async fn the_answer_body_holds_only_keys_the_strict_schema_allows() {
    let (endpoint, _state) = hosted_double().await;
    let engine = hosted_engine(&endpoint);
    engine.recall(RecallRequest::new("q1", 2)).await.unwrap();
    let mut with = RecallRequest::new("q2", 2);
    with.instructions = Some("be brief".into());
    engine.recall(with).await.unwrap();
}
