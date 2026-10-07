//! Chunked documents end to end on both wires: written in pieces under the
//! event limit, read back whole by `get` and `list`, ranked by piece, and
//! forgotten whole; documents written as one event read as before.

use serde_json::{Value, json};
use tinymemory_api::{
    DocumentBody, FetchMode, FetchRequest, ForgetTarget, GetRequest, ItemId, ItemKind,
    LearningKind, ListRequest, MemoryMeta, MetaFilter, Namespace, Reach, StoreItem,
};

use super::*;
use crate::cortex::envelope::chunks::{MAX_EVENT_TEXT_BYTES, PAGE_BREAK};
use crate::cortex::testing::{Shared, both, direct_double, direct_engine};

const SCOPE: &str = "app:tinymemory/source:pdf/app:documents";

fn meta() -> MemoryMeta {
    MemoryMeta {
        namespace: Namespace::source("pdf"),
        file_path: Some("/docs/handbook.pdf".into()),
        ..MemoryMeta::default()
    }
}

/// A document of `pages` pages, each a titled section of about 20 KiB, so
/// the whole is well over one event's target.
fn handbook(pages: usize) -> StoreItem {
    let body: Vec<String> = (1..=pages)
        .map(|page| {
            let filler = format!("Policy text for page {page}. ").repeat(700);
            format!("# Chapter {page}\n\n{filler}\n")
        })
        .collect();
    StoreItem::Document {
        title: Some("Handbook".into()),
        body: DocumentBody::Text(body.join(&PAGE_BREAK.to_string())),
        mime: Some("application/pdf".into()),
        meta: meta(),
    }
}

fn body(item: &StoreItem) -> &str {
    match item {
        StoreItem::Document {
            body: DocumentBody::Text(text),
            ..
        } => text,
        _ => panic!("a document"),
    }
}

/// The events the double holds in `scope`.
fn events(state: &Shared, scope: &str) -> Vec<Value> {
    state
        .log
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|event| event["scope"] == scope)
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_long_document_is_written_in_pieces_under_the_limit_and_read_back_whole() {
    for (engine, state) in both().await {
        let item = handbook(40);
        let id = item.fingerprint();
        let receipt = engine.store(item.clone()).await.unwrap();
        assert_eq!(receipt.id.as_str(), id, "the item keeps its identity");

        let written = events(&state, SCOPE);
        assert!(written.len() > 2, "{} pieces", written.len());
        for event in &written {
            let text = event["content"]["text"].as_str().unwrap();
            assert!(text.len() <= MAX_EVENT_TEXT_BYTES, "{}", text.len());
            assert!(
                text.len() <= 300 * 1024,
                "packed near the target: {}",
                text.len()
            );
        }

        let got = engine
            .get(GetRequest {
                ids: vec![ItemId::new(id.clone())],
                reach: None,
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, item.render_text(), "get reassembles the body");

        let listed = engine
            .list(ListRequest::new(
                MetaFilter::kinds([ItemKind::Document]),
                10,
            ))
            .await
            .unwrap();
        assert_eq!(listed.items.len(), 1, "one item, not one per piece");
        assert_eq!(listed.items[0].text, item.render_text());
    }
}

#[tokio::test]
async fn a_hit_on_a_piece_is_that_piece_with_its_page_and_section() {
    let (endpoint, _state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let item = handbook(40);
    engine.store(item.clone()).await.unwrap();
    let mut request = FetchRequest::new("Policy text page", FetchMode::Hybrid, 3);
    request.filter.reach = Some(Reach::exact(Namespace::source("pdf")));
    let page = engine.fetch(request).await.unwrap();
    assert!(!page.hits.is_empty());
    let hit = &page.hits[0];
    assert_eq!(hit.id.as_str(), item.fingerprint(), "the item's own id");
    assert!(hit.text.len() < body(&item).len(), "a piece, not the whole");
    assert!(body(&item).contains(hit.text.split("\n\n").nth(1).unwrap_or_default()));
    assert_eq!(
        hit.meta.file_path.as_deref(),
        Some("handbook.pdf"),
        "the file's name, never its folders"
    );
    assert!(
        hit.meta.tags.iter().any(|tag| tag.starts_with("page:")),
        "{:?}",
        hit.meta.tags
    );
    assert!(
        hit.meta
            .tags
            .iter()
            .any(|tag| tag.starts_with("section:Chapter ")),
        "{:?}",
        hit.meta.tags
    );
}

#[tokio::test]
async fn forgetting_a_chunked_document_removes_every_piece() {
    for (engine, state) in both().await {
        let item = handbook(30);
        let receipt = engine.store(item).await.unwrap();
        assert!(events(&state, SCOPE).len() > 2);
        let report = engine
            .forget(ForgetTarget::Ids(vec![receipt.id.clone()]))
            .await
            .unwrap();
        assert_eq!(report.forgotten, 1, "one item");
        assert!(events(&state, SCOPE).is_empty(), "no piece left behind");
    }
}

#[tokio::test]
async fn a_store_that_lost_a_piece_writes_only_that_piece_again() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let item = handbook(30);
    engine.store(item.clone()).await.unwrap();
    let before = events(&state, SCOPE).len();
    state.log.lock().unwrap().lose_last(SCOPE);
    let got = engine
        .get(GetRequest {
            ids: vec![ItemId::new(item.fingerprint())],
            reach: None,
        })
        .await
        .unwrap();
    assert!(got.is_empty(), "never a truncated body: {got:?}");
    let listed = engine
        .list(ListRequest::new(
            MetaFilter::kinds([ItemKind::Document]),
            10,
        ))
        .await
        .unwrap();
    assert!(listed.items.is_empty(), "never a truncated body");
    let again = engine.store(item.clone()).await.unwrap();
    assert!(!again.replayed, "a piece was missing");
    assert_eq!(events(&state, SCOPE).len(), before, "exactly that piece");
    let replay = engine.store(item).await.unwrap();
    assert!(replay.replayed, "every piece present: a replay");
}

#[tokio::test]
async fn a_short_document_is_one_event_and_reads_the_same() {
    for (engine, state) in both().await {
        let item = StoreItem::Document {
            title: Some("Note".into()),
            body: DocumentBody::Text(format!("Page one.{PAGE_BREAK}Page two.")),
            mime: None,
            meta: meta(),
        };
        engine.store(item.clone()).await.unwrap();
        let written = events(&state, SCOPE);
        assert_eq!(written.len(), 1);
        assert_eq!(
            written[0]["content"]["text"],
            format!("Page one.{PAGE_BREAK}Page two."),
            "the body itself"
        );
        let envelope = crate::cortex::envelope::decode_event(&written[0])
            .unwrap()
            .envelope;
        assert!(envelope.chunk.is_none(), "no piece info: {envelope:?}");
        let listed = engine
            .list(ListRequest::new(MetaFilter::default(), 10))
            .await
            .unwrap();
        assert_eq!(listed.items[0].text, item.render_text());
        assert!(listed.items[0].meta.tags.is_empty());
    }
}

#[tokio::test]
async fn an_event_written_before_chunking_still_reads() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let id = "legacy-doc-id";
    let envelope = json!({
        "v": 2,
        "id": id,
        "kind": "document",
        "text": "Refunds take five days.",
        "meta": { "namespace": "source:pdf", "source": { "kind": "file" } },
        "title": "Refunds"
    });
    state.log.lock().unwrap().append(&json!({
        "scope": SCOPE,
        "modality": "document",
        "idempotency_key": "legacy",
        "content": { "kind": "message", "role": "user", "text": envelope.to_string() },
        "context": { "labels": [crate::cortex::envelope::labels::item(id)] },
    }));
    let got = engine
        .get(GetRequest {
            ids: vec![ItemId::new(id)],
            reach: None,
        })
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].text.contains("Refunds take five days."));
    let mut request = FetchRequest::new("refunds", FetchMode::Hybrid, 3);
    request.filter.reach = Some(Reach::exact(Namespace::source("pdf")));
    let page = engine.fetch(request).await.unwrap();
    assert_eq!(page.hits.len(), 1);
    assert!(page.hits[0].text.contains("Refunds take five days."));
    assert!(page.hits[0].meta.tags.is_empty());
}

#[tokio::test]
async fn an_event_over_the_limit_is_refused_before_anything_is_sent() {
    for (engine, state) in both().await {
        let huge = StoreItem::learning(
            "x".repeat(MAX_EVENT_TEXT_BYTES),
            LearningKind::Fact,
            0.9,
            MemoryMeta::default(),
        );
        let small = StoreItem::document("A small note.", MemoryMeta::default());
        let writes_before = state.count("POST");
        let error = engine.store_many(vec![small, huge]).await.unwrap_err();
        assert!(
            matches!(error, crate::cortex::Error::InvalidRequest(_)),
            "{error:?}"
        );
        assert!(error.to_string().contains("1 MiB"), "{error}");
        assert_eq!(state.count("POST"), writes_before, "nothing was written");
    }
}

#[test]
fn the_double_answers_a_reused_key_as_a_conflict_before_checking_size() {
    let mut log = crate::cortex::testing::CortexLog::default();
    let event = |text: String| {
        json!({
            "scope": SCOPE,
            "modality": "document",
            "idempotency_key": "k",
            "content": { "kind": "message", "role": "user", "text": text },
        })
    };
    assert_eq!(log.append(&event("small".into())).0, 202);
    let (status, body) = log.append(&event("x".repeat(2 * 1024 * 1024)));
    assert_eq!(status, 409, "{body}");
    let (status, body) = log.append(&json!({
        "scope": SCOPE,
        "idempotency_key": "other",
        "content": { "text": "x".repeat(2 * 1024 * 1024) },
    }));
    assert_eq!(status, 422, "{body}");
}

#[test]
fn a_piece_is_tagged_only_with_the_page_and_section_it_has() {
    use crate::cortex::envelope::{ChunkInfo, Envelope};
    let item = StoreItem::document("piece text", meta());
    let mut envelope = Envelope::for_item(&item, &item.fingerprint())
        .unwrap()
        .remove(0);
    let tags = |pages: Option<[u32; 2]>, section: Option<&str>| {
        let mut piece = envelope.clone();
        piece.chunk = Some(ChunkInfo {
            index: 1,
            count: 3,
            pages,
            section: section.map(str::to_string),
        });
        super::items::located_meta(&piece).tags
    };
    assert_eq!(
        tags(Some([3, 3]), Some("Billing")),
        ["page:3", "section:Billing"]
    );
    assert_eq!(tags(Some([3, 5]), None), ["page:3-5"]);
    assert_eq!(tags(None, Some("Billing")), ["section:Billing"]);
    assert!(tags(None, None).is_empty(), "no page, no section: no tag");
    envelope.chunk = None;
    assert!(super::items::located_meta(&envelope).tags.is_empty());
}

#[tokio::test]
async fn a_long_document_without_pages_or_headings_gets_no_extra_tags() {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint);
    let body = format!("{}\n\n", "Plain paragraph about refunds. ".repeat(40)).repeat(300);
    let item = StoreItem::Document {
        title: None,
        body: DocumentBody::Text(body.clone()),
        mime: None,
        meta: meta(),
    };
    engine.store(item.clone()).await.unwrap();
    assert!(events(&state, SCOPE).len() > 1, "written in pieces");
    let mut request = FetchRequest::new("refunds paragraph", FetchMode::Hybrid, 3);
    request.filter.reach = Some(Reach::exact(Namespace::source("pdf")));
    let page = engine.fetch(request).await.unwrap();
    assert!(!page.hits.is_empty());
    for hit in &page.hits {
        assert!(hit.meta.tags.is_empty(), "{:?}", hit.meta.tags);
        assert!(
            body.contains(&hit.text),
            "a hit is a piece of the body: {:?}",
            &hit.text[..hit.text.len().min(80)]
        );
    }
}
