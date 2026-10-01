//! The section surface, exercised through the public API only.
//!
//! The unit tests use a double that can seed scores; this suite deliberately
//! cannot, and asserts only what a real caller can observe. `InMemoryProvider`
//! is the conformance crate's reference driver — a driver that actually retains
//! — and `NullMemoryProvider` is one that retains nothing, which is where the
//! "works on every driver" claim is machine-checked rather than asserted.

// A failing assertion in a test *is* a panic; the crate-wide `expect_used` /
// `unwrap_used` / `panic` lints exist to keep the library from panicking, not
// the tests. Same allowance, and same reasoning, as `src/registry/test.rs`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use tinymemory::error::MemoryError;
use tinymemory::namespace::MemorySection;
use tinymemory::null::NullMemoryProvider;
use tinymemory::provider::MemoryProvider;
use tinymemory::recall::OwnedRecallOpts;
use tinymemory::sections::{Sections, NAMESPACE_FILTER_CONFLICT};
use tinymemory::types::{MemoryCategory, MemoryTaint};
use tinymemory_conformance::InMemoryProvider;

fn retaining() -> Arc<dyn MemoryProvider> {
    Arc::new(InMemoryProvider::new())
}

#[tokio::test]
async fn a_conversation_round_trips_through_the_section_surface() {
    let provider = retaining();
    let sections = Sections::new(provider.as_ref());

    let namespace = sections
        .conversations()
        .put(
            "thread-8f21",
            "turn-1",
            "we agreed to ship on the 14th",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("put");

    assert_eq!(namespace.as_str(), "conversation:thread-8f21");

    let entry = sections
        .conversations()
        .get("thread-8f21", "turn-1")
        .await
        .expect("get")
        .expect("the entry must be there");
    assert_eq!(entry.content, "we agreed to ship on the 14th");

    let scopes = sections.conversations().scopes().await.expect("scopes");
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].scope(), "thread-8f21");
    assert_eq!(scopes[0].entries, 1);

    assert!(sections
        .conversations()
        .forget("thread-8f21", "turn-1")
        .await
        .expect("forget"));
    assert_eq!(
        sections
            .conversations()
            .scopes()
            .await
            .expect("scopes")
            .len(),
        0
    );
}

#[tokio::test]
async fn the_three_sections_do_not_see_each_others_entries() {
    let provider = retaining();
    let sections = Sections::new(provider.as_ref());

    for view in [
        sections.conversations(),
        sections.learnings(),
        sections.documents(),
    ] {
        view.put(
            "shared-scope",
            "shared-key",
            "content",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("put");
    }

    // Three sections, three namespaces, one entry each — not one shared row.
    for view in [
        sections.conversations(),
        sections.learnings(),
        sections.documents(),
    ] {
        let scopes = view.scopes().await.expect("scopes");
        assert_eq!(scopes.len(), 1, "section {:?}", view.section());
        assert_eq!(scopes[0].entries, 1);
    }
}

#[tokio::test]
async fn a_section_recall_reaches_every_scope_in_that_section_only() {
    let provider = retaining();
    let sections = Sections::new(provider.as_ref());

    for scope in ["rust-async", "rust-macros"] {
        sections
            .learnings()
            .put(
                scope,
                "note",
                "borrow checker notes",
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await
            .expect("put");
    }
    sections
        .conversations()
        .put(
            "thread-1",
            "note",
            "borrow checker notes",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("put");

    let found = sections
        .recall()
        .across_section(
            &MemorySection::Learning,
            "borrow checker",
            10,
            &OwnedRecallOpts::default(),
            None,
        )
        .await
        .expect("across_section");

    assert_eq!(found.namespaces_searched, 2);
    assert!(!found.truncated);
    assert_eq!(found.hits.len(), 2);
    for hit in &found.hits {
        let namespace = hit.namespace.as_deref().expect("a hit carries a namespace");
        assert!(
            namespace.starts_with("learning:"),
            "leaked out of the section: {namespace}"
        );
    }
}

#[tokio::test]
async fn recall_options_may_not_pin_a_namespace() {
    let provider = retaining();
    let pinned = OwnedRecallOpts {
        namespace: Some("learning:elsewhere".to_string()),
        ..OwnedRecallOpts::default()
    };

    let err = Sections::new(provider.as_ref())
        .recall()
        .across_section(&MemorySection::Learning, "q", 10, &pinned, None)
        .await
        .expect_err("a pinned namespace conflicts with the section");

    match err {
        MemoryError::Invalid(message) => assert_eq!(message, NAMESPACE_FILTER_CONFLICT),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn a_custom_section_is_a_first_class_citizen() {
    let provider = retaining();
    let sections = Sections::new(provider.as_ref());
    let ops = MemorySection::Custom("ops".to_string());

    let namespace = sections
        .section(&ops)
        .put(
            "deploys",
            "2026-01-01",
            "rolled back",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("put");
    assert_eq!(namespace.as_str(), "ops:deploys");

    let scopes = sections.section(&ops).scopes().await.expect("scopes");
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].scope(), "deploys");

    // …and it is not mistaken for one of the named sections.
    assert_eq!(
        sections.documents().scopes().await.expect("scopes").len(),
        0
    );
}

#[tokio::test]
async fn every_call_succeeds_on_a_driver_that_retains_nothing() {
    let provider: Arc<dyn MemoryProvider> = Arc::new(NullMemoryProvider::new());
    let sections = Sections::new(provider.as_ref());

    sections
        .documents()
        .put(
            "handbook",
            "k",
            "content",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("put must succeed even where nothing is retained");

    assert!(sections
        .documents()
        .get("handbook", "k")
        .await
        .expect("get")
        .is_none());
    assert!(sections
        .documents()
        .list_section(None, None)
        .await
        .expect("list_section")
        .is_empty());

    let found = sections
        .recall()
        .in_scope(
            &MemorySection::Document,
            "handbook",
            "q",
            10,
            &OwnedRecallOpts::default(),
            None,
        )
        .await
        .expect("in_scope");
    assert!(found.hits.is_empty());
}
