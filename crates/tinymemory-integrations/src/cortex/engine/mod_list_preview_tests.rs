//! Preview listings: the listing's items, order and cursors, each
//! conversation and chunked document taken from its first event, with no
//! item lookups.

use super::*;
use crate::cortex::testing::{both, thread_meta};
use tinymemory_api::{ItemKind, MetaFilter, Role, Turn};

fn is_item_lookup(request: &str) -> bool {
    request.contains("labels=tm%3Ai%3A")
}

async fn store_mixed(engine: &CortexEngine) {
    for item in crate::cortex::testing::sample_items() {
        engine.store(item).await.unwrap();
    }
    for thread in ["a", "b", "c"] {
        let item = StoreItem::Conversation {
            turns: (0..30)
                .map(|i| Turn::new(Role::User, format!("{thread} turn {i}")))
                .collect(),
            meta: thread_meta(thread),
        };
        engine.store(item).await.unwrap();
    }
    engine
        .store(StoreItem::document("word ".repeat(40_000), thread_meta("big")))
        .await
        .unwrap();
}

async fn all(engine: &CortexEngine, limit: usize, preview: bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut cursor = None;
    loop {
        let mut req = ListRequest::new(MetaFilter::default(), limit);
        req.cursor = cursor;
        let page = if preview {
            engine.list_preview(req).await.unwrap()
        } else {
            engine.list(req).await.unwrap()
        };
        hits.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    hits
}

#[tokio::test]
async fn lists_the_same_items_in_the_same_order() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        for limit in [1, 2, 5, 50] {
            let whole = all(&engine, limit, false).await;
            let preview = all(&engine, limit, true).await;
            let ids = |hits: &[Hit]| hits.iter().map(|h| h.id.clone()).collect::<Vec<_>>();
            assert_eq!(ids(&preview), ids(&whole), "limit {limit}");
            for (p, w) in preview.iter().zip(&whole) {
                assert_eq!((p.kind, &p.meta), (w.kind, &w.meta));
            }
        }
    }
}

#[tokio::test]
async fn a_conversation_previews_as_its_first_turn() {
    for (engine, _state) in both().await {
        store_mixed(&engine).await;
        let preview = all(&engine, 50, true).await;
        let whole = all(&engine, 50, false).await;
        for (p, w) in preview
            .iter()
            .zip(&whole)
            .filter(|(p, _)| p.kind == ItemKind::Conversation)
        {
            assert!(p.text.contains("turn 0"), "{}", p.text);
            assert!(!p.text.contains("turn 1"), "{}", p.text);
            assert!(w.text.contains("turn 29"));
        }
    }
}

#[tokio::test]
async fn makes_no_item_lookup() {
    for (engine, state) in both().await {
        store_mixed(&engine).await;
        let before = state.requests().len();
        all(&engine, 50, true).await;
        let lookups: Vec<String> = state.requests()[before..]
            .iter()
            .filter(|r| is_item_lookup(r))
            .cloned()
            .collect();
        assert!(lookups.is_empty(), "item lookups: {lookups:?}");

        let before = state.requests().len();
        all(&engine, 50, false).await;
        assert!(
            state.requests()[before..].iter().any(|r| is_item_lookup(r)),
            "the whole listing made no item lookup; the check above proves nothing"
        );
    }
}
