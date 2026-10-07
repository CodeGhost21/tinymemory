//! Brain ingestion, search, forgetting and the belief builds it hands back.

use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    ConsolidateRequest, Consolidation, Error, ListRequest, MemoryMeta, Namespace, SourceKind,
    SourceRef,
};

use super::*;

fn brain() -> (Arc<ReferenceEngine>, Brain) {
    let engine = Arc::new(ReferenceEngine::new());
    (engine.clone(), Brain::new(engine, MemoryLayout::default()))
}

#[tokio::test]
async fn a_document_lands_at_its_source_node_without_an_agent() {
    let (engine, brain) = brain();
    let meta = MemoryMeta {
        namespace: Namespace::agent("sneaky"),
        agent_id: Some("sneaky".into()),
        file_path: Some("/docs/refunds.pdf".into()),
        ..MemoryMeta::default()
    };
    let ingested = brain
        .ingest(
            BrainDocument::new(BrainSource::Pdf, "Refunds take five days.")
                .titled("refunds.pdf")
                .with_meta(meta),
        )
        .await
        .unwrap();
    assert_eq!(
        ingested.job,
        Some(BackgroundJob::BuildBeliefs {
            request: ConsolidateRequest::new(Reach::subtree(Namespace::source("pdf")))
                .kinds([ItemKind::Document]),
        })
    );
    let listed = engine
        .list(ListRequest::new(Default::default(), 10))
        .await
        .unwrap();
    let meta = &listed.items[0].meta;
    assert_eq!(meta.namespace, Namespace::source("pdf"));
    assert_eq!(meta.agent_id, None);
    assert_eq!(meta.file_path.as_deref(), Some("/docs/refunds.pdf"));
    assert_eq!(meta.source.kind, SourceKind::File);
}

#[tokio::test]
async fn a_reader_s_source_is_kept() {
    let (engine, brain) = brain();
    let meta = MemoryMeta {
        source: SourceRef {
            kind: SourceKind::Folder,
            id: Some("handbook".into()),
        },
        ..MemoryMeta::default()
    };
    brain
        .ingest(BrainDocument::new(BrainSource::Markdown, "Be kind.").with_meta(meta))
        .await
        .unwrap();
    let listed = engine
        .list(ListRequest::new(Default::default(), 10))
        .await
        .unwrap();
    assert_eq!(listed.items[0].meta.source.kind, SourceKind::Folder);
}

#[tokio::test]
async fn ingest_many_batches_and_builds_once_per_source() {
    let (engine, brain) = brain();
    let documents: Vec<BrainDocument> = (0..MAX_STORE_MANY + 5)
        .map(|i| {
            let source = if i % 2 == 0 {
                BrainSource::Notion
            } else {
                BrainSource::Github
            };
            BrainDocument::new(source, format!("page {i}"))
        })
        .collect();
    let batch = brain.ingest_many(documents).await.unwrap();
    assert_eq!(batch.receipts.len(), MAX_STORE_MANY + 5);
    assert_eq!(engine.len(), MAX_STORE_MANY + 5);
    assert_eq!(batch.jobs.len(), 2);
    let again = brain
        .ingest_many(vec![BrainDocument::new(BrainSource::Notion, "page 0")])
        .await
        .unwrap();
    assert!(again.receipts[0].replayed);
}

#[tokio::test]
async fn a_blank_document_is_refused_and_nothing_is_stored() {
    let (engine, brain) = brain();
    let error = brain
        .ingest_many(vec![
            BrainDocument::new(BrainSource::Web, "fine"),
            BrainDocument::new(BrainSource::Web, "  "),
        ])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    assert!(engine.is_empty());
}

#[tokio::test]
async fn search_and_forget_stay_inside_one_source() {
    let (engine, brain) = brain();
    for (source, text) in [
        (BrainSource::Pdf, "refund policy pdf"),
        (BrainSource::Notion, "refund policy notion"),
    ] {
        brain
            .ingest(BrainDocument::new(source, text))
            .await
            .unwrap();
    }
    assert_eq!(brain.search("refund", None, 10).await.unwrap().len(), 2);
    let notion = brain
        .search("refund", Some(&BrainSource::Notion), 10)
        .await
        .unwrap();
    assert_eq!(notion.len(), 1);
    assert!(notion[0].text.contains("notion"));

    let report = brain.forget(&BrainSource::Pdf).await.unwrap();
    assert_eq!(report.forgotten, 1);
    assert_eq!(engine.len(), 1);
    assert!(
        brain
            .search("refund", Some(&BrainSource::Pdf), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn forgetting_a_source_forgets_its_collections() {
    let (engine, brain) = brain();
    brain
        .ingest(BrainDocument::new(BrainSource::Github, "repo-wide notes"))
        .await
        .unwrap();
    let repo = brain
        .layout()
        .brain_collection(&BrainSource::Github, "acme-api")
        .unwrap();
    assert_eq!(repo.to_string(), "source:github/project:acme-api");
    for (namespace, text) in [
        (repo.clone(), "issue in acme/api"),
        (Namespace::source("notion"), "a notion page"),
    ] {
        let meta = MemoryMeta {
            namespace,
            ..MemoryMeta::default()
        };
        engine.store(StoreItem::document(text, meta)).await.unwrap();
    }
    let hits = brain
        .search("issue", Some(&BrainSource::Github), 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "a source's search reads its collections");

    let report = brain.forget(&BrainSource::Github).await.unwrap();
    assert_eq!(report.forgotten, 2, "the source node and its collection");
    assert_eq!(engine.len(), 1, "another source is untouched");
}

#[tokio::test]
async fn an_engine_that_builds_on_its_own_gets_no_build_from_an_ingest() {
    let engine = Arc::new(ReferenceEngine::new().with_consolidation(Consolidation::Automatic));
    let brain = Brain::new(engine.clone(), MemoryLayout::default());
    let ingested = brain
        .ingest(BrainDocument::new(
            BrainSource::Pdf,
            "Refunds take five days.",
        ))
        .await
        .unwrap();
    assert_eq!(ingested.job, None);
    let batch = brain
        .ingest_many(vec![
            BrainDocument::new(BrainSource::Notion, "Deploys run on Fridays."),
            BrainDocument::new(BrainSource::Github, "CI runs on every push."),
        ])
        .await
        .unwrap();
    assert!(batch.jobs.is_empty(), "{:?}", batch.jobs);
    assert_eq!(engine.len(), 3, "the documents are still stored");

    let refresh = brain.build(&BrainSource::Pdf).unwrap();
    assert_eq!(
        refresh,
        BackgroundJob::BuildBeliefs {
            request: ConsolidateRequest::new(Reach::subtree(Namespace::source("pdf")))
                .kinds([ItemKind::Document]),
        }
    );
    let report = crate::background::BackgroundRunner::new(engine, MemoryLayout::default())
        .run(refresh)
        .await
        .unwrap();
    assert_eq!(report.outcome, crate::background::JobOutcome::Done);
}
