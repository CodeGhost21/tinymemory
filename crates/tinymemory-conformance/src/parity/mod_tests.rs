//! Recall parity scoring, against the reference driver's substring recall.

#![allow(clippy::expect_used)]

use std::collections::HashSet;
use std::time::Duration;

use tinymemory_api::provider::MemoryCore;

use super::{measure, Latency, ParityNote, ParityReport, BUNDLED_CORPUS};
use crate::InMemoryProvider;

#[test]
fn latency_percentiles_use_the_nearest_rank() {
    let samples: Vec<Duration> = (1..=20).map(Duration::from_millis).collect();
    let latency = Latency::of(&samples);
    assert_eq!(latency.p50, Duration::from_millis(10));
    assert_eq!(latency.p95, Duration::from_millis(19));
    assert_eq!(latency.max, Duration::from_millis(20));
}

#[test]
fn the_latency_of_nothing_is_zero() {
    assert_eq!(Latency::of(&[]), Latency::default());
}

#[test]
fn the_bundled_corpus_asks_in_other_words() {
    let keys: HashSet<&str> = BUNDLED_CORPUS.iter().map(|n| n.key).collect();
    assert_eq!(keys.len(), BUNDLED_CORPUS.len(), "keys must be unique");
    for note in &BUNDLED_CORPUS {
        assert!(
            !note
                .note
                .to_lowercase()
                .contains(&note.question.to_lowercase()),
            "`{}` quotes its note, so it would measure matching, not recall",
            note.key
        );
    }
}

#[tokio::test]
async fn an_engine_that_finds_each_note_first_scores_one() {
    // The reference driver's recall is a substring match, so a question that
    // is a fragment of exactly one note finds it and nothing else.
    let corpus = [
        ParityNote {
            key: "a",
            note: "the heron nests by the mill pond",
            question: "heron nests",
        },
        ParityNote {
            key: "b",
            note: "the kettle is in the left cupboard",
            question: "kettle is in",
        },
    ];
    let provider = InMemoryProvider::new();
    let report = measure(&provider, "parity-test", &corpus)
        .await
        .expect("measure");
    assert_eq!((report.notes, report.questions), (2, 2));
    assert!((report.hit_at_1 - 1.0).abs() < f64::EPSILON, "{report:?}");
    assert!((report.hit_at_5 - 1.0).abs() < f64::EPSILON, "{report:?}");
    assert!((report.mrr - 1.0).abs() < f64::EPSILON, "{report:?}");
    assert!(report.missed.is_empty());
}

#[tokio::test]
async fn a_paraphrase_a_word_matcher_cannot_follow_is_a_miss() {
    let provider = InMemoryProvider::new();
    let report = measure(&provider, "parity-test", &BUNDLED_CORPUS)
        .await
        .expect("measure");
    assert_eq!(report.driver, crate::REFERENCE_DRIVER_ID);
    assert_eq!(report.notes, BUNDLED_CORPUS.len());
    assert!(report.hit_at_1 <= report.mrr && report.mrr <= report.hit_at_5);
    assert_eq!(
        report.missed.len(),
        BUNDLED_CORPUS.len(),
        "no bundled question is a substring of its note"
    );
}

#[tokio::test]
async fn measuring_leaves_nothing_behind() {
    let provider = InMemoryProvider::new();
    measure(&provider, "parity-test", &BUNDLED_CORPUS)
        .await
        .expect("measure");
    let left = provider
        .list(Some("parity-test"), None, None)
        .await
        .expect("list");
    assert!(left.is_empty(), "{} notes left behind", left.len());
}

#[test]
fn a_report_renders_as_a_table_row() {
    let report = ParityReport {
        driver: "d".into(),
        notes: 30,
        questions: 30,
        hit_at_1: 0.5,
        hit_at_5: 0.75,
        mrr: 0.6,
        store: Latency {
            p50: Duration::from_millis(12),
            p95: Duration::from_millis(40),
            max: Duration::from_millis(41),
        },
        recall: Latency {
            p50: Duration::from_millis(3),
            p95: Duration::from_millis(9),
            max: Duration::from_millis(9),
        },
        missed: Vec::new(),
    };
    assert_eq!(
        report.markdown_row("embedded"),
        "| embedded | 30 | 0.50 | 0.75 | 0.60 | 12 / 40 ms | 3 / 9 ms |"
    );
    assert_eq!(
        ParityReport::markdown_header().lines().count(),
        2,
        "a header row and its rule"
    );
}
