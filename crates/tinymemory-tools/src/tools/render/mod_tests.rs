//! Compact result rendering.

use super::*;
use tinymemory_api::chrono::{TimeZone, Utc};
use tinymemory_api::{
    Facet, FacetBucket, ItemKind, Namespace, SourceKind, SourceRef,
};

fn sample_hit() -> Hit {
    let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("notes".into()));
    meta.namespace = Namespace::agent("writer");
    meta.file_path = Some("/notes/a.md".into());
    meta.workspace = Some("hidden-from-the-model".into());
    meta.tags = vec!["rust".into()];
    meta.observed_at = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).single();
    Hit {
        id: ItemId::new("abc"),
        kind: ItemKind::Learning,
        text: "ownership moves".into(),
        meta,
        score: 0.1,
        confidence: Some(0.9),
    }
}

#[test]
fn a_hit_carries_the_subset_and_never_the_namespace() {
    let rendered = hit(&sample_hit());
    assert_eq!(
        rendered,
        json!({
            "id": "abc",
            "kind": "learning",
            "text": "ownership moves",
            "score": 0.1,
            "confidence": 0.9,
            "meta": {
                "source": { "kind": "folder", "id": "notes" },
                "file_path": "/notes/a.md",
                "tags": ["rust"],
                "observed_at": "2026-03-04T05:06:07Z"
            }
        })
    );
}

#[test]
fn a_bare_hit_leaves_optional_fields_out() {
    let bare = Hit {
        id: ItemId::new("x"),
        kind: ItemKind::Document,
        text: "t".into(),
        meta: MemoryMeta {
            source: SourceRef::default(),
            ..MemoryMeta::default()
        },
        score: 0.0,
        confidence: None,
    };
    assert_eq!(
        hit(&bare),
        json!({ "id": "x", "kind": "document", "text": "t", "score": 0.0,
                "meta": { "source": { "kind": "agent" } } })
    );
}

#[test]
fn pages_carry_a_cursor_only_when_there_is_more() {
    let more = FetchPage {
        hits: vec![sample_hit()],
        next_cursor: Some("1".into()),
    };
    assert_eq!(fetch(&more)["next_cursor"], json!("1"));
    let end = ListPage {
        items: vec![sample_hit()],
        next_cursor: None,
    };
    let rendered = list(&end);
    assert_eq!(rendered["items"].as_array().map(Vec::len), Some(1));
    assert!(rendered.get("next_cursor").is_none());
}

#[test]
fn recall_renders_answer_and_citations() {
    let source = sample_hit();
    let answer = RecallAnswer {
        answer: "it moves".into(),
        citations: vec![Citation {
            id: source.id,
            kind: source.kind,
            snippet: source.text,
            meta: source.meta,
            score: None,
        }],
        model: Some("m".into()),
    };
    let rendered = recall(&answer);
    assert_eq!(rendered["answer"], json!("it moves"));
    assert_eq!(rendered["citations"][0]["snippet"], json!("ownership moves"));
    assert!(rendered["citations"][0].get("score").is_none());
    assert!(rendered.get("model").is_none());
}

#[test]
fn writes_and_explore_render_their_fields() {
    let receipt = StoreReceipt {
        id: ItemId::new("i"),
        replayed: true,
    };
    assert_eq!(store(&receipt), json!({ "id": "i", "replayed": true }));
    assert_eq!(
        forget(&ForgetReport { forgotten: 2 }, &[ItemId::new("gone")]),
        json!({ "forgotten": 2, "skipped": ["gone"] })
    );
    assert_eq!(
        get(&[], &[ItemId::new("m")]),
        json!({ "items": [], "missing": ["m"] })
    );
    let page = ExplorePage {
        facet: Facet::Tag,
        buckets: vec![FacetBucket {
            value: "rust".into(),
            count: 3,
        }],
        total: 4,
        missing: 1,
        more_buckets: 0,
        truncated: false,
    };
    assert_eq!(
        explore(&page),
        json!({ "facet": "tag", "buckets": [{ "value": "rust", "count": 3 }],
                "total": 4, "missing": 1, "more_buckets": 0, "truncated": false })
    );
}
