//! What the hosted adapters do when the backend does not cooperate.
//!
//! Issue #18 §E6: "backend error, timeout, and partial-page responses on every
//! remote adapter — currently zero coverage". The existing per-adapter tests all
//! drive a backend that answers correctly, which is the half that was never in
//! doubt.
//!
//! Since §A4 landed, the assertion is two-fold: a failure comes back **at
//! all**, and — where the class is knowable — it comes back **typed**: a 401
//! downcasts to `Unauthorized`, a 500 to `Backend`, a dead port to
//! `Unreachable`, so a caller can act on the class instead of parsing prose.
//!
//! A read that answers `Ok(None)` when the backend returned 500 is saying "this
//! memory does not exist" when the truth is "I could not ask". A caller cannot
//! tell those apart, so it writes the memory again, or reports to a user that
//! their memory is gone, or — worst — a sync job treats the empty read as
//! authoritative and prunes. Nothing surfaces until much later, which is exactly
//! the failure mode that keeps `OPENCOMPANY_MEMORY=remote` gated downstream.
//!
//! Each test drives a real adapter over a real TCP socket against a double that
//! misbehaves in one specific way, matching the harness the happy-path tests
//! already use.

#![allow(clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use axum::routing::{any, get};
use axum::Router;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::MemoryProvider;
use tinymemory_api::recall::RecallOpts;
use tinymemory_api::traits::Memory;
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use crate::{CogneeMemory, Mem0Memory, SupermemoryMemory};

/// Serves `app` on an ephemeral port and returns its base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    endpoint
}

/// A backend that fails every route with `status`.
async fn failing(status: StatusCode) -> String {
    serve(Router::new().fallback(any(move || async move { status }))).await
}

/// A backend that fails every route with `status` and a body carrying a
/// fake secret, counting requests — for the redaction and retry-count
/// assertions.
async fn failing_with_body(
    status: StatusCode,
    body: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicU32>) {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = hits.clone();
    let endpoint = serve(Router::new().fallback(any(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (status, body)
        }
    })))
    .await;
    (endpoint, hits)
}

/// A backend that answers every route with `200 OK` and a body that is not the
/// JSON the adapter expects.
///
/// Distinct from an HTTP failure: the transport succeeded, so an adapter that
/// only checks the status code reaches its deserializer with rubbish.
async fn malformed() -> String {
    serve(Router::new().fallback(any(|| async { "this is not the JSON you asked for" }))).await
}

/// Every adapter, as a `Memory`, built against `endpoint`.
///
/// Boxed rather than generic so each assertion below is written once and run
/// three times — the point is that no adapter is exempt.
fn adapters(endpoint: &str) -> Vec<(&'static str, Box<dyn Memory>)> {
    vec![
        (
            "supermemory",
            Box::new(SupermemoryMemory::new(endpoint, None).expect("client")) as Box<dyn Memory>,
        ),
        (
            "mem0",
            Box::new(Mem0Memory::new(endpoint, None).expect("client")),
        ),
        (
            "cognee",
            Box::new(CogneeMemory::self_hosted(endpoint, None).expect("client")),
        ),
    ]
}

#[tokio::test]
async fn a_backend_failure_on_write_is_reported_rather_than_swallowed() {
    let endpoint = failing(StatusCode::INTERNAL_SERVER_ERROR).await;
    for (name, memory) in adapters(&endpoint) {
        let result = memory
            .store_with_taint(
                "ns",
                "k",
                "content",
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await;
        let error = result.expect_err("a 500 on write must surface");
        assert!(
            matches!(
                error.downcast_ref::<MemoryError>(),
                Some(MemoryError::Backend(_))
            ),
            "{name}: a 500 must arrive typed as Backend, got: {error}"
        );
    }
}

#[tokio::test]
async fn a_backend_failure_on_read_is_not_reported_as_absence() {
    // The one that matters most. `Ok(None)` here means "no such memory", and
    // the truth is "the backend is down".
    let endpoint = failing(StatusCode::INTERNAL_SERVER_ERROR).await;
    for (name, memory) in adapters(&endpoint) {
        let result = memory.get("ns", "k").await;
        let Err(error) = result else {
            panic!(
                "{name}: a 500 on read must not be laundered into `Ok(None)` — \
                 'I could not ask' and 'it is not there' are different answers"
            );
        };
        // Assert the failure is the *backend's*, not something incidental like a
        // malformed URL. Without this the test would pass for the wrong reason
        // if the adapter never reached the network at all.
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("500"),
            "{name}: expected the backend status to survive into the error, got: {rendered}"
        );
    }
}

#[tokio::test]
async fn an_unauthorized_backend_is_not_reported_as_an_empty_store() {
    // A wrong or expired credential is the most likely failure in production,
    // and the most dangerous one to render as "you have no memories".
    let endpoint = failing(StatusCode::UNAUTHORIZED).await;
    for (name, memory) in adapters(&endpoint) {
        let listed = memory.list(None, None, None).await;
        let error = listed.expect_err("a 401 must not present as an empty result set");
        assert!(
            matches!(
                error.downcast_ref::<MemoryError>(),
                Some(MemoryError::Unauthorized(_))
            ),
            "{name}: a 401 must arrive typed as Unauthorized, got: {error}"
        );

        let recalled = memory.recall("anything", 10, RecallOpts::default()).await;
        assert!(
            recalled.is_err(),
            "{name}: a 401 on recall must not present as no matches"
        );
    }
}

#[tokio::test]
async fn a_malformed_backend_response_is_an_error_and_not_a_panic() {
    // `200 OK` with a body the adapter cannot parse. An adapter that unwraps
    // its way through deserialization takes the caller's process down.
    let endpoint = malformed().await;
    for (name, memory) in adapters(&endpoint) {
        let result = memory.get("ns", "k").await;
        assert!(
            result.is_err(),
            "{name}: an unparseable 200 body must surface as an error"
        );
    }
}

#[tokio::test]
async fn an_unreachable_backend_is_reported_rather_than_hanging() {
    // Nothing is listening. This is the timeout/connection-refused leg of §E6.
    // Bind a port, learn its number, drop the listener: the address is now
    // reliably closed rather than merely unlikely to be in use.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);

    for (name, memory) in adapters(&endpoint) {
        let result = memory.get("ns", "k").await;
        assert!(
            result.is_err(),
            "{name}: an unreachable backend must surface as an error"
        );
    }
}

#[tokio::test]
async fn a_cursor_that_never_clears_is_refused_rather_than_walked_for_ever() {
    // Mem0's hosted arm pages until the server says stop: an empty page or a
    // null `next`. Both are things the *server* controls, so a server that
    // keeps answering a page and a cursor -- a bug, a proxy replaying one
    // response, a filter that never narrows -- would spin the request loop and
    // grow the buffer until the process died. The self-hosted arm already
    // refuses past its ceiling; this pins the hosted one doing the same.
    let app = Router::new().fallback(any(|| async {
        axum::Json(serde_json::json!({
            "count": 1,
            "next": "https://api.mem0.ai/v3/memories/?page=2",
            "previous": null,
            "results": [{"id": "m-1", "memory": "x", "metadata": {}}]
        }))
    }));
    let endpoint = serve(app).await;
    let memory = Mem0Memory::api(&endpoint, "m0-test-key").expect("client");

    // Bounded so a genuinely unbounded loop fails the test rather than hanging
    // the suite: the ceiling is 500 requests against a local socket, which
    // finishes far inside this. `count` is the walker now — issue #69 made
    // the keyed `get` a single filtered request, so a poisoned cursor cannot
    // spin it any more; the whole-store walk is where the ceiling lives.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(60), memory.count()).await;

    let Ok(result) = outcome else {
        panic!("the hosted listing never terminated against a cursor that never clears");
    };
    let error = result.expect_err("a cursor that never clears cannot be answered correctly");
    assert!(
        format!("{error:#}").contains("pages"),
        "the refusal must name the page ceiling it hit, got: {error:#}"
    );
}

/// Supermemory's twin of the guard above (issue #75): its per-tag pager
/// loops on server-supplied `totalPages`, which is exactly as
/// server-controlled as mem0's cursor — a value that never lets the walk
/// finish must be refused, not walked for ever.
#[tokio::test]
async fn a_total_pages_that_never_lets_the_walk_finish_is_refused() {
    use axum::routing::{get, post};
    let app = Router::new()
        .route(
            "/v3/container-tags/list",
            get(|| async { axum::Json(serde_json::json!([{"containerTag": "tinymemory:tm_x"}])) }),
        )
        .route(
            "/v4/memories/list",
            post(|| async {
                axum::Json(serde_json::json!({
                    "memoryEntries": [{"id": "sm-1", "memory": "x", "metadata": {}}],
                    "pagination": {"totalPages": 1_000_000}
                }))
            }),
        );
    let endpoint = serve(app).await;
    let memory = SupermemoryMemory::api(&endpoint, "sm-test-key").expect("client");

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(60), memory.count()).await;
    let Ok(result) = outcome else {
        panic!("the per-tag walk never terminated against a lying totalPages");
    };
    let error = result.expect_err("a totalPages that never clears cannot be answered correctly");
    assert!(
        format!("{error:#}").contains("pages"),
        "the refusal must name the page ceiling it hit, got: {error:#}"
    );
}

/// Issue #75: non-2xx bodies are read through the 64 KiB error-body cap, not
/// `Response::text()`'s unbounded buffer. A hostile endpoint answering every
/// request with a 500 and a body that never ends must cost a bounded read and
/// a prompt typed error — not a buffer that grows until the process dies.
#[tokio::test]
async fn an_endless_error_body_is_capped_rather_than_buffered() {
    use axum::body::Body;
    use axum::http::Response;
    use futures::stream;

    let app = Router::new().fallback(any(|| async {
        let endless = stream::repeat_with(|| {
            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(&[b'x'; 8192]))
        });
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_stream(endless))
            .expect("response")
    }));
    let endpoint = serve(app).await;
    let memory = Mem0Memory::self_hosted(&endpoint, None).expect("client");

    // Bounded: with the cap, the read stops at 64 KiB and the typed error
    // surfaces immediately; reverting to `text()` hangs here accumulating
    // the stream until timeout or OOM.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), memory.count()).await;
    let Ok(result) = outcome else {
        panic!("an endless error body was buffered instead of capped");
    };
    let error = result.expect_err("a 500 must surface as an error");
    assert!(
        format!("{error:#}").contains("500"),
        "the status error surfaces despite the endless body: {error:#}"
    );
}

#[tokio::test]
async fn a_paginated_export_terminates_instead_of_looping() {
    // The partial-page leg of §E6. A backend that keeps answering with a page
    // and a cursor would spin an exporter forever; the contract terminates on
    // `next_cursor: None`, and a driver that never emits one never finishes.
    //
    // Driven through the bound provider rather than the raw `Memory`:
    // `export_page` is a `MemoryPortability` method, and portability is a
    // mandatory supertrait of `MemoryProvider`, so it is always callable — which
    // is exactly why a non-terminating one is worth pinning.
    let app = Router::new().fallback(get(|| async {
        axum::Json(serde_json::json!({
            "memoryEntries": [],
            "results": [],
            "data": [],
            "pagination": {"totalPages": 1}
        }))
    }));
    let endpoint = serve(app).await;

    let providers: Vec<(&str, Box<dyn MemoryProvider>)> = vec![
        (
            "supermemory",
            Box::new(crate::supermemory_provider(
                SupermemoryMemory::new(&endpoint, None).expect("client"),
            )) as Box<dyn MemoryProvider>,
        ),
        (
            "mem0",
            Box::new(crate::mem0_provider(
                Mem0Memory::new(&endpoint, None).expect("client"),
            )),
        ),
        (
            "cognee",
            Box::new(crate::cognee_provider(
                CogneeMemory::self_hosted(&endpoint, None).expect("client"),
            )),
        ),
    ];

    for (name, provider) in providers {
        // Bounded so a non-terminating implementation fails rather than hanging
        // the whole suite.
        let finished = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut cursor: Option<String> = None;
            for page_number in 0..100usize {
                let Ok(page) = provider.export_page(cursor.as_deref(), 100).await else {
                    // An error is an acceptable answer here; a hang is not.
                    return true;
                };
                match page.next_cursor {
                    None => return true,
                    Some(next) => cursor = Some(next),
                }
                let _ = page_number;
            }
            false
        })
        .await;
        assert_eq!(
            finished,
            Ok(true),
            "{name}: export_page never terminated — an exporter would spin here"
        );
    }
}

/// #68 review Major 1: deep health must be reachable through the PUBLIC
/// adapter types — the first cut implemented it on the inner composition and
/// every hand-delegating wrapper shadowed it with the trait default's `None`.
/// A 401 is `Down` naming the credential class; a 503 is `Degraded` (answered,
/// cannot serve). And per the review's redaction minor: the backend's error
/// body — which a vendor is free to fill with the rejected key — must NOT
/// reach the standing status reason.
#[tokio::test]
async fn public_adapters_probe_typed_health_with_redacted_reasons() {
    let (unauthorized, _) = failing_with_body(
        StatusCode::UNAUTHORIZED,
        r#"{"detail":"bad key sk-SECRET123"}"#,
    )
    .await;
    for (name, memory) in adapters(&unauthorized) {
        let health = memory
            .health_probe()
            .await
            .unwrap_or_else(|| panic!("{name}: the public type must forward health_probe"));
        assert_eq!(health.as_str(), "down", "{name}: a 401 is Down");
        let reason = health.reason().unwrap_or_default();
        assert!(
            reason.contains("credential"),
            "{name}: the reason names the class: {reason}"
        );
        assert!(
            !reason.contains("sk-SECRET123"),
            "{name}: the backend's body must not reach the status surface: {reason}"
        );
    }

    let (throttled, _) = failing_with_body(StatusCode::SERVICE_UNAVAILABLE, "busy").await;
    for (name, memory) in adapters(&throttled) {
        let health = memory
            .health_probe()
            .await
            .unwrap_or_else(|| panic!("{name}: the public type must forward health_probe"));
        assert_eq!(
            health.as_str(),
            "degraded",
            "{name}: answered-but-cannot-serve is Degraded, not Down"
        );
    }
}

/// #68 review Major 5: a backend-side validation refusal (HTTP 400) must
/// arrive as `Invalid` — the class the tightened conformance refusal
/// assertion demands — never as `Backend`.
#[tokio::test]
async fn a_400_refusal_is_invalid_not_backend() {
    let (endpoint, _) = failing_with_body(
        StatusCode::BAD_REQUEST,
        r#"{"error":"content must not be empty"}"#,
    )
    .await;
    for (name, memory) in adapters(&endpoint) {
        let error = memory
            .store_with_taint(
                "ns",
                "k",
                "content",
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await
            .expect_err("a 400 must surface");
        assert!(
            matches!(
                error.downcast_ref::<MemoryError>(),
                Some(MemoryError::Invalid(_))
            ),
            "{name}: a 400 must be Invalid, got: {error}"
        );
    }
}

/// Issue #75: the multipart UPLOAD leg speaks the same taxonomy. The generic
/// 400 test above never reaches it — cognee's preceding dataset GET fails
/// first against a fail-everything double — so this double lets the resolve
/// succeed and fails only the upload, pinning the one write path that used
/// to answer an untyped string.
#[tokio::test]
async fn a_400_on_the_multipart_upload_leg_is_invalid_not_other() {
    use axum::routing::{get, post};
    let app = Router::new()
        .route(
            "/api/v1/datasets/",
            get(|| async { axum::Json(serde_json::json!([])) }),
        )
        .route(
            "/api/v1/remember",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    r#"{"error":"file rejected"}"#.to_owned(),
                )
            }),
        );
    let endpoint = serve(app).await;
    let memory = CogneeMemory::self_hosted(&endpoint, None).expect("client");

    let error = memory
        .store_with_taint(
            "ns",
            "k",
            "content",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("the upload 400 must surface");
    assert!(
        matches!(
            error.downcast_ref::<MemoryError>(),
            Some(MemoryError::Invalid(_))
        ),
        "a multipart 400 must be Invalid, got: {error}"
    );
}

/// #68 review Major 4: the retry split is now a per-call statement. A 503 on
/// a retrying READ is attempted three times; the same 503 on a WRITE path
/// (`empty` — no marker, no retry machinery at all) is attempted once. The
/// counter is the proof, not a comment.
#[tokio::test]
async fn transient_failures_retry_reads_three_times_and_writes_once() {
    // Reads: every route 503s; the read path retries to its cap.
    let (endpoint, hits) = failing_with_body(StatusCode::SERVICE_UNAVAILABLE, "busy").await;
    let memory = SupermemoryMemory::api(&endpoint, "key").expect("client");
    let _ = memory.list(None, None, None).await;
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "a transient read failure retries to the cap"
    );

    // Writes: the LIST half of upsert succeeds (empty page — nothing to
    // update), so the create POST is the only thing that can fail. It must
    // reach the backend exactly once: the write path has no retry machinery
    // at all, and this counter — not a comment — is what pins the split
    // (#68 review, Major 4).
    let writes = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = writes.clone();
    let app = Router::new()
        .route(
            "/v4/memories/list",
            axum::routing::post(|| async { axum::Json(serde_json::json!({"memories": []})) }),
        )
        .fallback(any(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        }));
    let write_endpoint = serve(app).await;
    let memory = SupermemoryMemory::api(&write_endpoint, "key").expect("client");
    let error = memory
        .store_with_taint(
            "ns",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("the 503 write must surface");
    assert!(
        matches!(
            error.downcast_ref::<MemoryError>(),
            Some(MemoryError::Unavailable(_))
        ),
        "and typed: {error}"
    );
    assert_eq!(
        writes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a transient WRITE failure is attempted exactly once"
    );
}
