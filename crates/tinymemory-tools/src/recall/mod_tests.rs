//! Holistic recall against the reference engine and a half-broken one.

use async_trait::async_trait;
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, Error, FetchPage, FetchRequest, ForgetReport, ForgetTarget,
    ItemKind, LearningKind, ListPage, ListRequest, MemoryMeta, MetaFilter, Namespace, Reach,
    RecallAnswer, RecallRequest, Role, StoreItem, StoreReceipt, Turn, TurnRange,
};

use super::*;

fn turn(thread: &str, index: u32, text: &str) -> StoreItem {
    StoreItem::Conversation {
        turns: vec![Turn::new(Role::User, text)],
        meta: MemoryMeta {
            thread_id: Some(thread.to_string()),
            turns: Some(TurnRange {
                first: index,
                last: index,
            }),
            ..MemoryMeta::default()
        },
    }
}

async fn seeded() -> ReferenceEngine {
    let engine = ReferenceEngine::new();
    for item in [
        StoreItem::document("Refunds take five business days.", MemoryMeta::default()),
        StoreItem::document("Deploys happen on Fridays.", MemoryMeta::default()),
        StoreItem::learning(
            "Customers prefer refunds by email",
            LearningKind::Fact,
            0.9,
            MemoryMeta::default(),
        ),
        turn("t1", 0, "asked about refunds yesterday"),
        turn("t2", 4, "refunds question in this very thread"),
    ] {
        engine.store(item).await.unwrap();
    }
    engine
}

fn docs() -> MetaFilter {
    MetaFilter::kinds([ItemKind::Document])
}

#[tokio::test]
async fn fetch_sections_rank_for_the_pack_query() {
    let engine = seeded().await;
    let request = HolisticRecall::new(
        Some("refunds".into()),
        vec![ScopeSection::fetch("Docs", docs(), 1)],
    );
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(
        pack.markdown,
        "# Memory\n\n## Docs\n\n- Refunds take five business days.\n"
    );
    assert_eq!(pack.sections.len(), 1);
    assert_eq!(pack.sections[0].hits.len(), 1);
    assert_eq!(pack.refs.len(), 1);
    assert_eq!(pack.engine, "reference");
    assert!(pack.skipped.is_empty());
}

#[tokio::test]
async fn a_fetch_section_without_any_query_reads_the_latest() {
    let engine = seeded().await;
    let request = HolisticRecall::new(None, vec![ScopeSection::fetch("Docs", docs(), 5)]);
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(pack.sections[0].hits.len(), 2);
}

#[tokio::test]
async fn a_section_s_own_query_overrides_the_pack_s() {
    let engine = seeded().await;
    let mut section = ScopeSection::fetch("Docs", docs(), 1);
    section.query = SectionQuery::Fetch {
        query: Some("deploys fridays".into()),
    };
    let pack = holistic_recall(
        &engine,
        &HolisticRecall::new(Some("refunds".into()), vec![section]),
    )
    .await
    .unwrap();
    assert!(pack.markdown.contains("Deploys happen on Fridays."));
}

#[tokio::test]
async fn answered_sections_are_prose_with_citations() {
    let engine = seeded().await;
    let request = HolisticRecall::new(
        None,
        vec![ScopeSection::answer(
            "Refunds",
            "how long do refunds take",
            docs(),
            3,
        )],
    );
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert!(
        pack.markdown.contains("## Refunds\n\nFrom "),
        "{}",
        pack.markdown
    );
    assert!(pack.sections[0].answer.is_some());
    assert!(!pack.refs.is_empty());
}

#[tokio::test]
async fn excluded_ids_and_the_live_thread_window_are_left_out() {
    let engine = seeded().await;
    let conversations = MetaFilter::kinds([ItemKind::Conversation]);
    let mut request = HolisticRecall::new(
        Some("refunds".into()),
        vec![ScopeSection::fetch("History", conversations, 5)],
    );
    request.exclude_thread = Some(ThreadWindow {
        thread_id: "t2".into(),
        from_turn: 3,
    });
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert!(pack.markdown.contains("asked about refunds yesterday"));
    assert!(!pack.markdown.contains("this very thread"));

    request.exclude_thread = Some(ThreadWindow {
        thread_id: "t2".into(),
        from_turn: 5,
    });
    let older = holistic_recall(&engine, &request).await.unwrap();
    assert!(
        older.markdown.contains("this very thread"),
        "a turn before the window is no longer in the prompt"
    );

    request.exclude_thread = None;
    request.exclude_ids = older.refs.clone();
    let none = holistic_recall(&engine, &request).await.unwrap();
    assert!(none.is_empty());
    assert_eq!(none.skipped[0].reason, "empty");
}

/// The reference engine with recall (and optionally fetch) broken.
struct Broken {
    inner: ReferenceEngine,
    fetch_too: bool,
}

#[async_trait]
impl tinymemory_api::MemoryEngine for Broken {
    fn descriptor(&self) -> &EngineDescriptor {
        self.inner.descriptor()
    }
    async fn health(&self) -> EngineHealth {
        EngineHealth::Ok
    }
    async fn recall(&self, _req: RecallRequest) -> tinymemory_api::Result<RecallAnswer> {
        Err(Error::Unavailable("recall is down".into()))
    }
    async fn fetch(&self, req: FetchRequest) -> tinymemory_api::Result<FetchPage> {
        if self.fetch_too {
            return Err(Error::Unavailable("fetch is down".into()));
        }
        self.inner.fetch(req).await
    }
    async fn store(&self, item: StoreItem) -> tinymemory_api::Result<StoreReceipt> {
        self.inner.store(item).await
    }
    async fn forget(&self, target: ForgetTarget) -> tinymemory_api::Result<ForgetReport> {
        self.inner.forget(target).await
    }
    async fn list(&self, req: ListRequest) -> tinymemory_api::Result<ListPage> {
        self.inner.list(req).await
    }
}

#[tokio::test]
async fn a_failed_answer_falls_back_to_fetch_only_when_asked() {
    let engine = Broken {
        inner: seeded().await,
        fetch_too: false,
    };
    let mut section = ScopeSection::answer("Refunds", "refunds", docs(), 3);
    let plain = holistic_recall(&engine, &HolisticRecall::new(None, vec![section.clone()]))
        .await
        .unwrap();
    assert!(plain.is_empty());
    assert!(plain.skipped[0].reason.contains("recall is down"));

    if let SectionQuery::Answer {
        fallback_to_fetch, ..
    } = &mut section.query
    {
        *fallback_to_fetch = true;
    }
    let fallen = holistic_recall(&engine, &HolisticRecall::new(None, vec![section]))
        .await
        .unwrap();
    assert!(
        fallen
            .markdown
            .contains("- Refunds take five business days.")
    );
}

#[tokio::test]
async fn a_failing_section_never_fails_the_pack() {
    let engine = Broken {
        inner: seeded().await,
        fetch_too: true,
    };
    let request = HolisticRecall::new(
        Some("refunds".into()),
        vec![
            ScopeSection::fetch("Docs", docs(), 3),
            ScopeSection::latest("Learnings", MetaFilter::kinds([ItemKind::Learning]), 3),
        ],
    );
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(pack.skipped.len(), 1);
    assert_eq!(pack.skipped[0].heading, "Docs");
    assert!(pack.markdown.contains("## Learnings"));
}

#[tokio::test]
async fn sections_read_only_their_scope() {
    let engine = ReferenceEngine::new();
    for (text, namespace) in [
        ("pdf fact about refunds", Namespace::source("pdf")),
        ("agent note about refunds", Namespace::agent("a")),
    ] {
        engine
            .store(StoreItem::document(
                text,
                MemoryMeta {
                    namespace,
                    ..MemoryMeta::default()
                },
            ))
            .await
            .unwrap();
    }
    let pdf_only = MetaFilter {
        reach: Some(Reach::subtree(Namespace::source("pdf"))),
        ..docs()
    };
    let pack = holistic_recall(
        &engine,
        &HolisticRecall::new(
            Some("refunds".into()),
            vec![ScopeSection::fetch("Pdf", pdf_only, 5)],
        ),
    )
    .await
    .unwrap();
    assert!(pack.markdown.contains("pdf fact"));
    assert!(!pack.markdown.contains("agent note"));
}

#[tokio::test]
async fn an_invalid_request_is_refused() {
    let engine = ReferenceEngine::new();
    let cases = [
        HolisticRecall {
            budget_tokens: 0,
            ..HolisticRecall::new(None, Vec::new())
        },
        HolisticRecall {
            title: " ".into(),
            ..HolisticRecall::new(None, Vec::new())
        },
        HolisticRecall::new(None, vec![ScopeSection::fetch(" ", docs(), 1)]),
        HolisticRecall::new(None, vec![ScopeSection::fetch("Docs", docs(), 0)]),
        HolisticRecall::new(None, vec![ScopeSection::answer("Docs", " ", docs(), 1)]),
    ];
    for request in cases {
        let error = holistic_recall(&engine, &request).await.unwrap_err();
        assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    }
}

#[tokio::test]
async fn an_item_is_listed_once_in_its_first_section() {
    let engine = seeded().await;
    let request = HolisticRecall::new(
        Some("refunds".into()),
        vec![
            ScopeSection::fetch("Docs", docs(), 1),
            ScopeSection::fetch("Everything", MetaFilter::default(), 10),
        ],
    );
    let pack = holistic_recall(&engine, &request).await.unwrap();
    assert_eq!(
        pack.markdown
            .matches("Refunds take five business days.")
            .count(),
        1,
        "{}",
        pack.markdown
    );
    assert!(
        pack.sections[1]
            .hits
            .iter()
            .all(|hit| hit.id != pack.sections[0].hits[0].id)
    );
}
