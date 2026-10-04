//! `store_many` on both wires: every item listed on return, ranked recall
//! probed for the last item only.

use super::*;
use crate::cortex::testing::both;
use tinymemory_api::{ListRequest, MemoryEngine, MemoryMeta, MetaFilter};

fn doc(text: &str) -> StoreItem {
    StoreItem::document(text, MemoryMeta::default())
}

#[tokio::test]
async fn a_batch_is_listed_on_return_and_settles_only_its_last_item() {
    for (engine, state) in both().await {
        let wire = engine.wire();
        let items = vec![
            doc("first bulk note"),
            doc("second bulk note"),
            doc("third bulk note"),
        ];
        let receipts = engine.store_many(items.clone()).await.unwrap();
        let ids: Vec<String> = receipts.iter().map(|r| r.id.as_str().to_string()).collect();
        let fingerprints: Vec<String> = items.iter().map(StoreItem::fingerprint).collect();
        assert_eq!(ids, fingerprints, "{wire:?}: receipts in item order");

        let listed = engine
            .list(ListRequest::new(MetaFilter::default(), 10))
            .await
            .unwrap();
        assert_eq!(listed.items.len(), 3, "{wire:?}");

        let probed = |text: &str| {
            state
                .seen
                .lock()
                .unwrap()
                .recalls
                .iter()
                .filter(|body| body["query"].as_str().is_some_and(|q| q.contains(text)))
                .count()
        };
        assert_eq!(probed("first bulk note"), 0, "{wire:?}");
        assert_eq!(probed("second bulk note"), 0, "{wire:?}");
        assert!(
            probed("third bulk note") >= 1,
            "{wire:?}: the last item settles"
        );

        let again = engine.store_many(items).await.unwrap();
        assert!(again.iter().all(|r| r.replayed), "{wire:?}");
    }
}

#[tokio::test]
async fn an_empty_or_invalid_batch_is_refused() {
    for (engine, _state) in both().await {
        assert!(matches!(
            engine.store_many(Vec::new()).await,
            Err(crate::cortex::Error::InvalidRequest(_))
        ));
        assert!(engine.store_many(vec![doc("   ")]).await.is_err());
    }
}

#[tokio::test]
async fn a_repeat_inside_a_batch_and_mixed_kinds_are_handled() {
    use tinymemory_api::LearningKind;
    for (engine, _state) in both().await {
        let wire = engine.wire();
        let learning = StoreItem::learning(
            "prefers tea",
            LearningKind::Preference,
            0.9,
            MemoryMeta::default(),
        );
        let receipts = engine
            .store_many(vec![doc("bulk doc"), learning, doc("bulk doc")])
            .await
            .unwrap();
        let replayed: Vec<bool> = receipts.iter().map(|r| r.replayed).collect();
        assert_eq!(replayed, [false, false, true], "{wire:?}");
        assert_eq!(receipts[0].id, receipts[2].id);
        let listed = engine
            .list(ListRequest::new(MetaFilter::default(), 10))
            .await
            .unwrap();
        assert_eq!(
            listed.items.len(),
            2,
            "{wire:?}: the repeat was not written twice"
        );
    }
}
