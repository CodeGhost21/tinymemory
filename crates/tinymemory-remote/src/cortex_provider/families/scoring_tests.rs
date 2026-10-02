//! Scoring over the hosted wire: entities found on the device, embedding
//! refused, and no request made for either.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::MemoryScoring;

use super::*;
use crate::cortex_provider::families::test_support::{hosted, requests};

#[tokio::test]
async fn scoring_extracts_locally_and_does_not_embed() {
    let (provider, state) = hosted().await;
    let before = requests(&state).len();
    assert_eq!(
        MemoryScoring::extract_entities(
            &provider,
            "Mail Alice@Example.com about https://x.io/a, ping @bob and #Rust"
        )
        .await
        .expect("entities"),
        vec![
            "email:alice@example.com",
            "url:https://x.io/a",
            "handle:bob",
            "hashtag:rust",
            "topic:rust",
        ]
    );
    assert!(MemoryScoring::extract_entities(&provider, "   ")
        .await
        .expect("none")
        .is_empty());
    let embedded = provider.embed_text("anything").await;
    assert!(
        matches!(embedded, Err(MemoryError::Unsupported { .. })),
        "{embedded:?}"
    );
    assert_eq!(provider.embedder_slug().await.expect("slug"), "cloud");
    assert_eq!(
        requests(&state).len(),
        before,
        "scoring never calls the backend"
    );
}

#[test]
fn the_extractor_finds_discriminators_and_ignores_lookalikes() {
    assert_eq!(
        extract_entities("ask ana#1234 (or @carol.) about #1bad and me@x"),
        vec!["handle:carol", "handle:ana#1234"]
    );
}

#[test]
fn an_entity_found_twice_is_listed_once() {
    assert_eq!(
        extract_entities("@Bob and @bob, #tea #Tea http://a.b http://a.b."),
        vec!["url:http://a.b", "handle:bob", "hashtag:tea", "topic:tea"]
    );
}

#[test]
fn lookalikes_are_not_entities() {
    for text in [
        "a@b",
        "@",
        "@.-",
        "#",
        "#a",
        "x#12345",
        "x#123",
        "https://",
        "user@host.c",
        "user@host.c0m",
    ] {
        let found = extract_entities(text);
        assert!(
            found.iter().all(|id| !id.starts_with("email:")
                && !id.starts_with("url:")
                && !id.starts_with("hashtag:")),
            "{text}: {found:?}"
        );
    }
}
