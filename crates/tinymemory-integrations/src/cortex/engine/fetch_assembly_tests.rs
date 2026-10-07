//! Fetch's conversation hits are assembled from their turns, a few
//! namespaces at a time.

use std::sync::atomic::Ordering;

use super::*;
use tinymemory_api::{FetchMode, MemoryEngine, MemoryMeta, Namespace, Role, StoreItem, Turn};

use crate::cortex::testing::{direct_double, direct_engine};

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
