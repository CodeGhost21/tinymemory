//! [`MetaFilter`] matching rules.

use chrono::TimeZone;

use super::*;

fn meta() -> MemoryMeta {
    MemoryMeta {
        workspace: Some("/ws".into()),
        folder: Some("/ws/src".into()),
        file_path: Some("/ws/src/lib.rs".into()),
        language: Some("rust".into()),
        repo: Some("o/r".into()),
        commit: Some("abc".into()),
        url: Some("https://x.test".into()),
        thread_id: Some("t1".into()),
        turns: Some(TurnRange { first: 0, last: 3 }),
        agent_id: Some("a1".into()),
        tool_call: Some(crate::ToolCallRef {
            name: "grep".into(),
            id: Some("c1".into()),
        }),
        source: crate::SourceRef {
            kind: SourceKind::Folder,
            id: Some("src_1".into()),
        },
        tags: vec!["x".into(), "y".into()],
        observed_at: Some(Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()),
    }
}

#[test]
fn an_empty_filter_matches_everything() {
    let filter = MetaFilter::default();
    assert!(filter.is_empty());
    assert!(filter.matches(ItemKind::Document, &meta()));
    assert!(filter.matches(ItemKind::Learning, &MemoryMeta::default()));
}

#[test]
fn every_exact_field_must_match() {
    let item = meta();
    let probes: Vec<(MetaFilter, MetaFilter)> = vec![
        (
            MetaFilter { workspace: Some("/ws".into()), ..Default::default() },
            MetaFilter { workspace: Some("/other".into()), ..Default::default() },
        ),
        (
            MetaFilter { language: Some("rust".into()), ..Default::default() },
            MetaFilter { language: Some("go".into()), ..Default::default() },
        ),
        (
            MetaFilter { repo: Some("o/r".into()), ..Default::default() },
            MetaFilter { repo: Some("o/x".into()), ..Default::default() },
        ),
        (
            MetaFilter { commit: Some("abc".into()), ..Default::default() },
            MetaFilter { commit: Some("def".into()), ..Default::default() },
        ),
        (
            MetaFilter { url: Some("https://x.test".into()), ..Default::default() },
            MetaFilter { url: Some("https://y.test".into()), ..Default::default() },
        ),
        (
            MetaFilter { thread_id: Some("t1".into()), ..Default::default() },
            MetaFilter { thread_id: Some("t2".into()), ..Default::default() },
        ),
        (
            MetaFilter { turns: Some(TurnRange { first: 0, last: 3 }), ..Default::default() },
            MetaFilter { turns: Some(TurnRange { first: 0, last: 4 }), ..Default::default() },
        ),
        (
            MetaFilter { agent_id: Some("a1".into()), ..Default::default() },
            MetaFilter { agent_id: Some("a2".into()), ..Default::default() },
        ),
        (
            MetaFilter { tool_call: Some("grep".into()), ..Default::default() },
            MetaFilter { tool_call: Some("ls".into()), ..Default::default() },
        ),
        (
            MetaFilter { source_id: Some("src_1".into()), ..Default::default() },
            MetaFilter { source_id: Some("src_2".into()), ..Default::default() },
        ),
        (
            MetaFilter { sources: vec![SourceKind::Folder], ..Default::default() },
            MetaFilter { sources: vec![SourceKind::Rss], ..Default::default() },
        ),
        (
            MetaFilter { tags_any: vec!["nope".into(), "y".into()], ..Default::default() },
            MetaFilter { tags_any: vec!["nope".into()], ..Default::default() },
        ),
        (
            MetaFilter::kinds([ItemKind::Document]),
            MetaFilter::kinds([ItemKind::Learning]),
        ),
    ];
    for (hit, miss) in probes {
        assert!(hit.matches(ItemKind::Document, &item), "{hit:?}");
        assert!(!miss.matches(ItemKind::Document, &item), "{miss:?}");
    }
}

#[test]
fn a_set_field_never_matches_an_item_without_it() {
    let filter = MetaFilter { repo: Some("o/r".into()), ..Default::default() };
    assert!(!filter.matches(ItemKind::Document, &MemoryMeta::default()));
}

#[test]
fn folder_and_file_path_match_on_a_path_boundary() {
    let item = meta();
    for prefix in ["/ws", "/ws/", "/ws/src", "/ws/src/"] {
        let filter = MetaFilter { folder: Some(prefix.into()), ..Default::default() };
        assert!(filter.matches(ItemKind::Document, &item), "{prefix}");
    }
    let filter = MetaFilter { folder: Some("/ws/sr".into()), ..Default::default() };
    assert!(!filter.matches(ItemKind::Document, &item));
    let filter = MetaFilter { file_path: Some("/ws/src".into()), ..Default::default() };
    assert!(filter.matches(ItemKind::Document, &item));
    let filter = MetaFilter { file_path: Some("/ws/src/lib".into()), ..Default::default() };
    assert!(!filter.matches(ItemKind::Document, &item));
}

#[test]
fn the_window_is_half_open_and_excludes_undated_items() {
    let at = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    let inclusive = MetaFilter { observed_after: Some(at), ..Default::default() };
    assert!(inclusive.matches(ItemKind::Document, &meta()));
    let exclusive = MetaFilter { observed_before: Some(at), ..Default::default() };
    assert!(!exclusive.matches(ItemKind::Document, &meta()));
    assert!(!inclusive.matches(ItemKind::Document, &MemoryMeta::default()));
}
