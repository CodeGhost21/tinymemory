//! Tests for [`TimeHint`]: validation, zone-aware days, the stable rank and
//! the wire form.

use super::*;
use chrono::TimeZone;

use crate::{ItemId, ItemKind, MemoryMeta};

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
}

fn hit(id: &str, observed: Option<(u32, u32)>) -> Hit {
    Hit {
        id: ItemId::new(id),
        kind: ItemKind::Conversation,
        text: id.to_string(),
        meta: MemoryMeta {
            observed_at: observed.map(|(d, h)| Utc.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap()),
            ..MemoryMeta::default()
        },
        score: 0.0,
        confidence: None,
    }
}

#[test]
fn a_backwards_range_and_an_unknown_zone_are_refused() {
    assert!(TimeHint::new(day(4), day(3), None).is_err());
    assert!(TimeHint::new(day(3), day(3), Some("IST".into())).is_err());
    assert!(TimeHint::new(day(3), day(3), Some("Asia/Kolkata".into())).is_ok());
}

#[test]
fn days_are_read_in_the_hint_zone_not_utc() {
    let ist = TimeHint::new(day(3), day(3), Some("Asia/Kolkata".into())).unwrap();
    // 2026-10-02 20:00 UTC is 01:30 on the 3rd in India.
    assert!(ist.covers(Utc.with_ymd_and_hms(2026, 10, 2, 20, 0, 0).unwrap()));
    // 2026-10-03 20:00 UTC is already the 4th there.
    assert!(!ist.covers(Utc.with_ymd_and_hms(2026, 10, 3, 20, 0, 0).unwrap()));
    let utc = TimeHint::new(day(3), day(3), None).unwrap();
    assert!(!utc.covers(Utc.with_ymd_and_hms(2026, 10, 2, 20, 0, 0).unwrap()));
}

#[test]
fn rank_lifts_hinted_days_and_keeps_every_hit_in_engine_order() {
    let hint = TimeHint::new(day(3), day(4), None).unwrap();
    let mut hits = vec![
        hit("a", Some((1, 9))),
        hit("b", Some((4, 9))),
        hit("undated", None),
        hit("c", Some((3, 9))),
        hit("d", Some((6, 9))),
    ];
    hint.rank(&mut hits);
    let order: Vec<_> = hits.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(order, ["b", "c", "a", "undated", "d"]);
}

#[test]
fn the_wire_form_is_plain_dates_and_an_optional_zone() {
    let hint = TimeHint::new(day(3), day(5), Some("Asia/Kolkata".into())).unwrap();
    let json = serde_json::to_value(&hint).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"from": "2026-10-03", "to": "2026-10-05", "zone": "Asia/Kolkata"})
    );
    let bare: TimeHint =
        serde_json::from_value(serde_json::json!({"from": "2026-10-03", "to": "2026-10-03"}))
            .unwrap();
    assert_eq!(bare.zone, None);
}
