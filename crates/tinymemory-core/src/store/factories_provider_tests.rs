//! `tinymemory-core`'s own store, held to the driver contract (#18 §A3/§E1).
//!
//! The TinyCortex adapter and the three hosted adapters each have a
//! `conformance_test.rs` asserting they uphold `MemoryProvider`. This crate's
//! own store had no such file, because until `create_memory_provider` there was
//! no way to express it as a driver at all — which is precisely the gap §A3
//! describes. These are the missing equivalents.

// `expect` is the assertion mechanism here, and its message is the failure
// diagnostic — `expect_used` is `warn` workspace-wide (Cargo.toml) and CI runs
// clippy with `-D warnings`. Scoped to that one lint: unlike the sibling
// conformance tests this file has no explicit `panic!`, so it does not need
// `clippy::panic` too.
#![allow(clippy::expect_used)]

use std::sync::Arc;

use tinymemory_api::host::MemoryConfig;
use tinymemory_api::provider::{audit_provider, MemoryProvider};

use super::factories::create_memory_provider;

/// Builds a provider over a real store in a throwaway directory.
///
/// A temp dir rather than an in-memory backend on purpose: `UnifiedMemory` is a
/// SQLite store, and a driver that only ever answered from memory would not be
/// the thing hosts actually bind.
fn provider(dir: &std::path::Path) -> Arc<dyn MemoryProvider> {
    // The store resolves its embedder through the process-global `EmbeddingHost`
    // and refuses to open without one. `init` is the crate's idempotent stub
    // installer, so this is the same seam every other core test uses rather
    // than a second setup path.
    crate::test_seams::init();
    create_memory_provider(&MemoryConfig::default(), dir).expect("the bundled store opens")
}

#[tokio::test]
async fn the_core_store_upholds_the_contract() {
    let dir = tempfile::tempdir().expect("temp dir");
    tinymemory_conformance::assert_provider(provider(dir.path())).await;
}

#[tokio::test]
async fn the_core_store_actually_retains() {
    // The conformance suite tolerates a driver that refuses a write; without
    // this probe a store that silently retained nothing could pass it
    // vacuously. That is not hypothetical — it is how a broken double slipped
    // through review once already.
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(
        tinymemory_conformance::retains_writes(provider(dir.path()).as_ref()).await,
        "the bundled store reported success and kept nothing"
    );
}

#[tokio::test]
async fn it_binds_under_the_reserved_namespace_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert_eq!(
        provider(dir.path()).driver_id(),
        tinymemory_api::drivers::NAMESPACE_DRIVER_ID,
        "the bundled store must not bind under another engine's id"
    );
}

#[tokio::test]
async fn its_advertised_capabilities_match_what_it_exposes() {
    // `audit_provider` is the honesty check: advertised families must equal
    // reachable accessors. Wrapping through `MemoryTraitProvider` derives the
    // advertisement from the accessors, so this should hold by construction —
    // it runs because that construction lives in another crate.
    let dir = tempfile::tempdir().expect("temp dir");
    audit_provider(provider(dir.path()).as_ref()).expect("the bundled store is honest");
}

#[tokio::test]
async fn the_registry_admits_it_as_an_embedded_driver() {
    use tinymemory::registry::{DriverClass, DriverRegistry, NAMESPACE_DRIVER_ID};

    // A reserved id with no admission path would be a driver nothing can bind.
    let admitted = DriverRegistry::builtin()
        .admit(NAMESPACE_DRIVER_ID, None, Default::default())
        .expect("the bundled store is admissible");
    assert_eq!(admitted.class, DriverClass::Embedded);
}
