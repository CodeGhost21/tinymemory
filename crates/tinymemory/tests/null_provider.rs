//! The `null` driver: the configuration a compiled-out or unconfigured memory
//! subsystem binds to.
//!
//! It has to be genuinely usable, not a placeholder that panics. A host whose
//! memory is switched off still calls the ports, and the difference between
//! "returns empty" and "aborts the process" is the difference between a
//! degraded deployment and an outage.

// A failing assertion in a test *is* a panic; the crate-wide `expect_used` /
// `unwrap_used` / `panic` lints exist to keep the library from panicking, not
// the tests. Same allowance, and same reasoning, as `src/registry/test.rs`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use tinymemory::api::capabilities::{Capabilities, Capability};
use tinymemory::api::null::{NullMemoryProvider, NULL_DRIVER_ID};
use tinymemory::api::provider::{audit_provider, MemoryProvider};
use tinymemory::types::{MemoryCategory, MemoryTaint};

const NS: &str = "null-provider";

#[test]
fn it_identifies_itself_and_passes_its_own_audit() {
    let provider = NullMemoryProvider::new();
    assert_eq!(provider.driver_id(), NULL_DRIVER_ID);
    assert!(audit_provider(&provider).is_ok());
    assert_eq!(provider.capabilities(), Capabilities::mandatory());
}

#[tokio::test]
async fn every_mandatory_method_answers_rather_than_panicking() {
    let provider: Arc<dyn MemoryProvider> = Arc::new(NullMemoryProvider::new());

    provider
        .store(
            NS,
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store is accepted and discarded, not refused");
    assert!(provider.get(NS, "k").await.expect("get answers").is_none());
    assert!(!provider.forget(NS, "k").await.expect("forget answers"));
    assert!(provider
        .list(None, None, None)
        .await
        .expect("list answers")
        .is_empty());
    assert!(provider
        .namespaces()
        .await
        .expect("namespaces answers")
        .is_empty());

    let opts = tinymemory::recall::OwnedRecallOpts::default();
    assert!(provider
        .recall("anything", 10, &opts, None)
        .await
        .expect("recall answers")
        .is_empty());

    let page = provider
        .export_page(None, 10)
        .await
        .expect("export answers");
    assert_eq!(page.records.len(), 0);
    assert!(page.next_cursor.is_none(), "an empty export must terminate");

    let outcome = provider
        .import_records(Vec::new())
        .await
        .expect("import answers");
    assert_eq!(outcome.imported, 0);
    assert_eq!(outcome.failed, 0);
}

#[tokio::test]
async fn it_is_healthy_rather_than_reporting_a_fault() {
    // "Memory is switched off" is a configuration, not a failure. Reporting
    // unhealthy would make an intentional deployment look like a broken one.
    let provider = NullMemoryProvider::new();
    assert_eq!(
        provider.health().await,
        tinymemory::health::MemoryHealth::Ready
    );
}

#[test]
fn no_optional_family_is_reachable_and_none_is_advertised() {
    let provider = NullMemoryProvider::new();
    for capability in Capability::ALL {
        if Capability::MANDATORY.contains(&capability) {
            continue;
        }
        assert!(
            !provider.provides(capability),
            "`{}` must not be reachable on the null driver",
            capability.as_str()
        );
        assert!(
            !provider.capabilities().contains(capability),
            "`{}` must not be advertised on the null driver",
            capability.as_str()
        );
    }
}

#[tokio::test]
async fn it_conforms_to_the_behavioural_suite() {
    // The contract-shape half of the suite applies to a discard driver exactly
    // as it does to a retaining one; the suite skips only the storage half.
    tinymemory_conformance::assert_provider(Arc::new(NullMemoryProvider::new())).await;
}
