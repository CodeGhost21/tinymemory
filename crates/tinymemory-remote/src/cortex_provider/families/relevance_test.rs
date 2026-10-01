//! The relevance estimate for ranked, unscored hits.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn the_best_ranked_full_match_scores_one() {
    let guess = estimate(0, 4, "favourite tea", "tea", "my favourite tea is oolong");
    assert_eq!(guess.rank, 1.0);
    assert_eq!(guess.overlap, 1.0);
    assert!((guess.score - 1.0).abs() < 1e-9, "{guess:?}");
}

#[test]
fn function_words_and_plurals_do_not_count_against_a_match() {
    let guess = estimate(0, 1, "what are the teas", "k", "tea");
    assert_eq!(guess.overlap, 1.0);
}

#[test]
fn a_lower_rank_scores_less() {
    let first = estimate(0, 4, "x", "k", "nothing shared");
    let last = estimate(3, 4, "x", "k", "nothing shared");
    assert_eq!(last.rank, 0.25);
    assert!(first.score > last.score);
    assert_eq!(first.overlap, 0.0);
}

#[test]
fn a_query_of_function_words_or_no_hits_has_nothing_to_estimate() {
    assert_eq!(estimate(0, 1, "the and of", "k", "c").overlap, 0.0);
    assert_eq!(estimate(0, 0, "x", "k", "c").rank, 0.0);
}

#[test]
fn freshness_halves_over_a_day() {
    assert_eq!(freshness(0.0), 1.0);
    assert!((freshness(86_400.0) - 0.5).abs() < 1e-9);
    assert_eq!(freshness(-5.0), 1.0);
}
