//! The conformance suite, run against the TinyCortex driver.
//!
//! The last name in issue #18's acceptance criterion 5, alongside the three
//! hosted adapters covered in `tinymemory-remote`.
//!
//! # Which TinyCortex driver
//!
//! This crate binds two, and they are conformance-tested differently.
//!
//! [`crate::provider`] composes the three mandatory families over any
//! `tinycortex::memory::Memory` backend. It needs nothing but the backend, so
//! the suite runs against it here with the engine's own `InMemoryMemoryStore`.
//!
//! [`crate::engine::TinycortexProvider`] serves every compiled family, and
//! needs a `MemoryClient` — which needs the host's process-global seams
//! (`set_embedding_host` and friends) installed before it will open. A test
//! that installs a process global is order-dependent, which `AGENTS.md` rules
//! out, so it is covered in `tests/full_provider_conformance.rs`: an
//! integration target that owns the global for its whole binary.
//!
//! Running against `InMemoryMemoryStore` rather than a SQLite workspace is
//! deliberate and is also the sharper test: it is the engine's simplest
//! `Memory`, so anything the suite catches is the *adapter's* behaviour rather
//! than the storage engine's.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use tinycortex::memory::store::InMemoryMemoryStore;

#[tokio::test]
async fn the_tinycortex_driver_upholds_the_contract() {
    let driver = crate::provider(Arc::new(InMemoryMemoryStore::new()));
    tinymemory_conformance::assert_provider(Arc::new(driver)).await;
}

/// The suite skips every write-path assertion when a driver does not retain, so
/// a backend that silently dropped writes would let the run above pass having
/// asserted almost nothing. This pins that it does retain.
#[tokio::test]
async fn the_backend_actually_retains() {
    let driver = crate::provider(Arc::new(InMemoryMemoryStore::new()));
    assert!(
        tinymemory_conformance::retains_writes(&driver).await,
        "the engine's in-memory store must retain writes, or the suite above \
         reports success having run four assertions of eleven"
    );
}
