//! Holistic recall with a date: a hint that arrives during the read lifts the
//! right day's hits into a section that shows fewer than it read.

use chrono::{NaiveDate, TimeZone, Utc};
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{ItemKind, MemoryEngine, MemoryMeta, MetaFilter, StoreItem, TimeHint};

use super::*;

fn on_day(text: &str, day: u32) -> StoreItem {
    StoreItem::document(
        text,
        MemoryMeta {
            observed_at: Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).single(),
            ..MemoryMeta::default()
        },
    )
}

fn day(d: u32) -> TimeHint {
    let d = NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
    TimeHint::new(d, d, None).unwrap()
}

async fn seeded() -> ReferenceEngine {
    let engine = ReferenceEngine::new();
    for (text, d) in [
        ("dinner at Toit", 1),
        ("dinner at Truffles", 3),
        ("dinner at home", 5),
    ] {
        engine.store(on_day(text, d)).await.unwrap();
    }
    engine
}

fn one_dinner() -> HolisticRecall {
    HolisticRecall::new(
        Some("dinner".into()),
        vec![ScopeSection::fetch(
            "Docs",
            MetaFilter::kinds([ItemKind::Document]),
            1,
        )],
    )
}

fn shown(pack: &ContextPack) -> Vec<String> {
    pack.sections[0]
        .hits
        .iter()
        .map(|h| h.text.clone())
        .collect()
}

#[tokio::test]
async fn a_late_date_lifts_its_day_into_a_section_that_shows_one() {
    let engine = seeded().await;
    let plain = holistic_recall(&engine, &one_dinner()).await.unwrap();
    assert_ne!(
        shown(&plain),
        ["dinner at Truffles"],
        "fixture: day 3 is not on top undated"
    );

    // Resolves only after the reads have been started.
    let late = async {
        tokio::task::yield_now().await;
        Some(day(3))
    };
    let dated = holistic_recall_dated(&engine, &one_dinner(), late)
        .await
        .unwrap();
    assert_eq!(shown(&dated), ["dinner at Truffles"]);
    assert!(
        dated.markdown.contains("dinner at Truffles"),
        "{}",
        dated.markdown
    );
}

#[tokio::test]
async fn no_date_in_time_leaves_the_pack_as_it_was() {
    let engine = seeded().await;
    let plain = holistic_recall(&engine, &one_dinner()).await.unwrap();
    let dated = holistic_recall_dated(&engine, &one_dinner(), async { None })
        .await
        .unwrap();
    assert_eq!(shown(&dated), shown(&plain));
    assert_eq!(dated.markdown, plain.markdown);
}

#[tokio::test]
async fn a_hint_in_an_unknown_zone_is_ignored_not_fatal() {
    let engine = seeded().await;
    let plain = holistic_recall(&engine, &one_dinner()).await.unwrap();
    let mut bad = day(3);
    bad.zone = Some("IST".into());
    let dated = holistic_recall_dated(&engine, &one_dinner(), async { Some(bad) })
        .await
        .unwrap();
    assert_eq!(shown(&dated), shown(&plain));
}

#[tokio::test]
async fn a_date_known_up_front_ranks_the_same_way() {
    let engine = seeded().await;
    let request = HolisticRecall {
        refers_to: Some(day(3)),
        ..one_dinner()
    };
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(shown(&pack), ["dinner at Truffles"]);
}

/// The reference engine, recording what reaches recall and fetch; recall
/// always fails, so an answered section falls back to fetch.
struct Recording {
    inner: ReferenceEngine,
    recalls: std::sync::Mutex<Vec<Option<TimeHint>>>,
    fetches: std::sync::Mutex<Vec<Option<TimeHint>>>,
}

#[async_trait::async_trait]
impl MemoryEngine for Recording {
    fn descriptor(&self) -> &tinymemory_api::EngineDescriptor {
        self.inner.descriptor()
    }
    async fn health(&self) -> tinymemory_api::EngineHealth {
        tinymemory_api::EngineHealth::Ok
    }
    async fn recall(
        &self,
        req: tinymemory_api::RecallRequest,
    ) -> tinymemory_api::Result<tinymemory_api::RecallAnswer> {
        self.recalls.lock().unwrap().push(req.refers_to);
        Err(tinymemory_api::Error::Unavailable("no answers here".into()))
    }
    async fn fetch(
        &self,
        req: tinymemory_api::FetchRequest,
    ) -> tinymemory_api::Result<tinymemory_api::FetchPage> {
        self.fetches.lock().unwrap().push(req.refers_to.clone());
        self.inner.fetch(req).await
    }
    async fn store(&self, item: StoreItem) -> tinymemory_api::Result<tinymemory_api::StoreReceipt> {
        self.inner.store(item).await
    }
    async fn forget(
        &self,
        target: tinymemory_api::ForgetTarget,
    ) -> tinymemory_api::Result<tinymemory_api::ForgetReport> {
        self.inner.forget(target).await
    }
    async fn list(
        &self,
        req: tinymemory_api::ListRequest,
    ) -> tinymemory_api::Result<tinymemory_api::ListPage> {
        self.inner.list(req).await
    }
}

#[tokio::test]
async fn an_answered_section_and_its_fetch_fallback_carry_the_date() {
    let engine = Recording {
        inner: seeded().await,
        recalls: Default::default(),
        fetches: Default::default(),
    };
    let mut section = ScopeSection::answer(
        "Dinners",
        "where did I eat",
        MetaFilter::kinds([ItemKind::Document]),
        1,
    );
    if let SectionQuery::Answer {
        fallback_to_fetch, ..
    } = &mut section.query
    {
        *fallback_to_fetch = true;
    }
    let request = HolisticRecall {
        refers_to: Some(day(3)),
        ..HolisticRecall::new(None, vec![section])
    };
    holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(*engine.recalls.lock().unwrap(), [Some(day(3))]);
    assert_eq!(*engine.fetches.lock().unwrap(), [Some(day(3))]);
}

#[tokio::test]
async fn a_fetch_section_with_no_query_keeps_its_newest_first_order() {
    let engine = seeded().await;
    let latest = HolisticRecall::new(
        None,
        vec![ScopeSection::fetch(
            "Docs",
            MetaFilter::kinds([ItemKind::Document]),
            3,
        )],
    );
    let plain = holistic_recall(&engine, &latest).await.unwrap();
    let dated = holistic_recall_dated(&engine, &latest, async { Some(day(3)) })
        .await
        .unwrap();
    assert_eq!(shown(&dated), shown(&plain), "newest first, not date first");
}
