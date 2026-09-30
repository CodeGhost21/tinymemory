//! Recall parity: how well, and how fast, an engine finds what it was given.
//!
//! Replacing one engine with another (openhuman#6718: hosted CortexDB for the
//! embedded engine) needs a number, not a feeling, for whether recall got
//! worse. This module is the engine-neutral half of that measurement: a fixed
//! corpus of short personal notes, and one question per note that paraphrases
//! it rather than quoting it, stored and asked through the mandatory
//! `store` / `recall` surface every driver serves and scored the same way for
//! any of them.
//!
//! It reports:
//!
//! - **hit@1** and **hit@5**: the note a question was written against is the
//!   first result, or among the first five;
//! - **MRR**: the mean of `1 / rank`, counting a note outside the first five as
//!   zero;
//! - **store** and **recall** latency (p50, p95, max). A store includes
//!   whatever the driver does before a note is readable — for hosted CortexDB
//!   that is its visibility wait — because that is the latency a caller feels.
//!
//! It measures the keyed-note path that OpenHuman's auto-recall and
//! `memory_recall` tool use, not an engine's whole retrieval (graphs, trees,
//! derived layers), and its corpus is small: it separates an engine that finds
//! paraphrases from one that only matches words, which is the question a
//! cutover has to answer first. The engine wiring (which embedder, which
//! account) belongs to the caller; see the facade's `recall_parity` example.

mod corpus;

use std::time::{Duration, Instant};

use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::MemoryProvider;
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

pub use corpus::{ParityNote, BUNDLED_CORPUS};

/// How many results each question asks for; ranks beyond it count as misses.
pub const RECALL_DEPTH: usize = 5;

/// Latency percentiles over one kind of call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// The median.
    pub p50: Duration,
    /// The 95th percentile (nearest rank).
    pub p95: Duration,
    /// The slowest call.
    pub max: Duration,
}

impl Latency {
    /// Percentiles of `samples`; all zero when there are none.
    #[must_use]
    pub fn of(samples: &[Duration]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        Self {
            p50: nearest_rank(&sorted, 50),
            p95: nearest_rank(&sorted, 95),
            max: sorted.last().copied().unwrap_or_default(),
        }
    }
}

/// The nearest-rank percentile of an ascending slice.
fn nearest_rank(sorted: &[Duration], percentile: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (percentile * sorted.len()).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

/// One engine's recall quality and latency over a corpus.
#[derive(Clone, Debug, PartialEq)]
pub struct ParityReport {
    /// The driver measured.
    pub driver: String,
    /// Notes stored.
    pub notes: usize,
    /// Questions asked.
    pub questions: usize,
    /// Share of questions whose note came first.
    pub hit_at_1: f64,
    /// Share of questions whose note was in the first [`RECALL_DEPTH`].
    pub hit_at_5: f64,
    /// Mean reciprocal rank, a note outside the first [`RECALL_DEPTH`] scoring 0.
    pub mrr: f64,
    /// Time from `store` to its return, per note.
    pub store: Latency,
    /// Time from `recall` to its return, per question.
    pub recall: Latency,
    /// The keys of the notes whose question missed entirely, for a reader who
    /// wants to see which paraphrases an engine could not follow.
    pub missed: Vec<String>,
}

impl ParityReport {
    /// The header of the markdown table [`Self::markdown_row`] fills.
    #[must_use]
    pub fn markdown_header() -> &'static str {
        "| engine | notes | hit@1 | hit@5 | MRR | store p50 / p95 | recall p50 / p95 |\n\
         | --- | --- | --- | --- | --- | --- | --- |"
    }

    /// This report as one row of the markdown table.
    #[must_use]
    pub fn markdown_row(&self, label: &str) -> String {
        format!(
            "| {label} | {} | {:.2} | {:.2} | {:.2} | {} / {} ms | {} / {} ms |",
            self.notes,
            self.hit_at_1,
            self.hit_at_5,
            self.mrr,
            self.store.p50.as_millis(),
            self.store.p95.as_millis(),
            self.recall.p50.as_millis(),
            self.recall.p95.as_millis(),
        )
    }
}

/// Stores every note of `corpus` in `namespace`, asks every question, scores
/// the answers, and forgets the notes again.
///
/// Use a namespace of the run's own: the notes are keyed by their slug, and a
/// namespace shared with other data would let its records compete in the
/// rankings.
///
/// # Errors
///
/// The first `store` or `recall` failure, which leaves a measurement that
/// would describe the failure rather than the engine. Notes already stored are
/// forgotten on the way out regardless.
pub async fn measure(
    provider: &dyn MemoryProvider,
    namespace: &str,
    corpus: &[ParityNote],
) -> Result<ParityReport, MemoryError> {
    let outcome = run(provider, namespace, corpus).await;
    for note in corpus {
        // Best effort: the measurement is already decided.
        let _ = provider.forget(namespace, note.key).await;
    }
    outcome
}

async fn run(
    provider: &dyn MemoryProvider,
    namespace: &str,
    corpus: &[ParityNote],
) -> Result<ParityReport, MemoryError> {
    let mut store_times = Vec::with_capacity(corpus.len());
    for note in corpus {
        let started = Instant::now();
        provider
            .store(
                namespace,
                note.key,
                note.note,
                MemoryCategory::Core,
                None,
                MemoryTaint::Internal,
            )
            .await?;
        store_times.push(started.elapsed());
    }

    let opts = OwnedRecallOpts {
        namespace: Some(namespace.to_string()),
        ..OwnedRecallOpts::default()
    };
    let mut recall_times = Vec::with_capacity(corpus.len());
    let (mut first, mut top, mut reciprocal) = (0_usize, 0_usize, 0.0_f64);
    let mut missed = Vec::new();
    for note in corpus {
        let started = Instant::now();
        let hits = provider
            .recall(note.question, RECALL_DEPTH, &opts, None)
            .await?;
        recall_times.push(started.elapsed());
        match hits
            .iter()
            .take(RECALL_DEPTH)
            .position(|hit| hit.key == note.key)
        {
            Some(position) => {
                first += usize::from(position == 0);
                top += 1;
                reciprocal += 1.0 / (position + 1) as f64;
            }
            None => missed.push(note.key.to_string()),
        }
    }

    let questions = corpus.len();
    let share = |count: f64| {
        if questions == 0 {
            0.0
        } else {
            count / questions as f64
        }
    };
    Ok(ParityReport {
        driver: provider.driver_id().to_string(),
        notes: corpus.len(),
        questions,
        hit_at_1: share(first as f64),
        hit_at_5: share(top as f64),
        mrr: share(reciprocal),
        store: Latency::of(&store_times),
        recall: Latency::of(&recall_times),
        missed,
    })
}

#[cfg(test)]
mod test;
