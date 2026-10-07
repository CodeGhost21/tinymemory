//! Tests for date hints on the CortexDB wire: what is sent, the capability
//! gate, and the bare retry after a refusal.

use super::*;

use chrono::{NaiveDate, TimeZone, Utc};
use tinymemory_api::{FetchMode, FetchRequest, MemoryEngine, MemoryMeta, SourceKind, StoreItem};

use crate::cortex::testing::{both, direct_double, direct_engine};

fn on_day(text: &str, day: u32) -> StoreItem {
    let mut meta = MemoryMeta::from_source(SourceKind::Github, None);
    // 20:00 UTC: already the next day in India, so the zone is exercised.
    meta.observed_at = Utc.with_ymd_and_hms(2026, 10, day, 20, 0, 0).single();
    StoreItem::document(text, meta)
}

fn fetch(hint: Option<TimeHint>) -> FetchRequest {
    let mut req = FetchRequest::new("launch plan", FetchMode::Hybrid, 5);
    req.refers_to = hint;
    req
}

fn india(day: u32) -> TimeHint {
    let d = NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
    TimeHint::new(d, d, Some("Asia/Kolkata".into())).unwrap()
}

#[tokio::test]
async fn a_hinted_fetch_sends_refers_during_and_puts_the_hinted_day_first() {
    for (engine, state) in both().await {
        for (text, day) in [
            ("launch plan one", 1),
            ("launch plan two", 2),
            ("launch plan three", 5),
        ] {
            engine.store(on_day(text, day)).await.unwrap();
        }
        let plain = engine.fetch(fetch(None)).await.unwrap();
        assert_ne!(
            plain.hits[0].text, "launch plan two",
            "fixture: undated order differs"
        );

        // Stored 2026-10-02 20:00 UTC = the 3rd in India.
        let dated = engine.fetch(fetch(Some(india(3)))).await.unwrap();
        assert_eq!(dated.hits[0].text, "launch plan two");
        assert_eq!(
            dated.hits.len(),
            plain.hits.len(),
            "a hint never drops a hit"
        );

        let recalls = state.seen.lock().unwrap().recalls.clone();
        let sent = recalls.last().unwrap();
        assert_eq!(
            sent["temporal"],
            serde_json::json!({
                "refers_during": {"from": "2026-10-03", "to": "2026-10-03"},
                "timezone": "Asia/Kolkata"
            })
        );
        assert!(
            sent["temporal"].get("valid_during").is_none()
                && sent["temporal"].get("natural").is_none()
        );
    }
}

#[tokio::test]
async fn a_direct_server_without_the_capability_gets_no_hint_and_is_asked_once() {
    let (endpoint, state) = direct_double().await;
    state.refers_unlisted.store(true, Ordering::SeqCst);
    let engine = direct_engine(&endpoint);
    engine.store(on_day("launch plan", 2)).await.unwrap();
    for _ in 0..2 {
        engine.fetch(fetch(Some(india(3)))).await.unwrap();
    }
    let seen = state.seen.lock().unwrap();
    assert!(
        seen.recalls.iter().all(|r| r.get("temporal").is_none()),
        "{:?}",
        seen.recalls
    );
    let probes = seen
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /v1/admin/version"))
        .count();
    assert_eq!(probes, 1, "the capability is cached: {:?}", seen.requests);
}

#[tokio::test]
async fn a_refused_hint_is_retried_without_it_and_never_sent_again() {
    for (engine, state) in both().await {
        state.refers_refused.store(true, Ordering::SeqCst);
        engine.store(on_day("launch plan", 2)).await.unwrap();
        state.seen.lock().unwrap().recalls.clear();

        let page = engine.fetch(fetch(Some(india(3)))).await.unwrap();
        assert_eq!(page.hits.len(), 1, "the refused hint cost no hit");
        let first: Vec<bool> = take_recalls(&state)
            .iter()
            .map(|r| r.get("temporal").is_some())
            .collect();
        let hinted = first.iter().filter(|t| **t).count();
        assert!(hinted >= 1, "fixture: the hint was sent: {first:?}");
        assert_eq!(
            first.len(),
            2 * hinted,
            "each refused pack retried bare once: {first:?}"
        );

        engine.fetch(fetch(Some(india(3)))).await.unwrap();
        let second = take_recalls(&state);
        assert!(!second.is_empty());
        assert!(
            second.iter().all(|r| r.get("temporal").is_none()),
            "never sent again: {second:?}"
        );
    }
}

fn take_recalls(state: &crate::cortex::testing::Shared) -> Vec<serde_json::Value> {
    std::mem::take(&mut state.seen.lock().unwrap().recalls)
}

#[tokio::test]
async fn a_refusal_that_is_not_about_the_hint_is_returned_and_keeps_the_gate() {
    for (engine, state) in both().await {
        engine.store(on_day("launch plan", 2)).await.unwrap();
        *state.fail_all.lock().unwrap() = Some((422, "INVALID_BODY"));
        assert!(engine.fetch(fetch(Some(india(3)))).await.is_err());
        *state.fail_all.lock().unwrap() = None;
        engine.fetch(fetch(Some(india(3)))).await.unwrap();
        let recalls = state.seen.lock().unwrap().recalls.clone();
        assert!(
            recalls.last().unwrap().get("temporal").is_some(),
            "a failure the bare retry repeated must not switch hints off"
        );
    }
}
