//! Explore: the same counts as the listing walk, from first events only,
//! without assembling conversations or chunked documents.

use super::*;
use crate::cortex::testing::{both, thread_meta};
use tinymemory_api::{
    Facet, MemoryEngine, MemoryMeta, Role, StoreItem, Turn, explore::explore_by_listing,
};

/// Whether `request` is an item lookup (the labelled walk that assembles a
/// conversation or a chunked document).
fn is_item_lookup(request: &str) -> bool {
    request.contains("labels=tm%3Ai%3A")
}

fn agent_meta(thread: &str, agent: &str) -> MemoryMeta {
    let mut meta = thread_meta(thread);
    meta.agent_id = Some(agent.to_string());
    meta
}

fn conversation(thread: &str, agent: &str, turns: usize) -> StoreItem {
    StoreItem::Conversation {
        turns: (0..turns)
            .map(|i| Turn::new(Role::User, format!("{thread} turn {i}")))
            .collect(),
        meta: agent_meta(thread, agent),
    }
}

async fn store_mixed(engine: &CortexEngine) {
    for item in crate::cortex::testing::sample_items() {
        engine.store(item).await.unwrap();
    }
    for (thread, agent, turns) in [("a", "planner", 3), ("b", "planner", 40), ("c", "coder", 2)] {
        engine
            .store(conversation(thread, agent, turns))
            .await
            .unwrap();
    }
    // Long enough to be stored in several pieces.
    let long = "word ".repeat(40_000);
    engine
        .store(StoreItem::document(long, thread_meta("big")))
        .await
        .unwrap();
}

#[tokio::test]
async fn counts_match_the_listing_walk_for_every_facet() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        for facet in [Facet::Kind, Facet::Agent, Facet::Thread, Facet::Namespace] {
            let fast = engine
                .explore(ExploreRequest::new(facet, 50))
                .await
                .unwrap();
            let slow = explore_by_listing(&engine, ExploreRequest::new(facet, 50))
                .await
                .unwrap();
            assert_eq!(fast, slow, "facet {}", facet.as_str());
        }
    }
}

#[tokio::test]
async fn counts_each_conversation_once_by_agent() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        let page = engine
            .explore(ExploreRequest::new(Facet::Agent, 50))
            .await
            .unwrap();
        let planner = page.buckets.iter().find(|b| b.value == "planner").unwrap();
        let coder = page.buckets.iter().find(|b| b.value == "coder").unwrap();
        assert_eq!((planner.count, coder.count), (2, 1));
    }
}

#[tokio::test]
async fn never_assembles_an_item() {
    for (engine, state) in both().await {
        store_mixed(&engine).await;
        let before = state.requests().len();
        engine
            .explore(ExploreRequest::new(Facet::Kind, 50))
            .await
            .unwrap();
        let lookups: Vec<String> = state.requests()[before..]
            .iter()
            .filter(|r| is_item_lookup(r))
            .cloned()
            .collect();
        assert!(lookups.is_empty(), "item lookups: {lookups:?}");

        // The listing walk this replaces does assemble them.
        let before = state.requests().len();
        explore_by_listing(&engine, ExploreRequest::new(Facet::Kind, 50))
            .await
            .unwrap();
        assert!(
            state.requests()[before..].iter().any(|r| is_item_lookup(r)),
            "the listing walk made no item lookup; the check above proves nothing"
        );
    }
}

#[tokio::test]
async fn stops_at_the_scan_limit_and_says_so() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        let mut req = ExploreRequest::new(Facet::Kind, 50);
        req.scan_limit = 2;
        let page = engine.explore(req).await.unwrap();
        assert_eq!(page.total, 2);
        assert!(page.truncated);

        let all = engine
            .explore(ExploreRequest::new(Facet::Kind, 50))
            .await
            .unwrap();
        assert!(!all.truncated);
        assert!(all.total > 2);
    }
}

#[tokio::test]
async fn respects_the_filter() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        let mut req = ExploreRequest::new(Facet::Agent, 50);
        req.filter.agent_id = Some("coder".to_string());
        let page = engine.explore(req).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.buckets.len(), 1);
        assert_eq!(page.buckets[0].value, "coder");
    }
}

#[tokio::test]
async fn refuses_an_invalid_request_before_reading() {
    for (engine, state) in both().await {
        let before = state.requests().len();
        let err = engine
            .explore(ExploreRequest::new(Facet::Kind, 0))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("explore limit"), "{err}");
        assert_eq!(state.requests().len(), before);
    }
}
