//! With every feature on, each optional crate is reachable through the facade,
//! and the pieces compose: scrub an item, store it in the reference engine,
//! run the conformance suite, and compile a context from what is left.
#![cfg(feature = "full")]

use tinymemory_api::{LearningKind, MemoryEngine, MemoryMeta, StoreItem};

#[tokio::test]
async fn the_optional_crates_compose_through_the_facade() {
    let engine = tinymemory_api::conformance::ReferenceEngine::new();
    tinymemory_api::conformance::run(&engine)
        .await
        .expect("the reference engine conforms");

    let item = StoreItem::learning(
        "prefers answers without the key sk-proj-abcdefghijklmnopqrstuvwxyz0123456789ABCD",
        LearningKind::Preference,
        0.8,
        MemoryMeta::default(),
    );
    let scrubbed = tinymemory_integrations::safety::scrub_item(item);
    assert!(scrubbed.report.changed());
    engine.store(scrubbed.value).await.expect("store");

    let doc = tinymemory_tools::context::compile(
        &engine,
        &tinymemory_tools::context::ContextSpec::default(),
    )
    .await
    .expect("compile");
    assert!(doc.markdown.contains("## Learnings"));
    assert!(!doc.markdown.contains("sk-proj-"));
    assert_eq!(doc.engine, "reference");
}

#[test]
fn the_reader_and_converter_crates_are_reachable() {
    assert_eq!(
        tinymemory_integrations::documents::language_for_path("src/main.rs"),
        Some("rust")
    );
    let _ = std::any::type_name::<tinymemory_integrations::import::Checkpoint>();
    let _ = std::any::type_name::<tinymemory_integrations::sources::MemorySourceEntry>();
}
