//! Which families each wire advertises, and that each advertisement is served.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::capabilities::Capability;
use tinymemory_api::provider::MemoryProvider;
use tinymemory_conformance::suite::assert_capability_audit;

use crate::cortex_provider::families::test_support::hosted;

const HOSTED_FAMILIES: [Capability; 10] = [
    Capability::Goals,
    Capability::ToolMemory,
    Capability::Documents,
    Capability::Sources,
    Capability::Maintenance,
    Capability::Retrieval,
    Capability::Profile,
    Capability::Episodic,
    Capability::Scoring,
    Capability::Tree,
];

#[tokio::test]
async fn hosted_memory_advertises_the_families_it_serves() {
    let (provider, _state) = hosted().await;
    let capabilities = provider.capabilities();
    for family in HOSTED_FAMILIES {
        assert!(capabilities.contains(family), "{family:?}");
    }
    for absent in [
        Capability::SourceSync,
        Capability::People,
        Capability::CodingSessions,
        Capability::Entities,
        Capability::Graph,
        Capability::Diff,
        Capability::Chunks,
        Capability::Ingest,
    ] {
        assert!(!capabilities.contains(absent), "{absent:?}");
    }
    assert_capability_audit(&provider);
}

#[tokio::test]
async fn the_direct_wire_advertises_what_it_always_has() {
    let endpoint = crate::conformance_test::cortex_backend().await;
    let memory = crate::CortexMemory::api(&endpoint, "cortex-key").expect("builds");
    let provider = crate::cortex_provider(memory);
    let capabilities = provider.capabilities();
    for family in HOSTED_FAMILIES {
        assert!(!capabilities.contains(family), "{family:?}");
    }
    assert_capability_audit(&provider);
}
