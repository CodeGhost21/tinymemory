//! Retrieval over the hosted wire: ranks as scores, leaves over the synced
//! records, and the top of the engine's ranking for namespace recall.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::types::{SourceItem, SourceScope};
use tinymemory_api::provider::{
    CoverWindowQuery, FastRetrieveQuery, MemoryCore, MemoryRetrieval, MemorySourceSink,
    SourceRetrievalQuery,
};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::*;
use crate::cortex_provider::families::test_support::hosted;

fn item(id: &str, content: &str, updated_at_ms: Option<i64>) -> SourceItem {
    SourceItem {
        item_id: id.to_string(),
        title: String::new(),
        content: content.to_string(),
        mime: None,
        url: Some(format!("https://example.test/{id}")),
        updated_at_ms,
        tags: Vec::new(),
    }
}

fn fast(limit: usize) -> FastRetrieveQuery {
    FastRetrieveQuery {
        limit,
        max_hops: 1,
        time_window_days: None,
    }
}

async fn sync(provider: &CortexProvider, source: &str, items: Vec<SourceItem>) {
    provider
        .accept_source_items(source, "composio", items, MemoryTaint::ExternalSync)
        .await
        .expect("accept");
}

#[test]
fn a_rank_scores_one_for_the_best_hit_and_never_below_a_tenth() {
    assert_eq!(rank_score(0), 1.0);
    assert!((rank_score(1) - 0.9).abs() < 1e-9);
    assert!((rank_score(2) - 0.8).abs() < 1e-9);
    assert_eq!(rank_score(50), 0.1);
}

#[tokio::test]
async fn fast_retrieve_answers_ranked_leaves_across_every_source() {
    let (provider, _state) = hosted().await;
    sync(
        &provider,
        "gmail:me",
        vec![item("m1", "oolong invoice", None)],
    )
    .await;
    sync(
        &provider,
        "slack:T1",
        vec![item("c1", "oolong at lunch", None)],
    )
    .await;
    sync(&provider, "notion:ws", vec![item("p1", "rent notes", None)]).await;
    let response = provider
        .fast_retrieve("oolong", fast(10), None)
        .await
        .expect("retrieve");
    assert_eq!(response.total, 2);
    assert!(!response.truncated);
    let scores: Vec<f32> = response.hits.iter().map(|hit| hit.score).collect();
    assert_eq!(scores, [1.0, 0.9]);
    for hit in &response.hits {
        assert_eq!(hit.level, 0);
        assert!(hit.child_ids.is_empty());
        assert!(hit
            .source_ref
            .as_deref()
            .is_some_and(|r| r.starts_with("https://")));
    }
    let chat = response
        .hits
        .iter()
        .find(|hit| hit.tree_scope == "slack:T1")
        .expect("the chat leaf");
    assert_eq!(chat.tree_kind.as_deref(), Some("chat"));
    assert_eq!(chat.tree_id, "sources/chat");
    let cut = provider
        .fast_retrieve("oolong", fast(1), None)
        .await
        .expect("retrieve");
    assert_eq!(cut.hits.len(), 1);
    assert!(cut.truncated);
}

#[tokio::test]
async fn a_restricted_caller_sees_only_its_sources() {
    let (provider, _state) = hosted().await;
    sync(
        &provider,
        "gmail:me",
        vec![item("m1", "oolong invoice", None)],
    )
    .await;
    sync(
        &provider,
        "gmail:you",
        vec![item("m2", "oolong order", None)],
    )
    .await;
    let scope = SourceScope::new(["gmail:me"]);
    let response = provider
        .fast_retrieve("oolong", fast(10), Some(&scope))
        .await
        .expect("retrieve");
    let sources: Vec<&str> = response
        .hits
        .iter()
        .map(|h| h.tree_scope.as_str())
        .collect();
    assert_eq!(sources, ["gmail:me"]);
    let none = provider
        .fast_retrieve("oolong", fast(10), Some(&SourceScope::default()))
        .await
        .expect("retrieve");
    assert!(none.hits.is_empty(), "an empty scope denies every source");
}

/// The caller's sources narrow the recall itself: however many records other
/// sources hold, they cannot fill the events budget before a permitted one.
#[tokio::test]
async fn a_restricted_caller_is_not_crowded_out_by_other_sources() {
    let (provider, _state) = hosted().await;
    sync(
        &provider,
        "gmail:you",
        (0..12)
            .map(|i| item(&format!("y{i}"), "oolong order", None))
            .collect(),
    )
    .await;
    sync(
        &provider,
        "gmail:me",
        vec![item("m1", "oolong invoice", None)],
    )
    .await;
    let scope = SourceScope::new(["gmail:me"]);
    let response = provider
        .fast_retrieve("oolong", fast(1), Some(&scope))
        .await
        .expect("retrieve");
    let bodies: Vec<&str> = response.hits.iter().map(|h| h.content.as_str()).collect();
    assert_eq!(bodies, ["oolong invoice"]);
}

#[tokio::test]
async fn an_empty_query_or_limit_is_handled_before_any_request() {
    let (provider, _state) = hosted().await;
    assert!(matches!(
        provider.fast_retrieve("  ", fast(5), None).await,
        Err(MemoryError::Invalid(_))
    ));
    let none = provider
        .fast_retrieve("x", fast(0), None)
        .await
        .expect("retrieve");
    assert!(none.hits.is_empty());
}

#[tokio::test]
async fn a_window_reads_what_was_observed_inside_it_in_time_order() {
    let (provider, _state) = hosted().await;
    let day = 86_400_000;
    let base = 1_767_225_600_000; // 2026-01-01
    sync(
        &provider,
        "notion:ws",
        vec![
            item("late", "late page", Some(base + 3 * day)),
            item("early", "early page", Some(base + day)),
            item("outside", "outside page", Some(base + 10 * day)),
        ],
    )
    .await;
    let response = provider
        .cover_window(
            &CoverWindowQuery {
                since_ms: base,
                until_ms: base + 5 * day,
                source_id: None,
                source_kind: Some(SourceKind::Document),
                limit: None,
            },
            None,
        )
        .await
        .expect("window");
    let bodies: Vec<&str> = response.hits.iter().map(|h| h.content.as_str()).collect();
    assert_eq!(bodies, ["early page", "late page"]);
    let empty = provider
        .cover_window(
            &CoverWindowQuery {
                since_ms: base,
                until_ms: base,
                source_id: None,
                source_kind: None,
                limit: None,
            },
            None,
        )
        .await
        .expect("window");
    assert!(empty.hits.is_empty());
    assert!(matches!(
        provider
            .cover_window(
                &CoverWindowQuery {
                    since_ms: i64::MAX,
                    until_ms: i64::MAX,
                    source_id: None,
                    source_kind: None,
                    limit: None,
                },
                None,
            )
            .await,
        Err(MemoryError::Invalid(_))
    ));
}

#[tokio::test]
async fn one_sources_records_are_retrieved_by_its_label() {
    let (provider, _state) = hosted().await;
    sync(
        &provider,
        "gmail:me",
        vec![
            item("1", "first mail", None),
            item("2", "second mail", None),
        ],
    )
    .await;
    sync(&provider, "gmail:you", vec![item("3", "their mail", None)]).await;
    let response = provider
        .retrieve_source(
            &SourceRetrievalQuery {
                source_id: Some("gmail:me".to_string()),
                source_kind: Some(SourceKind::Email),
                time_window_days: None,
                query: None,
                limit: 10,
            },
            None,
        )
        .await
        .expect("source");
    assert_eq!(response.hits.len(), 2);
    assert!(response.hits.iter().all(|h| h.tree_scope == "gmail:me"));
    let none = provider
        .retrieve_source(
            &SourceRetrievalQuery {
                source_id: None,
                source_kind: None,
                time_window_days: None,
                query: None,
                limit: 0,
            },
            None,
        )
        .await
        .expect("source");
    assert!(none.hits.is_empty());
}

#[tokio::test]
async fn a_recent_window_keeps_what_was_observed_lately() {
    let (provider, _state) = hosted().await;
    let now = Utc::now().timestamp_millis();
    let month_ago = now - 30 * 86_400_000;
    sync(
        &provider,
        "notion:ws",
        vec![
            item("fresh", "fresh page", Some(now)),
            item("stale", "stale page", Some(month_ago)),
        ],
    )
    .await;
    let response = provider
        .retrieve_source(
            &SourceRetrievalQuery {
                source_id: None,
                source_kind: None,
                time_window_days: Some(7),
                query: Some("page".to_string()),
                limit: 10,
            },
            None,
        )
        .await
        .expect("source");
    let bodies: Vec<&str> = response.hits.iter().map(|h| h.content.as_str()).collect();
    assert_eq!(bodies, ["fresh page"]);
}

#[test]
fn a_recent_window_reaches_a_day_past_now() {
    let window = recent_window(7);
    let bound = |at: usize| {
        let stamp = window["valid_during"][at].as_str().expect("a bound");
        DateTime::parse_from_rfc3339(stamp).expect("rfc 3339")
    };
    assert_eq!(bound(1) - bound(0), Duration::days(8));
}

#[tokio::test]
async fn leaves_are_read_by_event_id_and_only_this_accounts_count() {
    let (provider, state) = hosted().await;
    let outcome = provider
        .accept_source_items(
            "notion:ws",
            "composio",
            vec![item("1", "page one", None), item("2", "page two", None)],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    let foreign = outcome.ids[1].clone();
    state
        .foreign
        .lock()
        .expect("foreign")
        .insert(foreign.clone());
    let ids = vec![
        outcome.ids[0].clone(),
        outcome.ids[0].clone(),
        foreign,
        "no-such-event".to_string(),
    ];
    let leaves = provider.retrieve_leaves(&ids, None).await.expect("leaves");
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0].content, "page one");
    assert!(provider
        .retrieve_children(&leaves[0].node_id, 2, None, None, None)
        .await
        .expect("children")
        .is_empty());
}

#[tokio::test]
async fn namespace_recall_answers_the_top_of_the_engines_ranking() {
    let (provider, _state) = hosted().await;
    for (key, body, session) in [
        ("a", "tea one", Some("s1")),
        ("b", "tea two", None),
        ("c", "tea three", None),
        ("d", "tea four", None),
        ("e", "coffee", None),
    ] {
        provider
            .store(
                "notes",
                key,
                body,
                MemoryCategory::Core,
                session,
                MemoryTaint::Internal,
            )
            .await
            .expect("store");
    }
    let hits = provider
        .recall_namespace_scored("notes", "tea", 10, None)
        .await
        .expect("recall");
    assert_eq!(hits.len(), RANKED_NOTES);
    let scores: Vec<f64> = hits.iter().map(|h| h.score).collect();
    assert_eq!(scores.len(), 3);
    assert_eq!(scores[0], 1.0);
    assert!(scores.windows(2).all(|pair| pair[0] > pair[1]));
    for hit in &hits {
        let signals = &hit.score_breakdown;
        assert_eq!(signals.final_score, hit.score);
        assert_eq!(
            (
                signals.vector_similarity,
                signals.keyword_relevance,
                signals.graph_relevance,
                signals.episodic_relevance,
                signals.freshness,
            ),
            (0.0, 0.0, 0.0, 0.0, 0.0),
            "a rank carries no signal"
        );
    }
    let without = provider
        .recall_namespace_scored("notes", "tea", 10, Some("s1"))
        .await
        .expect("recall");
    assert!(without.iter().all(|h| h.key != "a"));
    assert!(provider
        .recall_namespace_scored("notes", "  ", 10, None)
        .await
        .expect("recall")
        .is_empty());
    let one = provider
        .recall_namespace_scored("notes", "tea", 1, None)
        .await
        .expect("recall");
    assert_eq!(one.len(), 1);
}

#[tokio::test]
async fn recent_recall_is_newest_first() {
    let (provider, _state) = hosted().await;
    for key in ["old", "new"] {
        provider
            .store(
                "notes",
                key,
                key,
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await
            .expect("store");
    }
    let hits = provider
        .recall_namespace_recent("notes", 1)
        .await
        .expect("recent");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, "new");
    assert_eq!(hits[0].score, hits[0].score_breakdown.freshness);
    assert!(
        hits[0].score > 0.0,
        "recency is the signal recent recall reports"
    );
    assert!(provider
        .recall_namespace_recent("notes", 0)
        .await
        .expect("recent")
        .is_empty());
}

#[tokio::test]
async fn entities_refuse_an_unknown_kind_and_are_otherwise_not_served() {
    let (provider, _state) = hosted().await;
    let bogus = ["planet".to_string()];
    assert!(matches!(
        provider.search_entities("x", Some(&bogus), 5).await,
        Err(MemoryError::Invalid(_))
    ));
    let known = ["person".to_string()];
    assert!(matches!(
        provider.search_entities("x", Some(&known), 5).await,
        Err(MemoryError::Unsupported { .. })
    ));
}
