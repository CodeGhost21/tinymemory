//! The keyed record layer, against the `/memory/*` double.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use tinymemory_api::provider::{MemoryCore, MemoryProvider};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::*;
use crate::cortex_provider::families::test_support::{hosted, requests};
use crate::hosted_test_support::Shared;

/// Every event the double holds in `scope`, oldest first.
fn events_in(state: &Shared, scope: &str) -> Vec<Value> {
    state
        .log
        .lock()
        .expect("log")
        .events
        .iter()
        .filter(|event| event["scope"] == scope)
        .cloned()
        .collect()
}

/// The envelope an event carries.
fn envelope(event: &Value) -> Value {
    serde_json::from_str(event["content"]["text"].as_str().expect("text")).expect("envelope")
}

#[tokio::test]
async fn a_record_keeps_what_it_was_written_with() {
    let (provider, _state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    let record = Record {
        key: "k".to_string(),
        content: "body".to_string(),
        category: MemoryCategory::Core,
        session_id: Some("s1".to_string()),
        taint: MemoryTaint::ExternalSync,
        provenance: Provenance {
            source: Some("src".to_string()),
            reference: Some("https://example.test/a".to_string()),
            document: Some("doc-1".to_string()),
        },
    };
    records
        .put(
            &place,
            &record,
            Some("2026-09-01T00:00:00+00:00".to_string()),
        )
        .await
        .expect("put");
    let live = records
        .live(&place, "k")
        .await
        .expect("read")
        .expect("live");
    assert_eq!(live.record, record);
}

#[tokio::test]
async fn a_family_record_and_a_store_share_one_shape() {
    let (provider, state) = hosted().await;
    provider
        .store(
            "notes",
            "stored",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    records
        .put(&place, &Record::plain("put", "v"), None)
        .await
        .expect("put");
    let events = events_in(&state, &place.scope);
    assert_eq!(events.len(), 2);
    for event in &events {
        let envelope = envelope(event);
        assert!(envelope.get("x").is_none(), "{envelope}");
        let key = envelope["k"].as_str().expect("key");
        assert_eq!(
            event["context"]["labels"],
            json!([crate::cortex_labels::key(key)]),
            "hosted `store` labels its key too"
        );
        assert!(event.get("directives").is_none(), "{event}");
    }
    // Each read path sees the other's record.
    assert!(records
        .live(&place, "stored")
        .await
        .expect("read")
        .is_some());
    let got = provider
        .get("notes", "put")
        .await
        .expect("get")
        .expect("entry");
    assert_eq!(got.content, "v");
}

#[tokio::test]
async fn a_rewrite_retires_exactly_the_versions_it_replaced() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    for (key, content) in [("k", "one"), ("k", "two"), ("other", "x"), ("k", "three")] {
        records
            .put(&place, &Record::plain(key, content), None)
            .await
            .expect("put");
    }
    let bodies: Vec<String> = events_in(&state, &place.scope)
        .iter()
        .map(|event| envelope(event)["c"].as_str().expect("c").to_string())
        .collect();
    assert_eq!(bodies, ["x", "three"]);
    assert_eq!(state.log.lock().expect("log").forgotten.len(), 2);
    let live = records
        .live(&place, "k")
        .await
        .expect("read")
        .expect("live");
    assert_eq!(live.record.content, "three");
}

#[tokio::test]
async fn a_removed_key_leaves_only_its_tombstone() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    records
        .put(&place, &Record::plain("k", "one"), None)
        .await
        .expect("put");
    assert!(records.remove(&place, "k").await.expect("remove"));
    let events = events_in(&state, &place.scope);
    assert_eq!(events.len(), 1);
    assert_eq!(envelope(&events[0])["d"], json!(true));
    assert!(records.live(&place, "k").await.expect("read").is_none());
    assert!(!records.remove(&place, "k").await.expect("second remove"));
}

#[tokio::test]
async fn a_label_miss_in_a_user_namespace_walks_the_scope() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    // A record written before hosted `store` labelled its keys.
    state.log.lock().expect("log").events.push(json!({
        "id": "evt_legacy",
        "scope": place.scope,
        "wal_offset": 1,
        "content": {
            "kind": "message",
            "role": "user",
            "text": "{\"k\":\"old\",\"c\":\"legacy\",\"cat\":\"core\",\"s\":null,\"t\":\"internal\",\"d\":false}"
        },
        "context": { "recorded_at": "2026-09-01T00:00:00Z" },
    }));
    let live = records
        .live(&place, "old")
        .await
        .expect("read")
        .expect("live");
    assert_eq!(live.record.content, "legacy");
}

#[tokio::test]
async fn a_bookkeeping_record_is_inert_and_never_walks_or_recalls() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::bookkeeping(&provider.dialect, "tmi:test".to_string()).expect("place");
    records
        .put(&place, &Record::plain("k", "v"), None)
        .await
        .expect("put");
    let events = events_in(&state, "tmi:test");
    assert_eq!(
        events[0]["directives"],
        json!({ "embed": "none", "extract": [] })
    );
    assert!(records
        .live(&place, "missing")
        .await
        .expect("read")
        .is_none());
    let seen = requests(&state);
    assert!(
        !seen.iter().any(|r| r.starts_with("POST /memory/recall")),
        "{seen:?}"
    );
    // The miss asked by label, and never walked the scope.
    let last = seen.last().expect("a request");
    assert!(last.contains("labels="), "{last}");
}

#[tokio::test]
async fn a_retire_that_fails_does_not_fail_the_write_and_the_next_write_finishes_it() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    records
        .put(&place, &Record::plain("k", "one"), None)
        .await
        .expect("put");
    state
        .rate_limit_forget
        .store(10, std::sync::atomic::Ordering::SeqCst);
    records
        .put(&place, &Record::plain("k", "two"), None)
        .await
        .expect("the write stands although its retire failed");
    assert_eq!(events_in(&state, &place.scope).len(), 2);
    let live = records
        .live(&place, "k")
        .await
        .expect("read")
        .expect("live");
    assert_eq!(live.record.content, "two");
    state
        .rate_limit_forget
        .store(0, std::sync::atomic::Ordering::SeqCst);
    records
        .put(&place, &Record::plain("k", "three"), None)
        .await
        .expect("put");
    assert_eq!(events_in(&state, &place.scope).len(), 1);
}

#[tokio::test]
async fn records_are_found_by_source_and_rechecked() {
    let (provider, _state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::family_namespace(&provider.dialect, "sources/documents").expect("place");
    for (key, source) in [("a", "s1"), ("b", "s2"), ("c", "s1")] {
        let record = Record {
            provenance: Provenance {
                source: Some(source.to_string()),
                ..Provenance::default()
            },
            ..Record::plain(key, "v")
        };
        records.put(&place, &record, None).await.expect("put");
    }
    let mut keys: Vec<String> = records
        .of_source(&place, "s1")
        .await
        .expect("read")
        .into_iter()
        .map(|v| v.record.key)
        .collect();
    keys.sort();
    assert_eq!(keys, ["a", "c"]);
}

#[tokio::test]
async fn a_key_holding_a_comma_reads_back_alone() {
    let (provider, _state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    records
        .put(&place, &Record::plain("a,b", "comma"), None)
        .await
        .expect("put");
    records
        .put(&place, &Record::plain("a", "plain"), None)
        .await
        .expect("put");
    let comma = records
        .live(&place, "a,b")
        .await
        .expect("read")
        .expect("live");
    let plain = records
        .live(&place, "a")
        .await
        .expect("read")
        .expect("live");
    assert_eq!(comma.record.content, "comma");
    assert_eq!(plain.record.content, "plain");
    let batch = records.versions(&place, &["a", "a,b"]).await.expect("read");
    assert_eq!(batch.len(), 2);
}

#[tokio::test]
async fn clearing_a_scope_removes_every_event_and_counts_the_live_keys() {
    let (provider, state) = hosted().await;
    let records = Records::new(&provider.dialect);
    let place = Place::namespace(&provider.dialect, "notes").expect("place");
    for key in ["a", "b"] {
        records
            .put(&place, &Record::plain(key, "v"), None)
            .await
            .expect("put");
    }
    records.remove(&place, "b").await.expect("remove");
    assert_eq!(records.clear(&place).await.expect("clear"), 1);
    assert!(events_in(&state, &place.scope).is_empty());
    assert_eq!(records.clear(&place).await.expect("clear again"), 0);
}

#[tokio::test]
async fn a_bookkeeping_scope_deeper_than_a_hosted_scope_is_refused() {
    let (provider, _state) = hosted().await;
    let deep: Vec<String> = (0..32).map(|i| format!("tmi:s{i}")).collect();
    assert!(Place::bookkeeping(&provider.dialect, deep.join("/")).is_err());
    let fits: Vec<String> = (0..31).map(|i| format!("tmi:s{i}")).collect();
    assert!(Place::bookkeeping(&provider.dialect, fits.join("/")).is_ok());
}

#[tokio::test]
async fn the_direct_wire_writes_exactly_what_it_always_has() {
    let endpoint = crate::conformance_test::cortex_backend().await;
    let memory = crate::CortexMemory::api(&endpoint, "cortex-key").expect("builds");
    let provider = crate::cortex_provider(memory);
    provider
        .store(
            "notes",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let scope = provider.dialect.scope_for("notes").expect("scope");
    let events = provider
        .dialect
        .events_matching(&scope, None)
        .await
        .expect("list");
    assert_eq!(events.len(), 1);
    assert!(
        events.iter().all(|e| e["context"].get("labels").is_none()),
        "{events:?}"
    );
    assert!(provider.as_goals().is_none());
    assert!(provider.as_documents().is_none());
    assert!(provider.as_sources().is_none());
    assert!(provider.as_tool_memory().is_none());
    assert!(provider.as_maintenance().is_none());
}
