//! The model-facing filter, the explore facet and the fetch mode.

use super::*;
use serde_json::json;
use tinymemory_api::Error;

fn invalid_message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result {
        Err(Error::InvalidRequest(message)) => message,
        other => panic!("expected an invalid request, got {other:?}"),
    }
}

#[test]
fn every_filter_field_maps_onto_the_meta_filter_and_reach_stays_unset() {
    let value = json!({ "filter": {
        "kinds": ["learning", "document"],
        "sources": ["folder"],
        "tags_any": ["rust"],
        "workspace": "ws",
        "folder": "/notes",
        "file_path": "/notes/a.md",
        "repo": "o/r",
        "url": "https://example.com",
        "thread_id": "t1",
        "agent_id": "a1",
        "observed_after": "2026-01-01T00:00:00Z",
        "observed_before": "2026-02-01T00:00:00+01:00"
    }});
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    let filter = meta_filter(&args, "filter").unwrap();
    assert_eq!(filter.kinds, [ItemKind::Learning, ItemKind::Document]);
    assert_eq!(filter.sources, [SourceKind::Folder]);
    assert_eq!(filter.tags_any, ["rust"]);
    assert_eq!(filter.workspace.as_deref(), Some("ws"));
    assert_eq!(filter.folder.as_deref(), Some("/notes"));
    assert_eq!(filter.file_path.as_deref(), Some("/notes/a.md"));
    assert_eq!(filter.repo.as_deref(), Some("o/r"));
    assert_eq!(filter.url.as_deref(), Some("https://example.com"));
    assert_eq!(filter.thread_id.as_deref(), Some("t1"));
    assert_eq!(filter.agent_id.as_deref(), Some("a1"));
    assert_eq!(
        filter.observed_after.map(|at| at.to_rfc3339()),
        Some("2026-01-01T00:00:00+00:00".to_string())
    );
    assert_eq!(
        filter.observed_before.map(|at| at.to_rfc3339()),
        Some("2026-01-31T23:00:00+00:00".to_string())
    );
    assert!(filter.reach.is_none());
}

#[test]
fn an_absent_filter_is_empty() {
    let value = json!({});
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    assert!(meta_filter(&args, "filter").unwrap().is_empty());
}

#[test]
fn a_filter_field_outside_the_subset_is_refused() {
    let value = json!({ "filter": { "commit": "abc" } });
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    let text = invalid_message(meta_filter(&args, "filter"));
    assert!(
        text.contains("`filter.commit` is not an argument"),
        "{text}"
    );
}

#[test]
fn a_namespace_inside_the_filter_is_refused() {
    let value = json!({ "filter": { "namespace": "agent:other" } });
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    let text = invalid_message(meta_filter(&args, "filter"));
    assert!(
        text.contains("`filter.namespace` is fixed by the host"),
        "{text}"
    );
}

#[test]
fn a_bad_kind_or_timestamp_is_refused_by_field() {
    let value = json!({ "filter": { "kinds": ["memo"] } });
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    let text = invalid_message(meta_filter(&args, "filter"));
    assert!(
        text.contains("`filter.kinds` `memo` is not an item kind"),
        "{text}"
    );

    let value = json!({ "filter": { "observed_after": "yesterday" } });
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    let text = invalid_message(meta_filter(&args, "filter"));
    assert!(
        text.contains("`filter.observed_after` must be an rfc 3339 timestamp"),
        "{text}"
    );
}

#[test]
fn facets_are_read_and_the_namespace_facet_is_refused() {
    let value = json!({ "a": "thread", "b": "namespace", "c": "colour" });
    let args = Args::parse("t", &value, &["a", "b", "c"]).unwrap();
    assert_eq!(facet(&args, "a").unwrap(), Facet::Thread);
    assert!(invalid_message(facet(&args, "b")).contains("`b` must be one of"));
    assert!(facet(&args, "c").is_err());
    assert!(facet(&args, "missing").is_err());
}

#[test]
fn fetch_modes_follow_the_engine() {
    let value = json!({ "mode": "vector" });
    let args = Args::parse("t", &value, &["mode"]).unwrap();
    let text = invalid_message(fetch_mode(
        &args,
        "mode",
        &[FetchMode::Keyword],
        FetchMode::Keyword,
    ));
    assert!(text.contains("`mode` must be one of keyword"), "{text}");
    assert_eq!(
        fetch_mode(&args, "mode", &FetchMode::ALL, FetchMode::Hybrid).unwrap(),
        FetchMode::Vector
    );
    assert_eq!(
        fetch_mode(&args, "absent", &FetchMode::ALL, FetchMode::Hybrid).unwrap(),
        FetchMode::Hybrid
    );
}
