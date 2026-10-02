//! Listing: paging, the engine's duplicate copies, label narrowing, each
//! conversation once, and the page ceiling.

use super::*;
use crate::testing::{both, direct_engine, serve, thread_meta};
use std::collections::HashSet;
use tinymemory_api::{ItemKind, MetaFilter, Role, Turn};

async fn store_docs(engine: &CortexEngine, count: usize) -> HashSet<String> {
    let mut ids = HashSet::new();
    for i in 0..count {
        let item = StoreItem::document(format!("note number {i}"), thread_meta("t"));
        ids.insert(engine.store(item).await.unwrap().id.0);
    }
    ids
}

#[tokio::test]
async fn paging_returns_every_item_exactly_once_despite_duplicate_copies() {
    for (engine, _state) in both().await {
        let stored = store_docs(&engine, 7).await;
        for limit in [1, 2, 3, 7, 50] {
            let mut seen = Vec::new();
            let mut cursor = None;
            loop {
                let mut req = ListRequest::new(MetaFilter::default(), limit);
                req.cursor = cursor;
                let page = engine.list(req).await.unwrap();
                assert!(page.items.len() <= limit);
                seen.extend(page.items.into_iter().map(|h| h.id.0));
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            assert_eq!(seen.len(), 7, "limit {limit}: {seen:?}");
            assert_eq!(seen.into_iter().collect::<HashSet<_>>(), stored);
        }
    }
}

#[tokio::test]
async fn a_cursor_crosses_from_one_kind_scope_to_the_next() {
    for (engine, _state) in both().await {
        for item in crate::testing::sample_items() {
            engine.store(item).await.unwrap();
        }
        let mut kinds = Vec::new();
        let mut cursor = None;
        loop {
            let mut req = ListRequest::new(MetaFilter::default(), 1);
            req.cursor = cursor;
            let page = engine.list(req).await.unwrap();
            kinds.extend(page.items.iter().map(|h| h.kind));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(
            kinds,
            vec![ItemKind::Document, ItemKind::Conversation, ItemKind::Learning]
        );
    }
}

#[tokio::test]
async fn a_long_conversation_is_listed_once_with_every_turn() {
    for (engine, _state) in both().await {
        let turns: Vec<Turn> = (0..150)
            .map(|i| Turn::new(Role::User, format!("turn {i}")))
            .collect();
        let item = StoreItem::Conversation {
            turns,
            meta: thread_meta("long"),
        };
        engine.store(item.clone()).await.unwrap();
        let page = engine
            .list(ListRequest::new(MetaFilter::default(), 10))
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "{:?}", engine.wire());
        assert_eq!(page.items[0].text, item.render_text());
        assert!(page.next_cursor.is_none());
    }
}

#[tokio::test]
async fn a_labelled_filter_narrows_server_side_and_is_rechecked() {
    for (engine, state) in both().await {
        for item in crate::testing::sample_items() {
            engine.store(item).await.unwrap();
        }
        let mut filter = MetaFilter {
            thread_id: Some("t-learn".into()),
            ..MetaFilter::default()
        };
        let page = engine.list(ListRequest::new(filter.clone(), 10)).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, ItemKind::Learning);
        let label = crate::envelope::labels::item("x").replace("tm:i:", "");
        let thread_label = format!(
            "labels=tm%3At%3A{}",
            crate::envelope::labels::digest("t-learn")
        );
        assert_ne!(label, thread_label);
        assert!(
            state.requests().iter().any(|r| r.contains(&thread_label)),
            "the thread label narrows the listing"
        );

        // A field with no label (a folder prefix) is applied client-side.
        filter.thread_id = None;
        filter.folder = Some("/nowhere".into());
        let none = engine.list(ListRequest::new(filter, 10)).await.unwrap();
        assert!(none.items.is_empty());
    }
}

#[tokio::test]
async fn a_malformed_cursor_is_an_invalid_request() {
    let (endpoint, _state) = crate::testing::direct_double().await;
    let mut req = ListRequest::new(MetaFilter::default(), 3);
    req.cursor = Some("garbage".into());
    let error = direct_engine(&endpoint).list(req).await.unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
}

/// A listing that always claims more and always hands back a fresh cursor,
/// holding nothing this crate wrote.
async fn endless_listing() -> String {
    use axum::extract::Query;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::BTreeMap;
    let app = Router::new().route(
        "/v1/events",
        get(|Query(params): Query<BTreeMap<String, String>>| async move {
            let next: u64 = params.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0) + 1;
            Json(serde_json::json!({
                "items": [{ "id": format!("foreign-{next}"), "content": { "text": "not ours" } }],
                "has_more": true,
                "next_cursor": next.to_string(),
            }))
        }),
    );
    serve(app).await
}

#[tokio::test]
async fn a_walk_past_the_page_ceiling_is_refused_not_truncated() {
    let engine = direct_engine(&endless_listing().await);
    let filter = MetaFilter {
        repo: Some("o/r".into()),
        ..MetaFilter::default()
    };
    let error = engine.forget(ForgetTarget::Filter(filter)).await.unwrap_err();
    assert!(error.to_string().contains("pages"), "{error}");
    let listed = engine.list(ListRequest::new(MetaFilter::default(), 5)).await.unwrap_err();
    assert!(listed.to_string().contains("pages"), "{listed}");
}

#[tokio::test]
async fn a_cursor_that_does_not_advance_is_refused() {
    use axum::routing::get;
    use axum::{Json, Router};
    let app = Router::new().route(
        "/v1/events",
        get(|| async {
            Json(serde_json::json!({ "items": [], "has_more": true, "next_cursor": "same" }))
        }),
    );
    let engine = direct_engine(&serve(app).await);
    let mut req = ListRequest::new(MetaFilter::default(), 5);
    req.cursor = None;
    let error = engine.list(req).await.unwrap_err();
    assert!(error.to_string().contains("does not advance"), "{error}");
}
