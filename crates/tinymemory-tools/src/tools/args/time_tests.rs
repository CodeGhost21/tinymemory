//! Tests for reading `refers_to`.

use super::*;
use serde_json::json;

fn read(value: serde_json::Value, zone: Option<&str>) -> Result<Option<TimeHint>> {
    let scope = ToolScope {
        zone: zone.map(str::to_owned),
        ..ToolScope::default()
    };
    let args = Args::parse("memory_fetch", &value, &["query", "refers_to"]).unwrap();
    time_hint(&args, "refers_to", &scope)
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
}

#[test]
fn a_range_is_read_in_the_scope_zone() {
    let hint = read(
        json!({"query": "q", "refers_to": {"from": "2026-10-03", "to": "2026-10-04"}}),
        Some("Asia/Kolkata"),
    )
    .unwrap()
    .unwrap();
    assert_eq!((hint.from, hint.to), (day(3), day(4)));
    assert_eq!(hint.zone.as_deref(), Some("Asia/Kolkata"));
}

#[test]
fn a_missing_to_is_a_single_day_and_no_range_is_none() {
    let hint = read(
        json!({"query": "q", "refers_to": {"from": "2026-10-03"}}),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!((hint.from, hint.to, hint.zone), (day(3), day(3), None));
    assert_eq!(read(json!({"query": "q"}), None).unwrap(), None);
}

#[test]
fn bad_ranges_are_refused_naming_the_field() {
    for (value, field) in [
        (json!({"from": "03/10/2026"}), "from"),
        (json!({"to": "2026-10-03"}), "from"),
        (json!({"from": "2026-10-05", "to": "2026-10-03"}), "to"),
        (json!({"from": "2026-10-03", "zone": "IST"}), "zone"),
    ] {
        let error = read(json!({"query": "q", "refers_to": value}), None).unwrap_err();
        assert!(error.to_string().contains(field), "{field}: {error}");
    }
    let error = read(json!({"query": "q", "refers_to": "yesterday"}), None).unwrap_err();
    assert!(error.to_string().contains("refers_to"), "{error}");
}

#[test]
fn a_bad_host_zone_is_reported_as_the_zone_not_the_models_dates() {
    let error = read(
        json!({"query": "q", "refers_to": {"from": "2026-10-03"}}),
        Some("IST"),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("time zone") && message.contains("IST"),
        "{message}"
    );
    assert!(!message.contains("`to`"), "{message}");
}
