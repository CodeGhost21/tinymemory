//! Export on both wires: every kind handed back whole, exactly as stored
//! (what a listing's rendered text drops included), paged by cursor, and a
//! chunked document missing a piece named rather than dropped.

use chrono::{TimeZone, Utc};
use tinymemory_api::{
    DocumentBody, ItemId, ItemKind, LearningKind, ListRequest, MemoryMeta, MetaFilter, Namespace,
    Role, SourceKind, SourceRef, StoreItem, ToolCallRef, Turn,
};

use super::*;
use crate::cortex::envelope::chunks::PAGE_BREAK;
use crate::cortex::testing::{both, direct_double, direct_engine};

/// One item of each kind, with every field a rendered hit loses set.
fn items() -> Vec<StoreItem> {
    let at = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).single();
    let agent = MemoryMeta {
        namespace: Namespace::agent("assistant"),
        thread_id: Some("thread-1".into()),
        source: SourceRef {
            kind: SourceKind::Conversation,
            id: Some("thread-1".into()),
        },
        ..MemoryMeta::default()
    };
    vec![
        StoreItem::Document {
            title: Some("Plan".into()),
            body: DocumentBody::Text("The plan has three steps.".into()),
            mime: Some("text/markdown".into()),
            meta: MemoryMeta {
                namespace: Namespace::source("markdown"),
                ..MemoryMeta::default()
            },
        },
        StoreItem::Conversation {
            turns: vec![
                Turn {
                    role: Role::User,
                    text: "what is in the plan".into(),
                    at,
                    tool_calls: Vec::new(),
                },
                Turn {
                    role: Role::Assistant,
                    text: "three steps".into(),
                    at,
                    tool_calls: vec![ToolCallRef {
                        name: "read_plan".into(),
                        id: Some("call-1".into()),
                    }],
                },
            ],
            meta: agent,
        },
        StoreItem::Learning {
            text: "The user prefers short answers.".into(),
            kind: LearningKind::Preference,
            confidence: 0.75,
            evidence: Some("said so twice".into()),
            meta: MemoryMeta {
                tags: vec!["flow:nl".into()],
                ..MemoryMeta::default()
            },
        },
    ]
}

/// Every item an export of `filter` hands back, following the cursor
/// `limit` at a time, with the incomplete ids.
async fn export_all(
    engine: &dyn MemoryEngine,
    filter: MetaFilter,
    limit: usize,
) -> (Vec<tinymemory_api::Exported>, Vec<ItemId>) {
    let (mut items, mut incomplete, mut cursor) = (Vec::new(), Vec::new(), None);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..100 {
        let mut request = ListRequest::new(filter.clone(), limit);
        request.cursor = cursor;
        let page = engine.export(request).await.unwrap();
        items.extend(page.items);
        incomplete.extend(page.incomplete);
        match page.next_cursor {
            Some(next) => {
                assert!(seen.insert(next.clone()), "the cursor {next} repeated");
                cursor = Some(next);
            }
            None => return (items, incomplete),
        }
    }
    panic!("the export did not end within 100 pages");
}

#[tokio::test]
async fn every_kind_is_exported_whole_exactly_as_stored() {
    for (engine, _) in both().await {
        let stored = items();
        for item in &stored {
            engine.store(item.clone()).await.unwrap();
        }
        let (exported, incomplete) = export_all(&engine, MetaFilter::default(), 10).await;
        assert!(incomplete.is_empty(), "{incomplete:?}");
        assert_eq!(exported.len(), stored.len(), "{exported:?}");
        for item in &stored {
            let found = exported
                .iter()
                .find(|e| e.id.as_str() == item.fingerprint())
                .unwrap_or_else(|| panic!("{:?} not exported", item.kind()));
            assert_eq!(&found.item, item, "exported exactly as stored");
        }
        let again = engine.store(exported[0].item.clone()).await.unwrap();
        assert!(again.replayed, "storing it where it was is a replay");
    }
}

#[tokio::test]
async fn an_export_pages_by_cursor_and_hands_each_item_back_once() {
    for (engine, _) in both().await {
        let mut want = Vec::new();
        for n in 0..5 {
            let item = StoreItem::learning(
                format!("fact {n}"),
                LearningKind::Fact,
                0.5,
                MemoryMeta::default(),
            );
            want.push(item.fingerprint());
            engine.store(item).await.unwrap();
        }
        let (exported, _) = export_all(&engine, MetaFilter::kinds([ItemKind::Learning]), 2).await;
        assert_eq!(exported.len(), 5, "each item once: {exported:?}");
        let mut ids: Vec<String> = exported.iter().map(|e| e.id.as_str().to_string()).collect();
        ids.sort_unstable();
        want.sort_unstable();
        assert_eq!(ids, want, "exactly the stored items");
    }
}

#[tokio::test]
async fn a_chunked_document_missing_a_piece_is_named_not_dropped() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let pages: Vec<String> = (1..=30)
        .map(|page| format!("# Chapter {page}\n\n{}", "Policy text. ".repeat(1600)))
        .collect();
    let item = StoreItem::Document {
        title: Some("Handbook".into()),
        body: DocumentBody::Text(pages.join(&PAGE_BREAK.to_string())),
        mime: Some("application/pdf".into()),
        meta: MemoryMeta {
            namespace: Namespace::source("pdf"),
            ..MemoryMeta::default()
        },
    };
    engine.store(item.clone()).await.unwrap();
    state
        .log
        .lock()
        .unwrap()
        .lose_last("app:tinymemory/source:pdf/app:documents");
    let (exported, incomplete) = export_all(&engine, MetaFilter::default(), 10).await;
    assert!(exported.is_empty(), "never a truncated body: {exported:?}");
    assert_eq!(incomplete, vec![ItemId::new(item.fingerprint())]);
}

#[tokio::test]
async fn an_export_below_the_clamp_reads_every_scope() {
    for (engine, state) in both().await {
        engine.store(items().remove(2)).await.unwrap();
        state
            .padding_scopes
            .store(998, std::sync::atomic::Ordering::SeqCst);
        let (exported, _) = export_all(&engine, MetaFilter::default(), 10).await;
        assert_eq!(exported.len(), 1, "{exported:?}");
    }
}
