//! Fetch's conversation hits: a one-turn conversation comes whole from its
//! pack event with no lookup, a longer one is assembled from its turns, and
//! the lookups run a few at a time.

use std::sync::atomic::Ordering;

use super::*;
use tinymemory_api::{FetchMode, GetRequest, ItemId, MemoryEngine, MemoryMeta, Role, Turn};

use crate::cortex::testing::{Shared, both, direct_double, direct_engine};

/// Event listings the double answered, on either wire.
fn listings(state: &Shared) -> usize {
    state.count("GET /v1/events") + state.count("GET /memory/events")
}

/// A conversation of `turns` at agent `agent`'s node, every turn naming the
/// launch, as a host logs one item per turn when `turns` is one.
fn conversation(agent: usize, turns: usize) -> StoreItem {
    StoreItem::Conversation {
        turns: (0..turns)
            .map(|turn| {
                let role = if turn % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                Turn::new(
                    role,
                    format!("Agent {agent} turn {turn}: the launch moved."),
                )
            })
            .collect(),
        meta: MemoryMeta {
            namespace: Namespace::agent(&format!("a{agent}")),
            thread_id: Some(format!("thread-{agent}")),
            ..MemoryMeta::default()
        },
    }
}

/// The conversation hits of a fetch for the launch.
async fn launch_hits(engine: &impl MemoryEngine) -> Vec<tinymemory_api::Hit> {
    engine
        .fetch(FetchRequest::new("launch", FetchMode::Hybrid, 20))
        .await
        .unwrap()
        .hits
        .into_iter()
        .filter(|hit| hit.kind == ItemKind::Conversation)
        .collect()
}

#[tokio::test]
async fn a_one_turn_conversation_hit_is_whole_without_a_lookup() {
    for (engine, state) in both().await {
        let wire = engine.wire();
        let items: Vec<StoreItem> = (0..6).map(|agent| conversation(agent, 1)).collect();
        engine.store_many(items.clone()).await.unwrap();

        let before = listings(&state);
        let hits = launch_hits(&engine).await;
        assert_eq!(hits.len(), 6, "{wire:?}: {hits:?}");
        assert_eq!(listings(&state), before, "{wire:?}: no event listing");

        // The same hits as a whole-item read, which assembles from the log.
        let whole = engine
            .get(GetRequest {
                ids: hits.iter().map(|hit| hit.id.clone()).collect(),
                reach: None,
            })
            .await
            .unwrap();
        assert_eq!(whole.len(), 6, "{wire:?}");
        for hit in &hits {
            let read = whole.iter().find(|read| read.id == hit.id).unwrap();
            assert_eq!(hit.text, read.text, "{wire:?}");
            assert_eq!(hit.meta, read.meta, "{wire:?}");
            assert_eq!(hit.confidence, read.confidence, "{wire:?}");
            let item = items
                .iter()
                .find(|item| ItemId::new(item.fingerprint()) == hit.id)
                .unwrap();
            assert_eq!(hit.text, item.render_text(), "{wire:?}");
        }
    }
}

#[tokio::test]
async fn a_longer_conversation_hit_is_still_assembled_whole() {
    for (engine, state) in both().await {
        let wire = engine.wire();
        let item = conversation(0, 3);
        engine.store(item.clone()).await.unwrap();

        let before = listings(&state);
        let hits = launch_hits(&engine).await;
        assert_eq!(hits.len(), 1, "{wire:?}: {hits:?}");
        assert_eq!(hits[0].text, item.render_text(), "{wire:?}: every turn");
        assert!(
            listings(&state) > before,
            "{wire:?}: its turns were looked up"
        );
    }
}

#[tokio::test]
async fn assembly_lookups_run_a_few_at_a_time() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let items: Vec<StoreItem> = (0..8).map(|agent| conversation(agent, 2)).collect();
    engine.store_many(items).await.unwrap();

    state.listing_delay_ms.store(40, Ordering::SeqCst);
    let hits = launch_hits(&engine).await;
    assert_eq!(hits.len(), 8, "{hits:?}");
    let peak = state.listings_peak.load(Ordering::SeqCst);
    assert!(peak > 1, "lookups overlap: peak {peak}");
    assert!(
        peak <= super::super::items::LOOKUPS_AT_ONCE,
        "at most LOOKUPS_AT_ONCE at once: peak {peak}"
    );
}
