//! The source sink over the hosted wire.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::types::{ForgetSelector, SourceItem};
use tinymemory_api::provider::{MemoryCore, MemorySourceSink};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::*;
use crate::cortex_provider::families::test_support::{hosted, requests};
use crate::cortex_provider::families::FamilyState;
use crate::hosted_test_support::Shared;

fn item(id: &str, title: &str, content: &str) -> SourceItem {
    SourceItem {
        item_id: id.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        mime: None,
        url: None,
        updated_at_ms: None,
        tags: Vec::new(),
    }
}

/// Every event the double holds in `scope`.
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

fn envelope(event: &Value) -> Value {
    serde_json::from_str(event["content"]["text"].as_str().expect("text")).expect("envelope")
}

#[test]
fn items_land_by_where_they_came_from() {
    assert_eq!(placement("gmail:me@x", "composio"), SourceKind::Email);
    assert_eq!(placement("outlook", "composio"), SourceKind::Email);
    assert_eq!(placement("slack:T1:C2", "composio"), SourceKind::Chat);
    assert_eq!(placement("whatsapp:1", "composio"), SourceKind::Chat);
    assert_eq!(placement("notion:page", "composio"), SourceKind::Document);
    assert_eq!(placement("gmail:me@x", "folder"), SourceKind::Document);
}

#[test]
fn an_items_title_heads_its_text_once() {
    assert_eq!(item_text(&item("1", "Subject", "Body")), "Subject\n\nBody");
    assert_eq!(
        item_text(&item("1", "Subject", "Subject and body")),
        "Subject and body"
    );
    assert_eq!(item_text(&item("1", "  ", "Body")), "Body");
}

#[tokio::test]
async fn a_batch_writes_labelled_records_with_their_provenance() {
    let (provider, state) = hosted().await;
    let batch = vec![SourceItem {
        url: Some("https://mail.example/1".to_string()),
        updated_at_ms: Some(1_767_225_600_000),
        ..item("m1", "Hello", "How are you?")
    }];
    let outcome = provider
        .accept_source_items("gmail:me", "composio", batch, MemoryTaint::ExternalSync)
        .await
        .expect("accept");
    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.skipped, 0);
    assert_eq!(outcome.ids.len(), 1);
    let events = events_in(&state, "tm:sources/tm:email");
    assert_eq!(events.len(), 1);
    let envelope = envelope(&events[0]);
    assert_eq!(envelope["k"], json!("item:gmail:me:m1"));
    assert_eq!(envelope["c"], json!("Hello\n\nHow are you?"));
    assert_eq!(envelope["t"], json!("external_sync"));
    assert_eq!(
        envelope["x"],
        json!({ "prov": { "src": "gmail:me", "ref": "https://mail.example/1" } })
    );
    assert_eq!(
        events[0]["context"]["observed_at"],
        json!("2026-01-01T00:00:00+00:00")
    );
    let labels = events[0]["context"]["labels"].as_array().expect("labels");
    assert!(labels.contains(&json!(crate::cortex_labels::source("gmail:me"))));
    // The record is readable through the ordinary keyed surface.
    let entry = provider
        .get("sources/email", "item:gmail:me:m1")
        .await
        .expect("get")
        .expect("entry");
    assert_eq!(entry.taint, MemoryTaint::ExternalSync);
    assert_eq!(entry.category, MemoryCategory::Core);
}

#[tokio::test]
async fn an_unchanged_item_is_skipped_and_a_changed_one_replaces_its_version() {
    let (provider, state) = hosted().await;
    let first = vec![item("a", "", "one"), item("b", "", "stays")];
    provider
        .accept_source_items(
            "notion:ws",
            "composio",
            first.clone(),
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("first");
    let again = provider
        .accept_source_items("notion:ws", "composio", first, MemoryTaint::ExternalSync)
        .await
        .expect("again");
    assert_eq!(again.written, 0);
    assert_eq!(again.skipped, 2);
    assert!(again.already_ingested);
    let changed = provider
        .accept_source_items(
            "notion:ws",
            "composio",
            vec![item("a", "", "two"), item("b", "", "stays")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("changed");
    assert_eq!(changed.written, 1);
    assert_eq!(changed.skipped, 1);
    assert!(!changed.already_ingested);
    let bodies: Vec<String> = events_in(&state, "tm:sources/tm:documents")
        .iter()
        .map(|event| envelope(event)["c"].as_str().expect("c").to_string())
        .collect();
    assert_eq!(bodies, ["stays", "two"], "the replaced version is retired");
}

#[tokio::test]
async fn an_empty_item_is_skipped_without_counting_as_already_ingested() {
    let (provider, state) = hosted().await;
    let outcome = provider
        .accept_source_items(
            "folder:1",
            "folder",
            vec![item("a", "title only", "   ")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    assert_eq!(outcome.written, 0);
    assert_eq!(outcome.skipped, 1);
    assert!(!outcome.already_ingested);
    assert!(state.log.lock().expect("log").events.is_empty());
}

#[tokio::test]
async fn a_batch_waits_for_its_last_write_once() {
    let (provider, state) = hosted().await;
    provider
        .accept_source_items(
            "slack:T1",
            "composio",
            vec![item("1", "", "a"), item("2", "", "b"), item("3", "", "c")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    let seen = requests(&state);
    let unlabelled_listings = seen
        .iter()
        .filter(|r| r.starts_with("GET /memory/events?") && !r.contains("labels="))
        .count();
    assert_eq!(unlabelled_listings, 1, "{seen:?}");
    assert!(
        !seen.iter().any(|r| r.starts_with("POST /memory/recall")),
        "{seen:?}"
    );
}

#[tokio::test]
async fn writes_wait_their_turn_at_the_pacing_gate() {
    let (provider, _state) = hosted().await;
    let provider = provider.with_families(FamilyState::with(
        Duration::from_millis(60),
        Duration::from_secs(3600),
        Duration::ZERO,
    ));
    let started = Instant::now();
    provider
        .accept_source_items(
            "folder:1",
            "folder",
            vec![item("1", "", "a"), item("2", "", "b"), item("3", "", "c")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    assert!(started.elapsed() >= Duration::from_millis(120));
}

#[tokio::test]
async fn a_failure_mid_batch_keeps_its_class_and_says_how_far_it_got() {
    let (provider, state) = hosted().await;
    state.fail_nth_experience.store(2, Ordering::SeqCst);
    let error = provider
        .accept_source_items(
            "folder:1",
            "folder",
            vec![item("1", "", "a"), item("2", "", "b"), item("3", "", "c")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect_err("the second write is refused");
    match error {
        MemoryError::Invalid(message) => {
            assert!(message.starts_with("[VALIDATION_ERROR]"), "{message}");
            assert!(message.contains("accepted 1 of 3"), "{message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn a_batch_needs_a_source_id() {
    let (provider, _state) = hosted().await;
    assert!(matches!(
        provider
            .accept_source_items(" ", "folder", vec![], MemoryTaint::ExternalSync)
            .await,
        Err(MemoryError::Invalid(_))
    ));
}

#[tokio::test]
async fn forgetting_a_source_removes_its_items_and_nothing_else() {
    let (provider, state) = hosted().await;
    for (source, id) in [("gmail:me", "1"), ("gmail:me", "2"), ("gmail:you", "3")] {
        provider
            .accept_source_items(
                source,
                "composio",
                vec![item(id, "", "mail")],
                MemoryTaint::ExternalSync,
            )
            .await
            .expect("accept");
    }
    assert_eq!(provider.forget_source("gmail:me").await.expect("forget"), 2);
    let left: Vec<String> = events_in(&state, "tm:sources/tm:email")
        .iter()
        .map(|event| envelope(event)["k"].as_str().expect("k").to_string())
        .collect();
    assert_eq!(left, ["item:gmail:you:3"]);
    assert_eq!(provider.forget_source("gmail:me").await.expect("again"), 0);
}

#[tokio::test]
async fn forgetting_by_selector_names_its_kind_or_its_event() {
    let (provider, state) = hosted().await;
    let outcome = provider
        .accept_source_items(
            "slack:T1",
            "composio",
            vec![item("1", "", "a"), item("2", "", "b")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    // A chunk is one item, by its event id.
    let removed = provider
        .forget_matching(&ForgetSelector::Chunk {
            chunk_id: outcome.ids[0].clone(),
        })
        .await
        .expect("chunk");
    assert_eq!(removed.chunks_removed, 1);
    // A kind names the namespace it lands in.
    let removed = provider
        .forget_matching(&ForgetSelector::Source {
            source_kind: "chat".to_string(),
            source_id: "slack:T1".to_string(),
        })
        .await
        .expect("source");
    assert_eq!(removed.chunks_removed, 1);
    assert!(events_in(&state, "tm:sources/tm:chat").is_empty());
    assert!(matches!(
        provider
            .forget_matching(&ForgetSelector::Source {
                source_kind: "composio".to_string(),
                source_id: "slack:T1".to_string(),
            })
            .await,
        Err(MemoryError::Invalid(_))
    ));
    for unsupported in [
        ForgetSelector::SourcePrefix {
            source_kind: "chat".to_string(),
            source_id_prefix: "slack:".to_string(),
        },
        ForgetSelector::Owner {
            source_kind: "chat".to_string(),
            owner: "me".to_string(),
        },
    ] {
        assert!(matches!(
            provider.forget_matching(&unsupported).await,
            Err(MemoryError::Unsupported { .. })
        ));
    }
}

#[tokio::test]
async fn a_chunk_that_is_not_a_synced_item_of_this_account_is_left_alone() {
    let (provider, state) = hosted().await;
    provider
        .store(
            "notes",
            "k",
            "mine",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let outcome = provider
        .accept_source_items(
            "folder:1",
            "folder",
            vec![item("1", "", "a")],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    let note_id = events_in(&state, "tm:notes")[0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let item_id = outcome.ids[0].clone();
    state
        .foreign
        .lock()
        .expect("foreign")
        .insert(item_id.clone());
    for chunk_id in [
        note_id,
        item_id,
        "no-such-event".to_string(),
        "bad id!".to_string(),
    ] {
        let removed = provider
            .forget_matching(&ForgetSelector::Chunk { chunk_id })
            .await
            .expect("forget");
        assert_eq!(removed.chunks_removed, 0);
    }
    assert_eq!(state.log.lock().expect("log").events.len(), 2);
}
