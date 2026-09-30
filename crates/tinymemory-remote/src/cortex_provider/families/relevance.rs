//! A relevance estimate for results the engine ranks but does not score.
//!
//! The engine's recall returns hits in rank order with no similarity, yet a
//! document hit must carry a score. This estimates one from what is known:
//! where the engine ranked the hit, and how much of the query's wording the hit
//! contains. It is an estimate, not an engine score, and the hit's breakdown
//! says which part is which.

use std::collections::HashSet;

/// The weight of the engine's rank in the estimate.
const RANK_WEIGHT: f64 = 0.3;

/// The weight of word overlap in the estimate.
const OVERLAP_WEIGHT: f64 = 0.7;

/// Words that say nothing about what a query is after.
const FUNCTION_WORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "as", "at", "be", "by", "can", "did", "do", "does", "for",
    "from", "had", "has", "have", "how", "i", "in", "is", "it", "its", "me", "my", "of", "on",
    "or", "our", "so", "that", "the", "their", "them", "there", "they", "this", "to", "was", "we",
    "what", "when", "where", "which", "who", "why", "will", "with", "you", "your",
];

/// The estimate for one hit, with its parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Estimate {
    /// `1 − position / total`: 1 for the engine's best hit.
    pub(super) rank: f64,
    /// The share of the query's content words the hit contains.
    pub(super) overlap: f64,
    /// `0.3 × rank + 0.7 × overlap`.
    pub(super) score: f64,
}

/// A query's content words: lowercased, split on anything but letters and
/// digits, function words dropped, a plural `s` trimmed.
fn content_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !FUNCTION_WORDS.contains(word))
        .map(|word| {
            if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") {
                word[..word.len() - 1].to_string()
            } else {
                word.to_string()
            }
        })
        .collect()
}

/// The estimate for the hit at `position` of `total`, for `query`, over the
/// hit's key and content.
pub(super) fn estimate(
    position: usize,
    total: usize,
    query: &str,
    key: &str,
    content: &str,
) -> Estimate {
    let rank = if total == 0 {
        0.0
    } else {
        1.0 - position as f64 / total as f64
    };
    let mut wanted = content_words(query);
    wanted.sort();
    wanted.dedup();
    let overlap = if wanted.is_empty() {
        0.0
    } else {
        let held: HashSet<String> = content_words(&format!("{key} {content}"))
            .into_iter()
            .collect();
        wanted.iter().filter(|word| held.contains(*word)).count() as f64 / wanted.len() as f64
    };
    Estimate {
        rank,
        overlap,
        score: RANK_WEIGHT * rank + OVERLAP_WEIGHT * overlap,
    }
}

/// How fresh something last updated `age_secs` ago is: 1 when new, halving
/// over the first day.
pub(super) fn freshness(age_secs: f64) -> f64 {
    1.0 / (1.0 + age_secs.max(0.0) / 86_400.0)
}

#[cfg(test)]
#[path = "relevance_test.rs"]
mod test;
