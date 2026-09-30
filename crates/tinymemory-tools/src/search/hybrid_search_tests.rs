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
