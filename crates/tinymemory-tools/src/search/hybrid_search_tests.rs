use super::*;

#[test]
fn profiles_resolve_by_their_published_names() {
    assert_eq!(
        WeightProfile::by_name("balanced"),
        Some(WeightProfile::BALANCED)
    );
    assert_eq!(
        WeightProfile::by_name("semantic"),
        Some(WeightProfile::SEMANTIC)
    );
    assert_eq!(
        WeightProfile::by_name("lexical"),
        Some(WeightProfile::LEXICAL)
    );
    assert_eq!(
        WeightProfile::by_name("graph_first"),
        Some(WeightProfile::GRAPH_FIRST)
    );
    assert_eq!(WeightProfile::by_name("mystery"), None);
    assert_eq!(WeightProfile::by_name("Balanced"), None);
}

#[test]
fn every_profile_weighs_to_one() {
    for profile in [
        WeightProfile::BALANCED,
        WeightProfile::SEMANTIC,
        WeightProfile::LEXICAL,
        WeightProfile::GRAPH_FIRST,
    ] {
        let sum = profile.graph + profile.vector + profile.keyword + profile.freshness;
        assert!((sum - 1.0).abs() < 1e-9, "{profile:?} sums to {sum}");
    }
}

#[test]
fn the_final_score_is_the_plain_weighted_sum() {
    let score = hybrid_final_score(&WeightProfile::BALANCED, 1.0, 0.5, 0.2, 0.0);
    assert!((score - (0.35 + 0.175 + 0.03)).abs() < 1e-9, "{score}");
    assert_eq!(
        hybrid_final_score(&WeightProfile::SEMANTIC, 0.0, 0.0, 0.0, 1.0),
        0.0
    );
}

#[test]
fn args_default_to_the_balanced_mode_and_ten_results() {
    let args: Args = serde_json::from_value(serde_json::json!({
        "query": "q",
        "namespace": "global"
    }))
    .unwrap();
    assert_eq!(args.mode, "balanced");
    assert_eq!(args.limit, 10);
    assert!(!args.include_breakdown);
}

#[tokio::test]
async fn rejects_unknown_mode_before_opening_external_search_resources() {
    let error = MemoryHybridSearchTool::new(crate::test_host::NoHost)
        .execute(serde_json::json!({
            "query": "release checklist",
            "namespace": "global",
            "mode": "mystery"
        }))
        .await
        .expect_err("an unknown mode must fail validation");

    let message = error.to_string();
    assert!(message.contains("unknown mode 'mystery'"), "{message}");
    // Validation runs before config, provider, and store setup. Reaching any
    // external search path would replace this precise validation error.
    assert!(!message.contains("load config failed"), "{message}");
}

fn hit(key: &str, score: f64, vector: f64) -> NamespaceMemoryHit {
    NamespaceMemoryHit {
        id: key.to_string(),
        kind: MemoryItemKind::Kv,
        namespace: "global".to_string(),
        key: key.to_string(),
        title: None,
        content: key.to_string(),
        category: "core".to_string(),
        source_type: None,
        updated_at: 0.0,
        score,
        score_breakdown: tinymemory_api::types::RetrievalScoreBreakdown {
            vector_similarity: vector,
            final_score: score,
            ..Default::default()
        },
        document_id: None,
        chunk_id: None,
        supporting_relations: Vec::new(),
        taint: tinymemory_api::types::MemoryTaint::Internal,
    }
}

fn keys(hits: &[NamespaceMemoryHit], order: &[(usize, f64)]) -> Vec<String> {
    order.iter().map(|(i, _)| hits[*i].key.clone()).collect()
}

/// A driver that ranks without measuring anything (hosted CortexDB) keeps its
/// order and its rank; re-weighting its zeros would report nothing at all.
#[test]
fn a_rank_only_drivers_order_is_kept() {
    let balanced = WeightProfile::by_name("balanced").expect("balanced");
    let hits = vec![
        hit("first", 1.0, 0.0),
        hit("second", 0.9, 0.0),
        hit("third", 0.8, 0.0),
    ];
    let (order, ranked) = ordered(&hits, &balanced, 2);
    assert!(ranked);
    assert_eq!(keys(&hits, &order), ["first", "second"]);
    assert_eq!(order[0].1, 1.0);
}

#[test]
fn a_scoring_drivers_hits_are_re_weighted() {
    let semantic = WeightProfile::by_name("semantic").expect("semantic");
    let hits = vec![
        hit("weak", 1.0, 0.2),
        hit("strong", 0.5, 0.9),
        hit("none", 0.4, 0.0),
    ];
    let (order, ranked) = ordered(&hits, &semantic, 10);
    assert!(!ranked);
    assert_eq!(
        keys(&hits, &order),
        ["strong", "weak"],
        "a hit scoring nothing is dropped"
    );
}

#[test]
fn one_measured_signal_makes_a_ranking_scored() {
    assert!(!rank_only(&[]));
    let mut fresh = hit("a", 1.0, 0.0);
    fresh.score_breakdown.freshness = 0.5;
    assert!(!rank_only(&[fresh]));
    let mut zero = hit("b", 0.0, 0.0);
    zero.score_breakdown.final_score = 0.0;
    assert!(!rank_only(&[zero]), "a zero final score is not a rank");
}
