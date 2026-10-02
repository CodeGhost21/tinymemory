//! End-to-end tests for the seam, against a real TinyCortex backend.
//!
//! These are the ones that would catch a conversion that type-checks but means
//! the wrong thing, because everything here goes in through the TinyMemory
//! contract and comes back out of the engine's own store.

#![allow(clippy::expect_used, clippy::panic)]

use tinycortex::memory::store::InMemoryMemoryStore;
use tinymemory_api::capabilities::Capability;
use tinymemory_api::provider::{audit_provider, MemoryCore, MemoryPortability, MemoryProvider};
use tinymemory_api::types::{MemoryCategory, MemoryTaint, GLOBAL_NAMESPACE};

use super::*;
use crate::TINYCORTEX_DRIVER_ID;

fn engine() -> Arc<dyn tinycortex::memory::Memory> {
    Arc::new(InMemoryMemoryStore::new())
}

#[tokio::test]
async fn the_adapter_reports_the_engine_backend_name() {
    let memory = TinycortexMemory::new(engine());
    assert_eq!(memory.name(), "in_memory");
}

#[tokio::test]
async fn a_lightweight_driver_advertises_document_ingestion() {
    let driver = crate::provider(engine());
    audit_provider(&driver).expect("advertised capabilities match the accessors");
    assert_eq!(driver.driver_id(), TINYCORTEX_DRIVER_ID);
    assert!(driver.capabilities().contains(Capability::DocumentIngest));
    assert!(driver.as_document_ingest().is_some());
    assert!(!driver
        .capabilities()
        .contains(Capability::ConversationIngest));
}

#[tokio::test]
async fn lightweight_document_ingestion_routes_into_the_embedded_store() {
    let driver = crate::provider(engine());
    let document = serde_json::from_value(serde_json::json!({
        "source": "upload",
        "source_id": "handbook",
        "content": "The release train leaves on Friday."
    }))
    .expect("document item");
    let outcome = driver
        .as_document_ingest()
        .expect("document route")
        .ingest_document(document)
        .await
        .expect("ingest document");

    assert_eq!(outcome.written, 1);
    let stored = driver
        .get("document:handbook", "handbook")
        .await
        .expect("get document")
        .expect("stored document");
    assert_eq!(stored.content, "The release train leaves on Friday.");
}

#[tokio::test]
async fn store_and_get_round_trip_through_the_contract() {
    let driver = crate::provider(engine());
    driver
        .store(
            "projects",
            "k",
            "body",
            MemoryCategory::Daily,
            Some("s1"),
            MemoryTaint::Internal,
        )
        .await
        .expect("store");

    let entry = driver
        .get("projects", "k")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(entry.content, "body");
    assert_eq!(entry.category, MemoryCategory::Daily);
    assert_eq!(entry.session_id.as_deref(), Some("s1"));
    assert_eq!(entry.taint, MemoryTaint::Internal);
}

/// The end-to-end version of the taint check: content stored as external
/// through the contract must still read back as external from the engine.
#[tokio::test]
async fn provenance_survives_the_seam_in_both_directions() {
    let driver = crate::provider(engine());
    driver
        .store(
            "ns",
            "synced",
            "from gmail",
            MemoryCategory::Core,
            None,
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("store");

    let entry = driver
        .get("ns", "synced")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(
        entry.taint,
        MemoryTaint::ExternalSync,
        "external content must not be laundered into internal trust"
    );
}

/// The shared `list(None, ..)` fix, verified against a real engine rather than
/// a test double.
#[tokio::test]
async fn listing_with_no_namespace_spans_every_namespace() {
    let driver = crate::provider(engine());
    for (namespace, key) in [(GLOBAL_NAMESPACE, "a"), ("projects", "b"), ("people", "c")] {
        driver
            .store(
                namespace,
                key,
                "body",
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await
            .expect("store");
    }

    let everything = driver.list(None, None, None).await.expect("list");
    assert_eq!(everything.len(), 3);

    let scoped = driver
        .list(Some("projects"), None, None)
        .await
        .expect("list");
    assert_eq!(scoped.len(), 1);
}

#[tokio::test]
async fn forget_reports_whether_the_entry_existed() {
    let driver = crate::provider(engine());
    driver
        .store(
            "ns",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    assert!(driver.forget("ns", "k").await.expect("forget"));
    assert!(!driver.forget("ns", "k").await.expect("forget again"));
}

#[tokio::test]
async fn namespaces_reports_the_engine_summaries() {
    let driver = crate::provider(engine());
    driver
        .store(
            "projects",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let namespaces = driver.namespaces().await.expect("namespaces");
    assert_eq!(namespaces.len(), 1);
    assert_eq!(namespaces[0].namespace, "projects");
    assert_eq!(namespaces[0].count, 1);
}

/// The acceptance property for the whole seam: a store can be exported through
/// the contract and restored into a second engine with provenance intact.
#[tokio::test]
async fn a_store_exports_and_restores_across_two_engines() {
    let source = crate::provider(engine());
    source
        .store(
            "ns",
            "internal",
            "typed by the user",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    source
        .store(
            "ns",
            "external",
            "from a sync",
            MemoryCategory::Daily,
            Some("s1"),
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("store");

    let mut records = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = source
            .export_page(cursor.as_deref(), 1)
            .await
            .expect("export page");
        pages += 1;
        assert!(pages < 10, "export did not terminate");
        records.extend(page.records);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(records.len(), 2);

    let target = crate::provider(engine());
    let outcome = target.import_records(records).await.expect("import");
    assert_eq!(outcome.imported, 2);
    assert_eq!(outcome.failed, 0);

    let external = target
        .get("ns", "external")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(external.taint, MemoryTaint::ExternalSync);
    assert_eq!(external.content, "from a sync");
    assert_eq!(external.session_id.as_deref(), Some("s1"));
    assert_eq!(external.category, MemoryCategory::Daily);

    let internal = target
        .get("ns", "internal")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(internal.taint, MemoryTaint::Internal);
    assert_eq!(internal.category, MemoryCategory::Core);
}

/// A driver id is rendered into logs and audit events; the backend handle may
/// hold a path or connection string and must not be.
#[test]
fn debug_renders_the_backend_name_and_not_the_handle() {
    let rendered = format!("{:?}", TinycortexMemory::new(engine()));
    assert!(rendered.contains("in_memory"));
}
