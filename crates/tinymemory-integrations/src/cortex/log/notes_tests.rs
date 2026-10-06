//! Pack notes: which warnings and evictions are read, and how they are logged.

use std::sync::{Mutex, Once};

use serde_json::json;

use super::*;

/// Every record logged in this test binary, as `(level, message)`. Tests
/// find their own by a scope no other test uses.
static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        RECORDS
            .lock()
            .unwrap()
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

fn capture() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Capture).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn logged_for(scope: &str) -> Vec<(log::Level, String)> {
    RECORDS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, message)| message.contains(&format!("scope={scope:?} ")))
        .cloned()
        .collect()
}

#[test]
fn a_clean_pack_carries_no_notes() {
    let pack = json!({ "pack_id": "p", "layers": { "events": [] } });
    assert!(notes(&pack).is_empty());
    assert!(notes(&json!({ "diagnostics": { "knapsack_evictions": 0 } })).is_empty());
}

#[test]
fn the_parent_sample_warning_is_told_apart_from_other_warnings() {
    let sample = "parent_pack_unranked_sample: events from 2 of 5 authorized descendant scopes, \
                  selected in storage order, not by relevance";
    let pack = json!({ "warnings": ["temporal_natural_ignored: x", sample] });
    assert_eq!(
        notes(&pack),
        [
            Note::Warning("temporal_natural_ignored: x"),
            Note::ParentSample(sample)
        ]
    );
    assert_eq!(Note::Warning("x").level(), log::Level::Debug);
    assert_eq!(Note::ParentSample(sample).level(), log::Level::Warn);
}

#[test]
fn an_eviction_is_read_from_the_count_and_the_context_receipt() {
    let pack = json!({
        "diagnostics": { "knapsack_evictions": 3 },
        "provenance": { "trail": [
            { "phase": "hybrid_retrieve" },
            { "phase": "context_contributors", "context_contributors": [
                { "event_id": "evt_kept", "rank": 0 },
                { "event_id": "evt_gone", "rank": 4, "evicted_from_layers": true }
            ]}
        ]}
    });
    assert_eq!(
        notes(&pack),
        [Note::Evicted {
            count: Some(3),
            events: vec!["evt_gone"]
        }]
    );
    let receipt_only = json!({ "provenance": { "trail": [
        { "phase": "context_contributors", "context_contributors": [
            { "event_id": "evt_gone", "evicted_from_layers": true }
        ]}
    ]}});
    assert_eq!(
        notes(&receipt_only),
        [Note::Evicted {
            count: None,
            events: vec!["evt_gone"]
        }]
    );
}

#[test]
fn an_eviction_and_a_parent_sample_are_logged_at_warn_with_the_scope() {
    capture();
    let scope = "app:tinymemory/agent:notes-test/app:conversations";
    report(
        scope,
        &json!({
            "warnings": ["entity_grounding_advisory: x", "parent_pack_unranked_sample: y"],
            "diagnostics": { "knapsack_evictions": 2 }
        }),
    );
    let logged = logged_for(scope);
    assert_eq!(logged.len(), 3, "{logged:?}");
    assert_eq!(logged[0].0, log::Level::Debug);
    assert!(logged[0].1.contains("entity_grounding_advisory"));
    assert_eq!(logged[1].0, log::Level::Warn);
    assert!(
        logged[1].1.contains("parent-scope sample"),
        "{}",
        logged[1].1
    );
    assert_eq!(logged[2].0, log::Level::Warn);
    assert!(
        logged[2].1.contains("knapsack_evictions=2"),
        "{}",
        logged[2].1
    );
}

#[tokio::test]
async fn every_recall_pack_is_reported_as_it_is_read() {
    use axum::routing::post;
    use axum::{Json, Router};
    use tinymemory_api::{FetchMode, FetchRequest, MemoryEngine, MetaFilter, Namespace, Reach};

    capture();
    let app = Router::new().route(
        "/v1/recall",
        post(|| async {
            Json(json!({
                "pack_id": "p",
                "layers": { "events": [] },
                "diagnostics": { "knapsack_evictions": 5 }
            }))
        }),
    );
    let endpoint = crate::cortex::testing::serve(app).await;
    let mut request = FetchRequest::new("q", FetchMode::Hybrid, 3);
    request.filter = MetaFilter {
        reach: Some(Reach::exact(Namespace::agent("notes-e2e"))),
        ..MetaFilter::kinds([tinymemory_api::ItemKind::Document])
    };
    crate::cortex::testing::direct_engine(&endpoint)
        .fetch(request)
        .await
        .unwrap();
    let logged = logged_for("app:tinymemory/agent:notes-e2e/app:documents");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].0, log::Level::Warn);
    assert!(
        logged[0].1.contains("knapsack_evictions=5"),
        "{}",
        logged[0].1
    );
}

#[test]
fn only_the_exact_parent_sample_code_is_a_parent_sample() {
    assert!(is_parent_sample("parent_pack_unranked_sample"));
    assert!(is_parent_sample(
        "parent_pack_unranked_sample: events from 2 of 3"
    ));
    for other in [
        "parent_pack_unranked_sampled: x",
        "parent_pack_unranked_sample_v2: x",
        "parent_pack_unranked",
    ] {
        assert!(!is_parent_sample(other), "{other}");
        assert_eq!(
            notes(&json!({ "warnings": [other] })),
            [Note::Warning(other)],
            "{other}"
        );
    }
}

#[test]
fn a_newline_in_a_scope_or_a_warning_cannot_forge_a_log_line() {
    capture();
    let scope = "app:tinymemory/agent:inject-test/app:documents\n[cortex] forged";
    report(
        scope,
        &json!({ "warnings": ["entity_grounding_advisory: x\nERROR forged line"] }),
    );
    let logged = logged_for(scope);
    assert_eq!(logged.len(), 1, "{logged:?}");
    let message = &logged[0].1;
    assert!(!message.contains('\n'), "no raw newline: {message:?}");
    assert!(message.contains("\\n[cortex] forged"), "{message}");
    assert!(message.contains("\\nERROR forged line"), "{message}");
}

#[test]
fn a_partial_event_is_logged_at_warn_with_its_reason() {
    capture();
    let scope = "app:tinymemory/agent:notes-partial/app:documents";
    let pack = json!({ "layers": { "events": [
        { "id": "a", "content": { "text": "whole" } },
        { "id": "b", "content": { "text": "[…]\nslice", "_partial": true,
                                  "_partial_reason": "budget_excerpt" } },
        { "id": "c", "content": { "text": "view", "_partial": true } },
    ] } });
    assert_eq!(
        notes(&pack),
        [Note::Partial(vec!["budget_excerpt", "unknown"])]
    );
    report(scope, &pack);
    let logged = logged_for(scope);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].0, log::Level::Warn);
    assert!(logged[0].1.contains("partial_events=2"), "{}", logged[0].1);
}
