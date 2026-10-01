//! The tree family over the hosted wire: leaves under the facts that cite
//! them, a source scope applied inside the listing, and the members hosted
//! memory has no tree for.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::json;
use tinymemory_api::chrono::{TimeZone, Utc};
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::types::{SourceItem, SourceScope};
use tinymemory_api::provider::{
    MemoryCore, MemorySourceSink, MemoryTree, SummaryContext, SummaryInput,
};
use tinymemory_api::tree::IngestRequest;
use tinymemory_api::types::{MemoryCategory, MemoryTaint, GLOBAL_NAMESPACE};

use super::*;
use crate::cortex_provider::families::records::{Place, Records};
use crate::cortex_provider::families::test_support::{hosted, requests};

fn item(id: &str, content: &str, updated_at_ms: i64) -> SourceItem {
    SourceItem {
        item_id: id.to_string(),
        title: String::new(),
        content: content.to_string(),
        mime: None,
        url: None,
        updated_at_ms: Some(updated_at_ms),
        tags: Vec::new(),
    }
}

fn day(n: u32) -> i64 {
    Utc.with_ymd_and_hms(2026, 9, n, 12, 0, 0)
        .single()
        .expect("a day")
        .timestamp_millis()
}

async fn sync(provider: &CortexProvider, source: &str, items: Vec<SourceItem>) -> Vec<String> {
    provider
        .accept_source_items(source, "composio", items, MemoryTaint::ExternalSync)
        .await
        .expect("accept")
        .ids
}

fn input(content: &str) -> SummaryInput {
    let at = Utc
        .with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("at");
    SummaryInput {
        id: "in-1".to_string(),
        content: content.to_string(),
        token_count: 3,
        entities: Vec::new(),
        topics: Vec::new(),
        time_range_start: at,
        time_range_end: at,
        score: 1.0,
    }
}

fn context() -> SummaryContext {
    SummaryContext {
        tree_id: "t".to_string(),
        tree_kind: "source".to_string(),
        target_level: 1,
        token_budget: 100,
        input_token_budget: 1000,
        overhead_reserve_tokens: 10,
        ask: None,
    }
}

#[tokio::test]
async fn leaves_hang_under_the_fact_that_cites_them() {
    let (provider, state) = hosted().await;
    let chat = sync(
        &provider,
        "slack:T1",
        vec![item("c1", "lunch at noon", day(3))],
    )
    .await;
    provider
        .store(
            GLOBAL_NAMESPACE,
            "pref",
            "likes oolong\nmore detail",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let note = Records::new(&provider.dialect)
        .live_all(&Place::namespace(&provider.dialect, GLOBAL_NAMESPACE).expect("place"))
        .await
        .expect("notes")
        .remove(0)
        .event_id;
    let scope = |namespace: &str| provider.dialect.scope_for(namespace).expect("scope");
    let fact = |id: &str, event: &str| {
        json!({
            "id": id, "scope": "x", "predicate": "says",
            "subject": { "type": "entity", "id": "ent_ada", "name": "Ada" },
            "object": { "type": "literal", "datatype": "string", "value": "something" },
            "supports": [event], "confidence": 0.5,
            "valid_from": "2026-09-01T00:00:00Z", "recorded_from": "2026-09-01T00:00:00Z",
        })
    };
    {
        let mut layers = state.layers.lock().expect("layers");
        layers.insert(
            ("facts".to_string(), scope("sources/chat")),
            vec![fact("fact_chat", &chat[0])],
        );
        layers.insert(
            ("facts".to_string(), scope(GLOBAL_NAMESPACE)),
            vec![fact("fact_note", &note)],
        );
    }
    let leaves = provider.recent_leaves(10, None).await.expect("leaves");
    let placed: Vec<(&str, Option<&str>, &str)> = leaves
        .iter()
        .map(|leaf| {
            (
                leaf.preview.as_str(),
                leaf.parent_summary_id.as_deref(),
                leaf.source_id.as_str(),
            )
        })
        .collect();
    assert!(
        placed.contains(&("lunch at noon", Some("fact_chat"), "slack:T1")),
        "{placed:?}"
    );
    assert!(
        placed.contains(&("likes oolong", Some("fact_note"), "global")),
        "{placed:?}"
    );
    assert_eq!(leaves.len(), 2);
}

#[tokio::test]
async fn leaves_are_newest_first_and_capped() {
    let (provider, _state) = hosted().await;
    sync(
        &provider,
        "notion:ws",
        vec![
            item("old", "old page", day(1)),
            item("new", "new page", day(9)),
            item("mid", "mid page", day(5)),
        ],
    )
    .await;
    let leaves = provider.recent_leaves(2, None).await.expect("leaves");
    let previews: Vec<&str> = leaves.iter().map(|leaf| leaf.preview.as_str()).collect();
    assert_eq!(previews, ["new page", "mid page"]);
    assert_eq!(leaves[0].source_id, "notion:ws");
    assert!(leaves[0].parent_summary_id.is_none(), "no fact cites it");
    assert!(provider
        .recent_leaves(0, None)
        .await
        .expect("none")
        .is_empty());
}

#[tokio::test]
async fn a_scoped_caller_reads_leaves_by_label_and_is_not_crowded_out() {
    let (provider, state) = hosted().await;
    sync(&provider, "gmail:me", vec![item("mine", "my mail", day(1))]).await;
    sync(
        &provider,
        "gmail:you",
        (0..5)
            .map(|i| item(&format!("yours-{i}"), "your mail", day(2 + i)))
            .collect(),
    )
    .await;
    let before = requests(&state).len();
    let scope = SourceScope::new(["gmail:me"]);
    let leaves = provider
        .recent_leaves(1, Some(&scope))
        .await
        .expect("leaves");
    let previews: Vec<&str> = leaves.iter().map(|leaf| leaf.preview.as_str()).collect();
    assert_eq!(previews, ["my mail"]);
    assert!(
        leaves[0].parent_summary_id.is_none(),
        "a scoped caller sees no forest"
    );
    let listed = &requests(&state)[before..];
    assert!(
        listed
            .iter()
            .all(|request| !request.contains("/memory/facts")),
        "{listed:?}"
    );
    assert!(
        listed
            .iter()
            .filter(|request| request.starts_with("GET /memory/events?"))
            .all(|request| request.contains("labels=")),
        "{listed:?}"
    );
    let before = requests(&state).len();
    assert!(provider
        .recent_leaves(5, Some(&SourceScope::default()))
        .await
        .expect("denied")
        .is_empty());
    assert_eq!(
        requests(&state).len(),
        before,
        "an empty scope asks nothing"
    );
}

#[tokio::test]
async fn the_members_that_write_or_walk_a_tree_are_unsupported() {
    let (provider, _state) = hosted().await;
    let unsupported = |result: Result<(), MemoryError>| {
        assert!(
            matches!(result, Err(MemoryError::Unsupported { .. })),
            "{result:?}"
        );
    };
    unsupported(
        provider
            .append(IngestRequest {
                namespace: "global".to_string(),
                content: "x".to_string(),
                timestamp: None,
                metadata: None,
            })
            .await,
    );
    unsupported(
        provider
            .query_source("global", "s", 5, None)
            .await
            .map(drop),
    );
    unsupported(provider.drill_down("global", "n").await.map(drop));
    unsupported(provider.seal("global").await.map(drop));
    unsupported(provider.cascade("global").await.map(drop));
    unsupported(provider.runtime_tree_status("global").await.map(drop));
    unsupported(provider.runtime_rebuild("global").await.map(drop));
    unsupported(provider.runtime_read_node("global", "n").await.map(drop));
}

#[tokio::test]
async fn nothing_to_fold_is_an_empty_summary_and_anything_else_needs_a_model() {
    let (provider, _state) = hosted().await;
    let empty = provider.summarise(&[], &context()).await.expect("empty");
    assert!(empty.content.is_empty());
    let blank = provider
        .summarise(&[input("   ")], &context())
        .await
        .expect("blank");
    assert!(blank.content.is_empty());
    let refused = provider.summarise(&[input("we met")], &context()).await;
    assert!(
        matches!(refused, Err(MemoryError::Unsupported { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn hosted_memory_seals_nothing_and_compiles_no_flavour() {
    let (provider, state) = hosted().await;
    assert_eq!(
        provider.flush_source_tree("gmail:me").await.expect("flush"),
        0
    );
    assert!(provider
        .root_summaries_with_caps(5, 20)
        .await
        .expect("roots")
        .is_empty());
    assert_eq!(
        provider.flavour_profile("gmail:me").await.expect("flavour"),
        None
    );
    assert!(matches!(
        provider.flavour_profile("  ").await,
        Err(MemoryError::Invalid(_))
    ));
    assert!(
        requests(&state).is_empty(),
        "none of these asks the backend"
    );
}
