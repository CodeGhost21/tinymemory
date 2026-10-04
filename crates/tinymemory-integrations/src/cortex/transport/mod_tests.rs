//! Tests for the transport: endpoint policy, the credential header, retry
//! split, body caps and transport classification.

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::cortex::testing::serve;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::any;

fn client(endpoint: &str) -> HttpClient {
    let mut client = HttpClient::new(
        CortexWire::Direct,
        endpoint,
        CortexCredential::api_key("ctx_key"),
    )
    .unwrap();
    client.set_read_backoff(Duration::from_millis(1));
    client
}

#[test]
fn credentialed_cleartext_is_refused_except_on_loopback() {
    let key = || CortexCredential::api_key("k");
    for refused in ["http://memory.example.com", "ftp://x", "not a url"] {
        assert!(
            matches!(
                HttpClient::new(CortexWire::Direct, refused, key()),
                Err(Error::Config(_))
            ),
            "{refused}"
        );
    }
    for allowed in [
        "https://memory.example.com",
        "http://127.0.0.1:3141",
        "http://[::1]:3141",
        "http://localhost:3141",
    ] {
        assert!(
            HttpClient::new(CortexWire::TinyHumans, allowed, key()).is_ok(),
            "{allowed}"
        );
    }
}

#[test]
fn a_blank_static_key_is_a_configuration_error() {
    assert!(matches!(
        HttpClient::new(
            CortexWire::Direct,
            "https://x",
            CortexCredential::api_key("  ")
        ),
        Err(Error::Config(_))
    ));
}

#[test]
fn the_credential_header_is_sensitive_and_unchanged() {
    let header = credential_header("ctx_secret").unwrap();
    assert!(header.is_sensitive());
    assert_eq!(header.as_bytes(), b"Bearer ctx_secret");
}

#[test]
fn a_token_that_cannot_be_a_header_is_unauthorized_and_not_echoed() {
    let error = credential_header("supersecret\r\nX-Injected: 1").unwrap_err();
    assert!(matches!(error, Error::Unauthorized(_)), "{error:?}");
    let rendered = format!("{error:?}");
    assert!(!rendered.contains("supersecret") && !rendered.contains("X-Injected"));
    assert!(matches!(
        credential_header("   "),
        Err(Error::Unauthorized(_))
    ));
}

#[tokio::test]
async fn a_built_request_carries_a_sensitive_authorization() {
    let request = client("https://example.test")
        .request(Method::GET, "v1/events")
        .await
        .unwrap()
        .build()
        .unwrap();
    let auth = request.headers().get(AUTHORIZATION).unwrap();
    assert!(auth.is_sensitive());
    assert!(!format!("{:?}", request.headers()).contains("ctx_key"));
}

#[test]
fn every_write_key_is_fresh_and_names_its_process() {
    let keys: std::collections::HashSet<String> =
        (0..1000).map(|_| fresh_idempotency_key()).collect();
    assert_eq!(keys.len(), 1000);
    let salt = |k: &str| k.split('-').nth(1).map(str::to_string);
    assert_eq!(
        salt(&fresh_idempotency_key()),
        salt(&fresh_idempotency_key())
    );
    assert!(fresh_idempotency_key().starts_with("tm-"));
}

#[test]
fn urlencoding_escapes_everything_a_cursor_could_reshape() {
    assert_eq!(
        urlencode("app:tinymemory/app:documents"),
        "app%3Atinymemory%2Fapp%3Adocuments"
    );
    assert_eq!(urlencode("a+b&c=d#e?f"), "a%2Bb%26c%3Dd%23e%3Ff");
    assert_eq!(urlencode("Az09-._~"), "Az09-._~");
    assert_eq!(urlencode("é"), "%C3%A9");
}

/// A server answering every request with `status`, counting hits. It has no
/// `whoami` (like a server before the actor model), which is not counted.
async fn counting(status: StatusCode) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new()
        .route("/v1/auth/whoami", any(|| async { StatusCode::NOT_FOUND }))
        .fallback(any(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (status, "busy")
            }
        }));
    (serve(app).await, hits)
}

#[tokio::test]
async fn transient_failures_retry_reads_three_times_and_writes_once() {
    let (endpoint, hits) = counting(StatusCode::SERVICE_UNAVAILABLE).await;
    let c = client(&endpoint);
    let read = c
        .json(Method::GET, "v1/events", None, Attempts::RetryTransient)
        .await;
    assert!(matches!(read, Err(Error::Unavailable(_))), "{read:?}");
    assert_eq!(
        hits.swap(0, Ordering::SeqCst),
        3,
        "a read retries to the cap"
    );

    let body = serde_json::json!({});
    let write = c
        .json(Method::POST, "v1/experience", Some(&body), Attempts::Once)
        .await;
    assert!(matches!(write, Err(Error::Unavailable(_))), "{write:?}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a write is sent exactly once"
    );
}

#[tokio::test]
async fn a_settled_refusal_is_not_retried() {
    let (endpoint, hits) = counting(StatusCode::UNAUTHORIZED).await;
    let error = client(&endpoint)
        .json(Method::GET, "v1/events", None, Attempts::RetryTransient)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Unauthorized(_)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_endless_error_body_is_capped_rather_than_buffered() {
    use axum::body::Body;
    use axum::http::Response;
    let app = Router::new().fallback(any(|| async {
        let endless = futures::stream::repeat_with(|| {
            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(&[b'x'; 8192]))
        });
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from_stream(endless))
            .unwrap()
    }));
    let endpoint = serve(app).await;
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        client(&endpoint).json(Method::GET, "v1/events", None, Attempts::Once),
    )
    .await
    .expect("an endless error body was buffered instead of capped");
    assert!(
        matches!(outcome, Err(Error::InvalidRequest(_))),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_success_body_over_the_cap_is_refused() {
    let app = Router::new().fallback(any(|| async { "y".repeat(4096) }));
    let endpoint = serve(app).await;
    let response = reqwest::get(format!("{endpoint}/x")).await.unwrap();
    let error = body::read_limited(response, "v1/x", 1024)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Engine(_)));
    assert!(error.to_string().contains("limit"), "{error}");
}

#[tokio::test]
async fn an_unreachable_endpoint_is_unavailable_and_names_the_class() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let error = client(&endpoint)
        .json(Method::GET, "v1/events", None, Attempts::Once)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("could not connect"), "{error}");
    assert!(!error.to_string().contains("ctx_key"));
}

/// A CortexDB that, like its cloud, serves only requests naming the token's
/// actor; it counts `whoami` lookups.
async fn actor_checking(caller: &'static str) -> (String, Arc<AtomicUsize>) {
    use axum::http::HeaderMap;
    use axum::routing::get;
    let lookups = Arc::new(AtomicUsize::new(0));
    let counter = lookups.clone();
    let app = Router::new()
        .route(
            "/v1/auth/whoami",
            get(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({ "caller": caller, "tenant_id": "t" }))
                }
            }),
        )
        .fallback(any(move |headers: HeaderMap| async move {
            match headers.get("x-cortex-actor").and_then(|v| v.to_str().ok()) {
                Some(actor) if actor == caller => (StatusCode::OK, "{}".to_string()),
                _ => (
                    StatusCode::UNAUTHORIZED,
                    r#"{"error_code":"ACTOR_MISMATCH"}"#.to_string(),
                ),
            }
        }));
    (serve(app).await, lookups)
}

#[tokio::test]
async fn the_direct_wire_names_the_whoami_actor_and_asks_once() {
    let (endpoint, lookups) = actor_checking("user:u_123").await;
    let c = client(&endpoint);
    for _ in 0..3 {
        c.json(Method::GET, "v1/events", None, Attempts::RetryTransient)
            .await
            .expect("served as the token's actor");
    }
    let body = serde_json::json!({});
    c.clone()
        .json(Method::POST, "v1/experience", Some(&body), Attempts::Once)
        .await
        .expect("a clone reuses the learned actor");
    assert_eq!(lookups.load(Ordering::SeqCst), 1, "whoami is asked once");
}

#[tokio::test]
async fn a_rejected_credential_makes_the_next_request_ask_again() {
    let (endpoint, lookups) = actor_checking("user:u_123").await;
    let c = client(&endpoint);
    c.json(Method::GET, "v1/events", None, Attempts::RetryTransient)
        .await
        .unwrap();
    c.actor
        .learn(StatusCode::OK, br#"{"caller":"user:somebody_else"}"#);
    let refused = c
        .json(Method::GET, "v1/events", None, Attempts::RetryTransient)
        .await;
    assert!(
        matches!(refused, Err(Error::Unauthorized(_))),
        "{refused:?}"
    );
    c.json(Method::GET, "v1/events", None, Attempts::RetryTransient)
        .await
        .expect("the actor is learned again after the refusal");
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn the_hosted_wire_sends_no_actor() {
    let (endpoint, lookups) = actor_checking("user:u_123").await;
    let hosted = HttpClient::new(
        CortexWire::TinyHumans,
        &endpoint,
        CortexCredential::api_key("ctx_key"),
    )
    .unwrap();
    let refused = hosted
        .json(Method::GET, "memory/events", None, Attempts::Once)
        .await;
    assert!(
        matches!(refused, Err(Error::Unauthorized(_))),
        "{refused:?}"
    );
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        0,
        "the backend names the actor"
    );
}
