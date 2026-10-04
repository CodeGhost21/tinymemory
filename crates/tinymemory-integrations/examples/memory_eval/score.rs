//! Scoring a pack against a probe, and summing the scores up.
//!
//! A pack is cut into **units**: each bullet, and each prose paragraph (an
//! answered section), in the order the model reads them. Every check is a
//! case-insensitive substring match:
//!
//! - **hit**: every `expect` string is somewhere in the pack.
//! - **rank**: the 1-based unit holding the first `expect` string. Its
//!   reciprocal averages to the MRR.
//! - **stale first**: a superseded value comes in an earlier unit than the
//!   current one. That is the error a reader is most likely to repeat.
//! - **leak**: a `forbidden` string is in the pack.
//! - **answer**: the scripted agent's extractive answer (see `agent`) holds
//!   every `expect` string and no stale one.

use serde::Serialize;

use crate::agent::answer;
use crate::scenarios::{Probe, Style, Via};

/// One probe's outcome in one phase.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub scenario: &'static str,
    pub phase: &'static str,
    pub id: &'static str,
    pub via: &'static str,
    pub style: &'static str,
    /// `None` when the probe expects nothing (a pure leak check).
    pub hit: Option<bool>,
    pub rank: Option<usize>,
    /// The heading of the section holding the first expected string.
    pub section: Option<String>,
    pub stale_present: bool,
    pub stale_first: bool,
    pub leak: bool,
    pub answer: Option<String>,
    pub answer_ok: Option<bool>,
    pub ms: f64,
    pub tokens: usize,
    pub units: usize,
    pub markdown: String,
}

/// Which lifecycle call a probe used.
pub fn via_name(via: &Via) -> &'static str {
    match via {
        Via::Ask => "pre_turn",
        Via::Resume { .. } => "start_session",
        Via::Compact { .. } => "recall_for_compaction",
        Via::Continue { .. } => "pre_turn (in thread)",
    }
}

/// The units of `markdown`, each with its section heading.
fn units(markdown: &str) -> Vec<(String, String)> {
    let mut heading = String::new();
    let mut out = Vec::new();
    let mut in_frontmatter = false;
    for line in markdown.lines() {
        let line = line.trim();
        if line == "---" {
            in_frontmatter = !in_frontmatter;
            continue;
        }
        if in_frontmatter || line.is_empty() || line.starts_with("# ") {
            continue;
        }
        if let Some(title) = line.strip_prefix("## ") {
            heading = title.to_string();
            continue;
        }
        let body = line.strip_prefix("- ").unwrap_or(line);
        out.push((heading.clone(), body.to_lowercase()));
    }
    out
}

/// Scores one pack.
pub fn score(
    scenario: &'static str,
    phase: &'static str,
    probe: &Probe,
    markdown: &str,
    tokens: usize,
    ms: f64,
) -> ProbeResult {
    let lower = markdown.to_lowercase();
    let units = units(markdown);
    let first = |needle: &str| {
        let needle = needle.to_lowercase();
        units.iter().position(|(_, unit)| unit.contains(&needle))
    };
    let expected = probe.expect.first().and_then(|needle| first(needle));
    let stale_at = probe.stale.iter().filter_map(|needle| first(needle)).min();
    let has = |needle: &&str| lower.contains(&needle.to_lowercase());
    let answered = answer(markdown, probe.question);
    let answer_ok = (!probe.expect.is_empty()).then(|| {
        answered.as_deref().is_some_and(|text| {
            let text = text.to_lowercase();
            probe.expect.iter().all(|e| text.contains(&e.to_lowercase()))
                && !probe.stale.iter().any(|s| text.contains(&s.to_lowercase()))
        })
    });
    ProbeResult {
        scenario,
        phase,
        id: probe.id,
        via: via_name(&probe.via),
        style: match probe.style {
            Style::Lexical => "lexical",
            Style::Paraphrase => "paraphrase",
        },
        hit: (!probe.expect.is_empty()).then(|| probe.expect.iter().all(has)),
        rank: expected.map(|at| at + 1),
        section: expected.map(|at| units[at].0.clone()),
        stale_present: probe.stale.iter().any(has),
        stale_first: match (stale_at, expected) {
            (Some(stale), Some(fresh)) => stale < fresh,
            (Some(_), None) => true,
            _ => false,
        },
        leak: probe.forbidden.iter().any(has),
        answer: answered,
        answer_ok,
        ms,
        tokens,
        units: units.len(),
        markdown: markdown.to_string(),
    }
}

/// Totals over a set of results.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub probes: usize,
    pub scored: usize,
    pub hits: usize,
    pub mrr: f64,
    pub answers_ok: usize,
    pub contradictions: usize,
    pub fresh_first: usize,
    pub leak_checks: usize,
    pub leaks: usize,
}

impl Totals {
    /// Sums `results`.
    pub fn of<'a>(results: impl IntoIterator<Item = &'a ProbeResult>) -> Self {
        let mut totals = Self::default();
        let mut reciprocal = 0.0;
        for result in results {
            totals.probes += 1;
            if let Some(hit) = result.hit {
                totals.scored += 1;
                totals.hits += usize::from(hit);
                reciprocal += result.rank.map_or(0.0, |rank| 1.0 / rank as f64);
                totals.answers_ok += usize::from(result.answer_ok == Some(true));
            }
            if result.stale_present || result.stale_first {
                totals.contradictions += 1;
                totals.fresh_first += usize::from(!result.stale_first);
            }
            if result.leak || result.hit.is_none() || result.id.contains("window") {
                totals.leak_checks += 1;
            }
            totals.leaks += usize::from(result.leak);
        }
        if totals.scored > 0 {
            totals.mrr = reciprocal / totals.scored as f64;
        }
        totals
    }

    /// `part` of `whole` as a percentage cell.
    pub fn pct(part: usize, whole: usize) -> String {
        if whole == 0 {
            "–".to_string()
        } else {
            format!("{:.0}% ({part}/{whole})", 100.0 * part as f64 / whole as f64)
        }
    }
}

/// Latency percentiles of a set of samples, in milliseconds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Latency {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

impl Latency {
    /// Percentiles of `samples`.
    pub fn of(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Self {
            n: sorted.len(),
            p50: at(0.5),
            p95: at(0.95),
            max: sorted[sorted.len() - 1],
        }
    }
}
