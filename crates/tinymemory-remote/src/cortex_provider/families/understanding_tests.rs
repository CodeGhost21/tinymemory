//! The server's derived layers as a forest: what each layer item reads as,
//! where it hangs, what is left out, and how often the layers are read.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use tinymemory_api::provider::types::SourceScope;
use tinymemory_api::provider::MemoryTree;

use super::*;
use crate::cortex_provider::families::test_support::{hosted, requests};
use crate::hosted_test_support::Shared;

fn entity(name: &str) -> Value {
    json!({ "type": "entity", "id": format!("ent_{name}"), "name": name })
}

fn literal(value: Value) -> Value {
    json!({ "type": "literal", "datatype": "string", "value": value })
}

fn fact_item(id: &str, subject: &str, object: &str, supports: &[&str], confidence: f64) -> Value {
    json!({
        "id": id,
        "scope": "ignored",
        "subject": entity(subject),
        "predicate": "works_at",
        "object": entity(object),
        "supports": supports,
        "valid_from": "2026-09-01T00:00:00Z",
        "recorded_from": "2026-09-01T00:00:00Z",
        "confidence": confidence,
    })
}

fn support(kind: &str, id: &str, weight: f64, polarity: &str) -> Value {
    json!({ "type": kind, "id": id, "weight": weight, "polarity": polarity })
}

fn belief_item(id: &str, supports: Vec<Value>, confidence: f64, stance: &str) -> Value {
    json!({
        "id": id,
        "scope": "ignored",
        "claim": {
            "subject": entity("Ada"),
            "predicate": "prefers",
            "object": literal(json!("tea")),
        },
        "stance": stance,
        "confidence": confidence,
        "supports": supports,
        "valid_from": "2026-09-02T00:00:00Z",
        "recorded_from": "2026-09-02T00:00:00Z",
        "last_revised_at": "2026-09-05T00:00:00Z",
    })
}

fn concept_item(id: &str, name: &str, beliefs: &[&str], facts: &[&str], confidence: f64) -> Value {
    json!({
        "id": id,
        "scope": "ignored",
        "name": name,
        "version": 1,
        "summary": format!("{name} in short"),
        "supported_by": { "beliefs": beliefs, "facts": facts },
        "confidence": confidence,
        "valid_from": "2026-09-03T00:00:00Z",
        "recorded_from": "2026-09-03T00:00:00Z",
        "last_synthesized_at": "2026-09-06T00:00:00Z",
    })
}

fn nodes(layer: &[Value], parse: fn(&Value) -> Option<Node>) -> Vec<Node> {
    layer.iter().filter_map(parse).collect()
}

/// What the double's layer routes answer for `layer` in `namespace`.
fn seed(
    provider: &CortexProvider,
    state: &Shared,
    layer: &str,
    namespace: &str,
    items: Vec<Value>,
) {
    let scope = provider.dialect.scope_for(namespace).expect("scope");
    state
        .layers
        .lock()
        .expect("layers")
        .insert((layer.to_string(), scope), items);
}

fn layer_reads(state: &Shared) -> usize {
    requests(state)
        .iter()
        .filter(|request| {
            [
                "/memory/facts?",
                "/memory/beliefs?",
                "/memory/understanding?",
            ]
            .iter()
            .any(|route| request.contains(route))
        })
        .count()
}

#[test]
fn claims_read_as_sentences() {
    let item = fact_item("fact_1", "Ada", "Acme", &[], 0.5);
    assert_eq!(fact(&item).expect("a fact").text, "Ada works at Acme");
    let mut number = item.clone();
    number["object"] = literal(json!(42));
    assert_eq!(fact(&number).expect("a fact").text, "Ada works at 42");
    let mut unnamed = item.clone();
    unnamed["subject"] = json!({ "type": "entity", "id": "ent_7" });
    assert_eq!(fact(&unnamed).expect("a fact").text, "ent_7 works at Acme");
    let mut empty = item;
    empty["object"] = literal(Value::Null);
    assert!(fact(&empty).is_none(), "a claim with no object is no node");

    let contradicted = belief_item("belief_1", Vec::new(), 0.5, "contradicted");
    assert_eq!(
        belief(&contradicted).expect("a belief").text,
        "Ada prefers tea (contradicted)"
    );
    let uncertain = belief_item("belief_2", Vec::new(), 0.5, "uncertain");
    assert_eq!(
        belief(&uncertain).expect("a belief").text,
        "Ada prefers tea (uncertain)"
    );
    assert_eq!(
        concept(&concept_item("concept_1", "Tea", &[], &[], 0.5))
            .expect("a concept")
            .text,
        "Tea — Tea in short"
    );
    let mut terse = concept_item("concept_2", "Tea", &[], &[], 0.5);
    terse["summary"] = json!("Tea");
    assert_eq!(concept(&terse).expect("a concept").text, "Tea");
}

#[test]
fn what_the_server_set_aside_is_left_out() {
    let live = fact_item("fact_1", "Ada", "Acme", &[], 0.5);
    let mut superseded = live.clone();
    superseded["superseded_by"] = json!("fact_2");
    let mut struck = live.clone();
    struck["recorded_to"] = json!("2026-09-04T00:00:00Z");
    let mut kept = live.clone();
    kept["superseded_by"] = Value::Null;
    assert!(fact(&live).is_some());
    assert!(fact(&kept).is_some(), "a null successor is no successor");
    assert!(fact(&superseded).is_none());
    assert!(fact(&struck).is_none());
    assert!(belief(&belief_item("belief_1", Vec::new(), 0.5, "deprecated")).is_none());
    let mut merged = concept_item("concept_1", "Tea", &[], &[], 0.5);
    merged["canonical_id"] = json!("concept_9");
    assert!(concept(&merged).is_none());
}

#[test]
fn each_node_hangs_under_its_most_confident_citer() {
    let facts = [
        fact_item("fact_1", "Ada", "Acme", &["evt_a", "evt_b"], 0.9),
        fact_item("fact_2", "Bo", "Acme", &["evt_b", "evt_c"], 0.5),
    ];
    let beliefs = [
        belief_item(
            "belief_1",
            vec![
                support("fact", "fact_1", 0.9, "for"),
                support("fact", "fact_2", 0.2, "for"),
                support("event", "evt_d", 0.1, "for"),
            ],
            0.8,
            "supported",
        ),
        // The most confident belief argues against fact_2: no parent of it.
        belief_item(
            "belief_2",
            vec![support("fact", "fact_2", 0.5, "against")],
            0.95,
            "supported",
        ),
        belief_item(
            "belief_3",
            vec![support("fact", "fact_2", 0.5, "for")],
            0.3,
            "supported",
        ),
    ];
    let concepts = [
        concept_item("concept_1", "Work", &["belief_1"], &["fact_2"], 0.7),
        concept_item("concept_2", "Tea", &["belief_1"], &[], 0.9),
    ];
    let mut forest = Forest::default();
    plant(
        &mut forest,
        "global",
        [
            nodes(&facts, fact),
            nodes(&beliefs, belief),
            nodes(&concepts, concept),
        ],
    );
    let placed: Vec<(&str, u32, Option<&str>, Vec<&str>)> = forest
        .summaries
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.level,
                node.parent_id.as_deref(),
                node.child_ids.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        placed,
        vec![
            ("fact_1", 1, Some("belief_1"), vec!["evt_a", "evt_b"]),
            ("fact_2", 1, Some("belief_1"), vec!["evt_c"]),
            ("belief_2", 2, None, vec![]),
            (
                "belief_1",
                2,
                Some("concept_2"),
                vec!["evt_d", "fact_1", "fact_2"]
            ),
            ("belief_3", 2, None, vec![]),
            ("concept_2", 3, None, vec!["belief_1"]),
            ("concept_1", 3, None, vec![]),
        ]
    );
    assert_eq!(
        forest.leaf_parents["evt_b"], "fact_1",
        "the more confident fact"
    );
    assert_eq!(forest.leaf_parents["evt_d"], "belief_1", "no fact cites it");
    let node = &forest.summaries[0];
    assert_eq!(node.tree_id, "hosted:global");
    assert_eq!(node.tree_kind, TREE_KIND);
    assert_eq!(node.tree_scope, "global");
    assert_eq!(node.preview.as_deref(), Some("Ada works at Acme"));
    let revised = &forest.summaries[3];
    assert_eq!(
        (
            revised.time_range_start.to_rfc3339(),
            revised.time_range_end.to_rfc3339()
        ),
        (
            "2026-09-02T00:00:00+00:00".to_string(),
            "2026-09-05T00:00:00+00:00".to_string()
        )
    );
}

#[tokio::test]
async fn the_forest_is_read_from_four_namespaces_once_a_minute() {
    let (provider, state) = hosted().await;
    seed(
        &provider,
        &state,
        "facts",
        "global",
        vec![fact_item("fact_1", "Ada", "Acme", &["evt_a"], 0.9)],
    );
    seed(
        &provider,
        &state,
        "understanding",
        "sources/email",
        vec![concept_item("concept_1", "Invoices", &[], &[], 0.6)],
    );
    let forest = provider.summary_forest(100, None).await.expect("forest");
    assert!(!forest.truncated);
    let trees: Vec<(&str, &str)> = forest
        .summaries
        .iter()
        .map(|node| (node.id.as_str(), node.tree_scope.as_str()))
        .collect();
    assert_eq!(
        trees,
        vec![("fact_1", "global"), ("concept_1", "sources/email")]
    );
    assert_eq!(layer_reads(&state), 12, "four namespaces, three layers");
    provider.summary_forest(100, None).await.expect("again");
    provider.recent_leaves(10, None).await.expect("leaves");
    assert_eq!(layer_reads(&state), 12, "one reading serves a minute");
    let cut = provider.summary_forest(1, None).await.expect("cut");
    assert_eq!(cut.summaries.len(), 1);
    assert!(cut.truncated);
}

#[tokio::test]
async fn a_long_layer_is_paged_and_cut_at_its_cap() {
    let (provider, state) = hosted().await;
    let many: Vec<Value> = (0..MAX_LAYER_ITEMS + 1)
        .map(|i| fact_item(&format!("fact_{i}"), "Ada", "Acme", &[], 0.5))
        .collect();
    seed(&provider, &state, "facts", "global", many);
    let forest = provider
        .summary_forest(usize::MAX, None)
        .await
        .expect("forest");
    assert!(
        forest.truncated,
        "a layer cut at its cap truncates the forest"
    );
    assert_eq!(forest.summaries.len(), MAX_LAYER_ITEMS);
    let fact_pages = requests(&state)
        .iter()
        .filter(|request| request.contains("/memory/facts?"))
        .count();
    assert_eq!(
        fact_pages,
        MAX_LAYER_ITEMS / LAYER_PAGE + 3,
        "10 pages, then 3 more namespaces"
    );
}

#[tokio::test]
async fn a_source_scoped_caller_is_shown_no_derived_nodes() {
    let (provider, state) = hosted().await;
    seed(
        &provider,
        &state,
        "facts",
        "global",
        vec![fact_item("fact_1", "Ada", "Acme", &[], 0.9)],
    );
    let scoped = provider
        .summary_forest(100, Some(&SourceScope::new(["gmail:me"])))
        .await
        .expect("forest");
    assert!(scoped.summaries.is_empty());
    assert!(!scoped.truncated);
    assert_eq!(layer_reads(&state), 0);
}

#[tokio::test]
async fn an_expired_reading_is_read_again() {
    let (provider, state) = hosted().await;
    let provider = provider.with_families(crate::cortex_provider::families::FamilyState {
        forest: ForestCache::new(std::time::Duration::ZERO),
        ..crate::cortex_provider::families::FamilyState::default()
    });
    provider.summary_forest(10, None).await.expect("forest");
    provider.summary_forest(10, None).await.expect("again");
    assert_eq!(layer_reads(&state), 24);
}
