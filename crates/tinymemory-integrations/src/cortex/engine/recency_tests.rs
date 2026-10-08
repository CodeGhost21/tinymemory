//! Choosing the scopes a capped fetch reads: named first, then newest.

use super::*;
use crate::cortex::envelope::ScopeLayout;
use tinymemory_api::ItemKind;

fn scope(namespace: &str) -> KindScope {
    KindScope::new(
        &ScopeLayout::Legacy,
        namespace.parse().unwrap(),
        ItemKind::Document,
    )
}

fn at(day: u32) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&format!("2026-10-{day:02}T09:00:00Z"))
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn kept(dated: Vec<(KindScope, Option<DateTime<Utc>>)>, query: &str, max: usize) -> Vec<String> {
    choose(dated, &words(query), max)
        .into_iter()
        .map(|scope| scope.namespace.to_string())
        .collect()
}

#[test]
fn the_newest_scopes_are_kept_in_their_given_order() {
    let dated = vec![
        (scope("source:gmail"), at(1)),
        (scope("source:notion"), at(5)),
        (scope("source:files"), None),
        (scope("source:github/project:acme--api"), at(7)),
        (scope("source:github/project:acme--web"), at(3)),
    ];
    assert_eq!(
        kept(dated, "what changed this week", 3),
        [
            "source:notion",
            "source:github/project:acme--api",
            "source:github/project:acme--web"
        ]
    );
}

#[test]
fn a_scope_the_query_names_is_kept_however_old() {
    let dated = vec![
        (scope("source:gmail"), at(1)),
        (scope("source:notion"), at(5)),
        (scope("source:github/project:acme--api"), at(7)),
        (scope("source:github/project:acme--web"), at(3)),
    ];
    // `gmail` by its source id, `web` by a part of its collection id.
    assert_eq!(
        kept(dated, "Did Gmail or the web repo mention it?", 2),
        ["source:gmail", "source:github/project:acme--web"]
    );
}

#[test]
fn short_words_and_unknown_dates_name_nothing() {
    assert!(words("is it on").is_empty());
    let dated = vec![(scope("source:files"), None), (scope("source:web"), at(2))];
    assert_eq!(kept(dated, "is it on", 1), ["source:web"]);
}

#[test]
fn the_newest_observed_at_of_a_listing_is_its_recency() {
    let events = vec![
        serde_json::json!({ "context": { "observed_at": "2026-10-03T00:00:00Z" } }),
        serde_json::json!({ "context": { "observed_at": "2026-10-06T08:30:00+05:30" } }),
        serde_json::json!({ "context": {} }),
    ];
    assert_eq!(
        newest(&events).map(|at| at.to_rfc3339()),
        Some("2026-10-06T03:00:00+00:00".to_string())
    );
    assert_eq!(newest(&[]), None);
}

#[test]
fn a_write_makes_a_scope_known_and_recent() {
    let recency = Recency::default();
    assert_eq!(recency.get("p"), None);
    recency.set("q", None);
    assert_eq!(recency.get("q"), Some(None));
    recency.touch("p");
    assert!(recency.get("p").flatten().is_some());
}
