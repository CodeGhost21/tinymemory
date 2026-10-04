//! Compiling against the reference engine and against failing engines.

use async_trait::async_trait;
use chrono::TimeZone;
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, Error as ApiError, FetchPage, FetchRequest, ForgetReport,
    ForgetTarget, LearningKind, ListPage, MemoryMeta, RecallAnswer, StoreItem, StoreReceipt,
};

use super::*;
use crate::context::spec::Brief;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap()
}

fn learning(text: &str, confidence: f32, day: Option<u32>) -> StoreItem {
    let meta = MemoryMeta {
        observed_at: day.map(|d| Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()),
        ..MemoryMeta::default()
    };
    StoreItem::learning(text, LearningKind::Preference, confidence, meta)
}

#[tokio::test]
async fn an_empty_engine_yields_an_empty_document() {
    let engine = ReferenceEngine::new();
    let doc = ContextCompiler::at(at())
        .compile(&engine, &ContextSpec::default())
        .await
        .unwrap();
    assert_eq!(doc.markdown, "");
    assert_eq!(doc.tokens, 0);
    assert!(doc.refs.is_empty());
    assert_eq!(doc.engine, "reference");
    assert_eq!(doc.generated_at, at());
}

#[tokio::test]
async fn briefs_and_learnings_fill_the_document_in_order() {
    let engine = ReferenceEngine::new();
    engine
        .store(StoreItem::document(
            "The user is Steven, a builder of memory systems.",
            MemoryMeta::default(),
        ))
        .await
        .unwrap();
    engine
        .store(learning("prefers terse answers", 0.9, Some(3)))
        .await
        .unwrap();
    engine
        .store(learning("likes Rust", 0.4, Some(5)))
        .await
        .unwrap();
    engine
        .store(learning("uses vim", 0.8, Some(5)))
        .await
        .unwrap();
    engine
        .store(learning("undated habit", 1.0, None))
        .await
        .unwrap();
    let spec = ContextSpec {
        briefs: vec![Brief::new("About the user", "who is the user")],
        ..ContextSpec::default()
    };
    let doc = ContextCompiler::at(at())
        .compile(&engine, &spec)
        .await
        .unwrap();
    let md = &doc.markdown;
    assert!(md.starts_with("---\n"));
    assert!(md.contains("## About the user"));
    let order: Vec<usize> = [
        "uses vim",
        "likes Rust",
        "prefers terse answers",
        "undated habit",
    ]
    .iter()
    .map(|text| md.find(&format!("- {text}")).unwrap())
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{md}");
    assert!(!doc.refs.is_empty());
    assert_eq!(doc.tokens, crate::context::estimate_tokens(md));
}

#[tokio::test]
async fn the_learnings_limit_caps_the_list() {
    let engine = ReferenceEngine::new();
    for i in 0..5 {
        engine
            .store(learning(&format!("habit {i}"), 0.5, Some(i + 1)))
            .await
            .unwrap();
    }
    let spec = ContextSpec {
        briefs: Vec::new(),
        learnings_limit: 2,
        ..ContextSpec::default()
    };
    let doc = ContextCompiler::at(at())
        .compile(&engine, &spec)
        .await
        .unwrap();
    assert_eq!(doc.markdown.matches("\n- habit").count(), 2);
    assert!(doc.markdown.contains("- habit 4"));
    assert!(doc.markdown.contains("- habit 3"));
    assert_eq!(doc.refs.len(), 2);
}

/// Fails every recall and every listing.
struct Broken(EngineDescriptor);

#[async_trait]
impl MemoryEngine for Broken {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.0
    }
    async fn health(&self) -> EngineHealth {
        EngineHealth::Down("broken".into())
    }
    async fn recall(&self, _req: RecallRequest) -> tinymemory_api::Result<RecallAnswer> {
        Err(ApiError::Unavailable("down".into()))
    }
    async fn fetch(&self, _req: FetchRequest) -> tinymemory_api::Result<FetchPage> {
        Err(ApiError::Unavailable("down".into()))
    }
    async fn store(&self, _item: StoreItem) -> tinymemory_api::Result<StoreReceipt> {
        Err(ApiError::Unavailable("down".into()))
    }
    async fn forget(&self, _target: ForgetTarget) -> tinymemory_api::Result<ForgetReport> {
        Err(ApiError::Unavailable("down".into()))
    }
    async fn list(&self, _req: ListRequest) -> tinymemory_api::Result<ListPage> {
        Err(ApiError::Unavailable("down".into()))
    }
}

#[tokio::test]
async fn failing_briefs_and_learnings_are_skipped_not_fatal() {
    let engine = Broken(ReferenceEngine::new().descriptor().clone());
    let doc = compile(&engine, &ContextSpec::default()).await.unwrap();
    assert_eq!(doc.markdown, "");
}

#[tokio::test]
async fn an_invalid_spec_is_refused() {
    let engine = ReferenceEngine::new();
    let spec = ContextSpec {
        budget_tokens: 0,
        ..ContextSpec::default()
    };
    assert!(matches!(
        compile(&engine, &spec).await,
        Err(crate::context::Error::InvalidSpec(_))
    ));
}

#[tokio::test]
async fn a_reach_keeps_the_document_to_one_agent_s_memory() {
    let engine = ReferenceEngine::new();
    let mut own = learning("researcher habit", 0.9, Some(1));
    own.meta_mut().namespace = tinymemory_api::Namespace::agent("researcher");
    let mut sibling = learning("writer habit", 0.9, Some(1));
    sibling.meta_mut().namespace = tinymemory_api::Namespace::agent("writer");
    for item in [own, sibling, learning("shared habit", 0.9, Some(1))] {
        engine.store(item).await.unwrap();
    }
    let spec = ContextSpec {
        briefs: vec![Brief::new("About the user", "habit")],
        reach: Some(tinymemory_api::Reach::of(tinymemory_api::Namespace::agent(
            "researcher",
        ))),
        ..ContextSpec::default()
    };
    let doc = ContextCompiler::at(at())
        .compile(&engine, &spec)
        .await
        .unwrap();
    assert!(
        doc.markdown.contains("- researcher habit"),
        "{}",
        doc.markdown
    );
    assert!(
        doc.markdown.contains("- shared habit"),
        "the root is inherited"
    );
    assert!(!doc.markdown.contains("writer habit"), "{}", doc.markdown);
}
