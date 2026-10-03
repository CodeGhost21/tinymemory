//! Tests for the actor cache: what each `whoami` answer teaches it.

use super::*;

#[test]
fn starts_unknown_so_the_first_request_asks() {
    assert_eq!(ActorCache::default().lookup(), Lookup::Ask);
}

#[test]
fn a_whoami_caller_is_sent_from_then_on() {
    let cache = ActorCache::default();
    let learned = cache.learn(StatusCode::OK, br#"{"caller":"user:u_123","tenant_id":"t"}"#);
    let expected = HeaderValue::from_static("user:u_123");
    assert_eq!(learned.as_ref(), Some(&expected));
    assert_eq!(cache.lookup(), Lookup::Send(expected));
    assert_eq!(
        cache.clone().lookup(),
        cache.lookup(),
        "clones share what was learned"
    );
}

#[test]
fn a_server_without_whoami_is_not_asked_again() {
    for status in [StatusCode::NOT_FOUND, StatusCode::METHOD_NOT_ALLOWED] {
        let cache = ActorCache::default();
        assert_eq!(cache.learn(status, b""), None);
        assert_eq!(cache.lookup(), Lookup::Skip, "{status}");
    }
}

#[test]
fn an_answer_without_a_usable_caller_sends_nothing() {
    for body in [
        &b"not json"[..],
        br#"{"tenant_id":"t"}"#,
        br#"{"caller":"   "}"#,
        br#"{"caller":42}"#,
        b"{\"caller\":\"user:\\r\\nX-Injected: 1\"}",
    ] {
        let cache = ActorCache::default();
        assert_eq!(cache.learn(StatusCode::OK, body), None);
        assert_eq!(cache.lookup(), Lookup::Skip);
    }
}

#[test]
fn a_failed_lookup_is_retried_on_the_next_request() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let cache = ActorCache::default();
        assert_eq!(cache.learn(status, b"{}"), None);
        assert_eq!(cache.lookup(), Lookup::Ask, "{status}");
    }
}

#[test]
fn forgetting_asks_again() {
    let cache = ActorCache::default();
    cache.learn(StatusCode::OK, br#"{"caller":"user:local"}"#);
    cache.forget();
    assert_eq!(cache.lookup(), Lookup::Ask);
}
