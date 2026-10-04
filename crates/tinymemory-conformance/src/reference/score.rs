//! The reference engine's scorers: a trivial keyword scorer and a
//! deterministic toy vector.
//!
//! Neither is meant to rank well. They exist so the reference engine can serve
//! all three fetch modes with behaviour that is obvious by inspection.

use tinymemory_api::FetchMode;

/// Dimensions of the toy vector.
const DIMENSIONS: usize = 64;

/// Smallest cosine similarity a vector hit must reach.
pub(crate) const VECTOR_THRESHOLD: f32 = 0.2;

/// Lowercase alphanumeric words of `text`.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The fraction of the query's distinct words that appear in `text`.
pub(crate) fn keyword(query: &str, text: &str) -> f32 {
    let mut wanted = words(query);
    wanted.sort();
    wanted.dedup();
    if wanted.is_empty() {
        return 0.0;
    }
    let held = words(text);
    let found = wanted.iter().filter(|word| held.contains(word)).count();
    found as f32 / wanted.len() as f32
}

/// A unit vector of hashed character trigrams over the lowercase words.
pub(crate) fn embed(text: &str) -> [f32; DIMENSIONS] {
    let mut vector = [0.0_f32; DIMENSIONS];
    for word in words(text) {
        let padded: Vec<char> = format!(" {word} ").chars().collect();
        for window in padded.windows(3) {
            // FNV-1a: deterministic across runs and platforms, unlike the
            // standard library's randomly seeded hasher.
            let mut hash: u32 = 0x811c_9dc5;
            for c in window {
                hash ^= u32::from(*c);
                hash = hash.wrapping_mul(0x0100_0193);
            }
            vector[hash as usize % DIMENSIONS] += 1.0;
        }
    }
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

/// Cosine similarity of two unit vectors.
pub(crate) fn cosine(a: &[f32; DIMENSIONS], b: &[f32; DIMENSIONS]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The score of `text` for `query` in `mode`, or `None` when it is not a hit.
pub(crate) fn score(mode: FetchMode, query: &str, text: &str) -> Option<f32> {
    let lexical = keyword(query, text);
    let semantic = cosine(&embed(query), &embed(text));
    let (score, hit) = match mode {
        FetchMode::Keyword => (lexical, lexical > 0.0),
        FetchMode::Vector => (semantic, semantic >= VECTOR_THRESHOLD),
        FetchMode::Hybrid => (
            (lexical + semantic) / 2.0,
            lexical > 0.0 || semantic >= VECTOR_THRESHOLD,
        ),
    };
    hit.then_some(score)
}

#[cfg(test)]
#[path = "score_tests.rs"]
mod tests;
