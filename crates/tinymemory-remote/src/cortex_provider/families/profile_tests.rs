//! The learned profile over the hosted wire: facets stored as the host
//! computes them, and the one merge that is the driver's.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{
    FacetState, FacetType, MemoryCore, MemoryProfile, ProfileFacet, UserState,
};

use super::*;
use crate::cortex_provider::families::test_support::hosted;

fn facet(key: &str, facet_type: FacetType, stability: f64, evidence: i32) -> ProfileFacet {
    ProfileFacet {
        facet_id: format!("id-{key}"),
        facet_type,
        key: key.to_string(),
        value: format!("value of {key}"),
        confidence: 0.5,
        evidence_count: evidence,
        source_segment_ids: None,
        first_seen_at: 1.0,
        last_seen_at: 1.0,
        state: FacetState::Active,
        stability,
        user_state: UserState::Auto,
        evidence_refs: Vec::new(),
        class: None,
        cue_families: None,
    }
}

#[test]
fn like_matches_as_sqlite_does() {
    assert!(like("skill:github:%:login", "skill:github:1:login"));
    assert!(like("SKILL:%", "skill:x"));
    assert!(like("a_c", "abc"));
    assert!(!like("a_c", "abbc"));
    assert!(like("%", ""));
    assert!(!like("x%", ""));
    assert!(like("%login", "skill:login"));
}

#[test]
fn a_new_facets_class_comes_from_its_key_then_its_type() {
    assert_eq!(class_of("veto/never-email", FacetType::Context), "veto");
    assert_eq!(
        class_of("skill:github:1:login", FacetType::Context),
        "tooling"
    );
    assert_eq!(class_of("misc/x", FacetType::Role), "identity");
    assert_eq!(class_of("plain", FacetType::Workflow), "tooling");
    assert_eq!(class_of("plain", FacetType::Preference), "style");
}

#[test]
fn segments_merge_without_repeats() {
    assert_eq!(merged_segments(Some("a,b"), Some("b")), "a,b");
    assert_eq!(merged_segments(Some("a"), Some("b")), "a,b");
    assert_eq!(merged_segments(Some(""), Some("b")), "b");
    assert_eq!(merged_segments(Some("a"), None), "a");
    assert_eq!(merged_segments(None, None), "");
}

#[tokio::test]
async fn a_provider_facet_merges_by_confidence() {
    let (provider, _state) = hosted().await;
    provider
        .upsert_provider_facet(
            "prf-1",
            FacetType::Preference,
            "style/verbosity",
            "terse",
            0.7,
            Some("seg-1"),
            10.0,
        )
        .await
        .expect("insert");
    provider
        .upsert_provider_facet(
            "prf-2",
            FacetType::Preference,
            "style/verbosity",
            "chatty",
            0.4,
            Some("seg-2"),
            20.0,
        )
        .await
        .expect("weaker");
    let facet = provider
        .get_facet("style/verbosity")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(facet.facet_id, "prf-1");
    assert_eq!(
        facet.value, "terse",
        "a weaker observation does not overwrite"
    );
    assert_eq!(facet.evidence_count, 2);
    assert_eq!(facet.source_segment_ids.as_deref(), Some("seg-1,seg-2"));
    assert_eq!(facet.last_seen_at, 20.0);
    assert_eq!(facet.class.as_deref(), Some("style"));
    provider
        .upsert_provider_facet(
            "prf-3",
            FacetType::Preference,
            "style/verbosity",
            "balanced",
            0.9,
            Some("seg-1"),
            30.0,
        )
        .await
        .expect("stronger");
    let facet = provider
        .get_facet("style/verbosity")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(facet.value, "balanced");
    assert_eq!(facet.source_segment_ids.as_deref(), Some("seg-1,seg-2"));
    assert_eq!(
        provider.list_all_facets().await.expect("all").len(),
        1,
        "each observation replaced the one record"
    );
}

#[tokio::test]
async fn a_provider_facet_of_another_type_starts_over() {
    let (provider, _state) = hosted().await;
    provider
        .upsert_provider_facet("a", FacetType::Role, "who", "engineer", 0.9, None, 1.0)
        .await
        .expect("role");
    provider
        .upsert_provider_facet("b", FacetType::Context, "who", "on leave", 0.1, None, 2.0)
        .await
        .expect("context");
    let facet = provider
        .get_facet("who")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(facet.facet_type, FacetType::Context);
    assert_eq!(facet.evidence_count, 1);
    assert_eq!(facet.source_segment_ids.as_deref(), Some(""));
}

#[tokio::test]
async fn facets_list_filter_and_drop_as_the_host_expects() {
    let (provider, _state) = hosted().await;
    let mut dropped = facet("goal/old", FacetType::Context, 0.1, 1);
    dropped.state = FacetState::Dropped;
    let mut pinned = facet("veto/pinned", FacetType::Context, 0.05, 1);
    pinned.state = FacetState::Dropped;
    pinned.user_state = UserState::Pinned;
    for facet in [
        facet("style/tone", FacetType::Preference, 0.9, 3),
        facet("skill:github:1:login", FacetType::Workflow, 0.5, 7),
        facet("skill:slack:2:login", FacetType::Workflow, 0.4, 2),
        dropped,
        pinned,
    ] {
        provider.upsert_facet(&facet).await.expect("upsert");
    }
    let active: Vec<String> = provider
        .list_active_facets()
        .await
        .expect("active")
        .into_iter()
        .map(|facet| facet.key)
        .collect();
    assert_eq!(
        active,
        vec!["style/tone", "skill:github:1:login", "skill:slack:2:login"]
    );
    assert_eq!(provider.list_all_facets().await.expect("all").len(), 5);
    let workflows: Vec<i32> = provider
        .facets_by_type(FacetType::Workflow)
        .await
        .expect("by type")
        .into_iter()
        .map(|facet| facet.evidence_count)
        .collect();
    assert_eq!(workflows, vec![7, 2], "most evidence first");

    assert!(
        provider
            .workflow_identity_matches("skill:github:%:login", "value of skill:github:1:login")
            .await
    );
    assert!(
        !provider
            .workflow_identity_matches("skill:github:%", "someone else")
            .await
    );
    assert!(
        !provider
            .workflow_identity_matches("style/%", "value of style/tone")
            .await,
        "not a workflow facet"
    );

    assert!(provider
        .set_facet_user_state("style/tone", UserState::Pinned)
        .await
        .expect("pin"));
    assert!(!provider
        .set_facet_user_state("nope", UserState::Pinned)
        .await
        .expect("unknown"));
    assert_eq!(
        provider
            .get_facet("style/tone")
            .await
            .expect("get")
            .expect("present")
            .user_state,
        UserState::Pinned
    );

    assert_eq!(
        provider.drop_facets_below(0.2).await.expect("drop"),
        1,
        "pinned is exempt"
    );
    assert!(provider.get_facet("goal/old").await.expect("get").is_none());
    assert!(provider
        .get_facet("veto/pinned")
        .await
        .expect("get")
        .is_some());

    assert!(provider
        .delete_facet_by_id("id-skill:slack:2:login")
        .await
        .expect("by id"));
    assert!(!provider
        .delete_facet_by_id("id-nothing")
        .await
        .expect("unknown id"));
    assert!(provider
        .delete_facet("skill:github:1:login")
        .await
        .expect("by key"));
    assert!(!provider
        .delete_facet("skill:github:1:login")
        .await
        .expect("again"));
}

#[tokio::test]
async fn a_facet_needs_a_key() {
    let (provider, _state) = hosted().await;
    let refused = provider
        .upsert_facet(&facet(" ", FacetType::Context, 0.5, 1))
        .await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn facets_are_bookkeeping_not_memory() {
    let (provider, _state) = hosted().await;
    provider
        .upsert_facet(&facet("style/tone", FacetType::Preference, 0.9, 3))
        .await
        .expect("upsert");
    assert!(provider.namespaces().await.expect("namespaces").is_empty());
}
