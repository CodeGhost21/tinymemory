//! The toy scorers behave as documented.

use super::*;

#[test]
fn keyword_scores_the_fraction_of_query_words_found() {
    assert_eq!(keyword("rust borrow", "The Rust book"), 0.5);
    assert_eq!(keyword("rust", "nothing here"), 0.0);
    assert_eq!(keyword("  ", "anything"), 0.0);
}

#[test]
fn the_toy_vector_is_deterministic_and_unit_length() {
    let a = embed("ownership and borrowing");
    let b = embed("ownership and borrowing");
    assert_eq!(a, b);
    let norm: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
    assert_eq!(embed(""), [0.0; 64]);
}

#[test]
fn similar_texts_are_closer_than_unrelated_ones() {
    let query = embed("borrowing rules");
    let near = embed("the borrowing rules of rust");
    let far = embed("zebra xylophone quartz");
    assert!(cosine(&query, &near) > cosine(&query, &far));
}

#[test]
fn each_mode_decides_its_own_hits() {
    assert!(score(FetchMode::Keyword, "rust", "rust code").is_some());
    assert!(score(FetchMode::Keyword, "rust", "python code").is_none());
    assert!(score(FetchMode::Vector, "borrowing", "borrowing rules").is_some());
    assert!(score(FetchMode::Vector, "borrowing", "zzz qqq").is_none());
    assert!(score(FetchMode::Hybrid, "rust", "rust code").is_some());
}
