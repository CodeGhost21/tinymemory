//! Tests for reading a rate limit's wait and holding requests back.

use super::*;
use reqwest::header::HeaderValue;

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn a_rate_limit_wait_is_read_from_either_header_and_capped() {
    assert_eq!(
        retry_after(&headers(&[("retry-after", "3")])),
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        retry_after(&headers(&[("ratelimit-reset", " 7 ")])),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "2"), ("ratelimit-reset", "9")])),
        Some(Duration::from_secs(2)),
        "Retry-After wins"
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "3600")])),
        Some(MAX_PAUSE)
    );
    for no_wait in [
        headers(&[]),
        headers(&[("retry-after", "0")]),
        headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]),
    ] {
        assert_eq!(retry_after(&no_wait), None);
    }
}

#[tokio::test]
async fn requests_wait_until_the_latest_rate_limit_lifts() {
    let pause = RatePause::default();
    let started = Instant::now();
    pause.wait().await;
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "nothing noted: no wait"
    );

    pause.note(&headers(&[("retry-after", "2")]));
    pause.note(&headers(&[("retry-after", "1")]));
    pause.clone().wait().await;
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(2),
        "the later lift wins, and clones share it: {waited:?}"
    );
    pause.wait().await;
    assert!(
        started.elapsed() < waited + Duration::from_millis(200),
        "once lifted, no wait"
    );
}

#[tokio::test]
async fn a_pause_extended_while_waiting_holds_the_waiter_too() {
    let pause = RatePause::default();
    let started = Instant::now();
    pause.note(&headers(&[("retry-after", "1")]));
    let extender = pause.clone();
    let extend = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        extender.note(&headers(&[("retry-after", "2")]));
    });
    pause.wait().await;
    extend.await.unwrap();
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(2250),
        "the waiter saw the extension made while it slept: {waited:?}"
    );
}
