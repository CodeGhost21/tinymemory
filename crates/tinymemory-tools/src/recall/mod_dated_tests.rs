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
