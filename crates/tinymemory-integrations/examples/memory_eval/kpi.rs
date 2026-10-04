//! The run's key numbers, grouped by what memory is for.
//!
//! The accuracy tables answer "did this probe pass". The KPIs answer the
//! questions a configuration is chosen on, one number each, so two runs
//! (two CortexDB flag profiles, see `compare`) can be set side by side:
//!
//! - **accuracy**: how often the pack, and an answer read from it, is right.
//! - **learning**: whether corrections and standing instructions stick
//!   (the `learnings` and `learning_from_feedback` scenarios), what belief
//!   building adds over raw recall, and what beliefs CortexDB holds.
//! - **surprise**: whether a break from routine surfaces (`surprise`).
//! - **conflicts**: whether disagreeing sources are flagged (`conflicts`),
//!   whether conflicts are raised where there are none, and whether the
//!   newest value wins a superseded one (`contradictions`).
//! - **cost**: what CortexDB's models and the `--llm` answerer spent, per
//!   correct answer, and the prompt tokens a pack costs the host.
//! - **latency**: what an agent waits for.
//!
//! Unless a KPI says otherwise it is read in the synthesis phase, after the
//! belief build: the state memory settles into.

use serde::{Deserialize, Serialize};

use crate::inspect::Usage;
use crate::score::{Latency, ProbeResult, Totals};
use crate::{ScenarioReport, Timings};

/// Scenarios whose probes test learning.
const LEARNING: [&str; 2] = ["learnings", "learning_from_feedback"];

/// Scenarios whose probes test surprise.
const SURPRISE: [&str; 1] = ["surprise"];

/// The conflicts a scenario plants, each named by a word its subject or
/// values hold. Scenarios not listed plant none, so a conflict raised there
/// is spurious; `contradictions` is left out of both counts, since a
/// superseded value may fairly be either closed or flagged.
const PLANTED: [(&str, &[&str]); 1] = [("conflicts", &["refund"])];

/// Scenarios that may hold conflicts without them counting as spurious.
const CONFLICTED: [&str; 2] = ["conflicts", "contradictions"];

/// What a KPI's value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Unit {
    /// A percentage, 0 to 100.
    Pct,
    /// A difference of percentages, in points.
    Points,
    /// US dollars.
    Usd,
    /// A count.
    Count,
    /// Milliseconds.
    Ms,
    /// A score from 0 to 1.
    Score,
}

/// Which way a KPI improves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Better {
    Higher,
    Lower,
    /// Neither: context, not a target.
    Neither,
}

/// One key number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Kpi {
    pub(crate) group: String,
    pub(crate) name: String,
    /// `None` when the run could not measure it (no `--llm`, no CortexDB).
    pub(crate) value: Option<f64>,
    pub(crate) unit: Unit,
    pub(crate) better: Better,
}

impl Kpi {
    fn new(group: &str, name: &str, value: Option<f64>, unit: Unit, better: Better) -> Self {
        Self {
            group: group.to_string(),
            name: name.to_string(),
            value,
            unit,
            better,
        }
    }

    /// The value as a table cell.
    pub(crate) fn cell(&self) -> String {
        self.value
            .map_or_else(|| "–".to_string(), |v| format(v, self.unit))
    }
}

/// `value` in `unit`, for a table.
pub(crate) fn format(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Pct => format!("{value:.0}%"),
        Unit::Points => format!("{value:+.0} pp"),
        Unit::Usd => format!("${value:.3}"),
        Unit::Count => format!("{value:.0}"),
        Unit::Ms => format!("{value:.0} ms"),
        Unit::Score => format!("{value:.2}"),
    }
}

/// `part` of `whole` as a percentage, if there is a whole.
fn pct(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| 100.0 * part as f64 / whole as f64)
}

/// The probes of `phase` in the scenarios `keep` accepts.
fn probes<'a>(
    reports: &'a [ScenarioReport],
    phase: &'a str,
    keep: impl Fn(&str) -> bool + 'a,
) -> impl Iterator<Item = &'a ProbeResult> + 'a {
    reports
        .iter()
        .filter(move |report| keep(report.name))
        .flat_map(|report| &report.probes)
        .filter(move |probe| probe.phase == phase)
}

/// Every KPI of a run. `usage` is CortexDB's spend over the run, absent on
/// the reference engine.
pub(crate) fn compute(
    reports: &[ScenarioReport],
    usage: Option<&Usage>,
    timings: &Timings,
) -> Vec<Kpi> {
    use Better::{Higher, Lower, Neither};
    let on_cortex = usage.is_some();
    let all = |phase| Totals::of(probes(reports, phase, |_| true));
    let (recall, synthesis) = (all("recall"), all("synthesis"));
    let learning = Totals::of(probes(reports, "synthesis", |n| LEARNING.contains(&n)));
    let surprise = Totals::of(probes(reports, "synthesis", |n| SURPRISE.contains(&n)));
    let conflicts = Totals::of(probes(reports, "synthesis", |n| n == "conflicts"));
    let leaks = Totals::of(reports.iter().flat_map(|r| &r.probes));
    let mut kpis = vec![
        Kpi::new(
            "accuracy",
            "pack hit (recall)",
            pct(recall.hits, recall.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "accuracy",
            "pack hit",
            pct(synthesis.hits, synthesis.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new("accuracy", "MRR", Some(synthesis.mrr), Unit::Score, Higher),
        Kpi::new(
            "accuracy",
            "extractive answer",
            pct(synthesis.answers_ok, synthesis.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "accuracy",
            "model answer",
            pct(synthesis.llm_ok, synthesis.llm_scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "accuracy",
            "captured",
            pct(synthesis.captured, synthesis.captured_checked),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "accuracy",
            "leaks",
            Some(leaks.leaks as f64),
            Unit::Count,
            Lower,
        ),
    ];

    // Learning.
    let mut held = crate::inspect::Captured::default();
    for report in reports {
        held.extend(report.synthesis.captured.clone());
    }
    let contested: usize = held
        .stances
        .iter()
        .filter(|(stance, _)| stance.as_str() != "supported")
        .map(|(_, n)| n)
        .sum();
    let confidence = (!held.confidences.is_empty())
        .then(|| held.confidences.iter().sum::<f64>() / held.confidences.len() as f64);
    let gain = pct(synthesis.hits, synthesis.scored)
        .zip(pct(recall.hits, recall.scored))
        .map(|(after, before)| after - before);
    kpis.extend([
        Kpi::new(
            "learning",
            "lesson in pack",
            pct(learning.hits, learning.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "learning",
            "lesson answered",
            pct(learning.llm_ok, learning.llm_scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "learning",
            "lesson captured",
            pct(learning.captured, learning.captured_checked),
            Unit::Pct,
            Higher,
        ),
        Kpi::new("learning", "synthesis gain", gain, Unit::Points, Higher),
        Kpi::new(
            "learning",
            "beliefs built",
            Some(reports.iter().map(|r| r.synthesis.built).sum::<usize>() as f64),
            Unit::Count,
            Neither,
        ),
        Kpi::new(
            "learning",
            "beliefs held",
            on_cortex.then_some(held.beliefs.len() as f64),
            Unit::Count,
            Neither,
        ),
        Kpi::new(
            "learning",
            "beliefs not supported",
            on_cortex.then_some(contested as f64),
            Unit::Count,
            Neither,
        ),
        Kpi::new(
            "learning",
            "belief confidence",
            confidence,
            Unit::Score,
            Neither,
        ),
        Kpi::new(
            "learning",
            "facts held",
            on_cortex.then_some(held.facts.len() as f64),
            Unit::Count,
            Neither,
        ),
    ]);

    // Surprise.
    kpis.extend([
        Kpi::new(
            "surprise",
            "surprise in pack",
            pct(surprise.hits, surprise.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "surprise",
            "surprise MRR",
            (surprise.scored > 0).then_some(surprise.mrr),
            Unit::Score,
            Higher,
        ),
        Kpi::new(
            "surprise",
            "surprise answered",
            pct(surprise.llm_ok, surprise.llm_scored),
            Unit::Pct,
            Higher,
        ),
    ]);

    // Conflicts.
    let (mut planted, mut found, mut spurious) = (0, 0, 0);
    for report in reports {
        let raised = &report.synthesis.captured.conflicts;
        match PLANTED.iter().find(|(name, _)| *name == report.name) {
            Some((_, words)) => {
                planted += words.len();
                found += words
                    .iter()
                    .filter(|word| raised.iter().any(|c| c.to_lowercase().contains(*word)))
                    .count();
            }
            None if !CONFLICTED.contains(&report.name) => spurious += raised.len(),
            None => {}
        }
    }
    let raised: usize = reports
        .iter()
        .map(|r| r.synthesis.captured.conflicts.len())
        .sum();
    kpis.extend([
        Kpi::new(
            "conflicts",
            "planted conflicts flagged",
            on_cortex.then(|| pct(found, planted)).flatten(),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "conflicts",
            "spurious conflicts",
            on_cortex.then_some(spurious as f64),
            Unit::Count,
            Lower,
        ),
        Kpi::new(
            "conflicts",
            "conflicts raised",
            on_cortex.then_some(raised as f64),
            Unit::Count,
            Neither,
        ),
        Kpi::new(
            "conflicts",
            "disagreement in pack",
            pct(conflicts.hits, conflicts.scored),
            Unit::Pct,
            Higher,
        ),
        Kpi::new(
            "conflicts",
            "fresh first",
            pct(synthesis.fresh_first, synthesis.contradictions),
            Unit::Pct,
            Higher,
        ),
    ]);

    // Cost.
    let answered: Vec<&ProbeResult> = reports
        .iter()
        .flat_map(|r| &r.probes)
        .filter(|p| p.llm_ok.is_some())
        .collect();
    let answer_usd = answered
        .iter()
        .filter_map(|p| p.llm_cost_usd)
        .reduce(|a, b| a + b);
    let answer_tokens: u64 = answered.iter().map(|p| p.llm_tokens).sum();
    let spent = usage.map(|u| u.cost_usd + answer_usd.unwrap_or_default());
    let correct = if synthesis.llm_scored > 0 {
        synthesis.llm_ok
    } else {
        synthesis.answers_ok
    };
    let all_probes: Vec<&ProbeResult> = reports.iter().flat_map(|r| &r.probes).collect();
    let pack_tokens = (!all_probes.is_empty()).then(|| {
        all_probes.iter().map(|p| p.tokens).sum::<usize>() as f64 / all_probes.len() as f64
    });
    kpis.extend([
        Kpi::new(
            "cost",
            "CortexDB models",
            usage.map(|u| u.cost_usd),
            Unit::Usd,
            Lower,
        ),
        Kpi::new(
            "cost",
            "CortexDB model calls",
            usage.map(|u| u.calls as f64),
            Unit::Count,
            Lower,
        ),
        Kpi::new(
            "cost",
            "CortexDB model tokens",
            usage.map(|u| u.tokens as f64),
            Unit::Count,
            Lower,
        ),
        Kpi::new("cost", "answerer", answer_usd, Unit::Usd, Lower),
        Kpi::new(
            "cost",
            "answerer tokens",
            (!answered.is_empty()).then_some(answer_tokens as f64),
            Unit::Count,
            Lower,
        ),
        Kpi::new(
            "cost",
            "per correct answer",
            spent
                .filter(|_| correct > 0)
                .map(|usd| usd / correct as f64),
            Unit::Usd,
            Lower,
        ),
        Kpi::new(
            "cost",
            "pack tokens (mean)",
            pack_tokens,
            Unit::Count,
            Lower,
        ),
    ]);
    if let Some(usage) = usage {
        for (role, spend) in &usage.by_role {
            kpis.push(Kpi::new(
                "cost by role",
                role,
                Some(spend.cost_usd),
                Unit::Usd,
                Lower,
            ));
        }
    }

    // Latency.
    let samples = |keep: &dyn Fn(&str) -> bool| -> Vec<f64> {
        timings
            .0
            .iter()
            .filter(|(step, _)| keep(step))
            .flat_map(|(_, samples)| samples.iter().copied())
            .collect()
    };
    let pre_turn = Latency::of(&samples(&|step| step.starts_with("pre_turn")));
    let probe = Latency::of(&samples(&|step| step.starts_with("probe ")));
    let total = |step: &str| {
        timings
            .0
            .get(step)
            .map(|samples| samples.iter().sum::<f64>())
    };
    kpis.extend([
        Kpi::new(
            "latency",
            "pre_turn p50",
            (pre_turn.n > 0).then_some(pre_turn.p50),
            Unit::Ms,
            Lower,
        ),
        Kpi::new(
            "latency",
            "pre_turn p95",
            (pre_turn.n > 0).then_some(pre_turn.p95),
            Unit::Ms,
            Lower,
        ),
        Kpi::new(
            "latency",
            "probe p50",
            (probe.n > 0).then_some(probe.p50),
            Unit::Ms,
            Lower,
        ),
        Kpi::new(
            "latency",
            "probe p95",
            (probe.n > 0).then_some(probe.p95),
            Unit::Ms,
            Lower,
        ),
        Kpi::new(
            "latency",
            "enrichment drain (total)",
            total("enrichment (queue drained)"),
            Unit::Ms,
            Lower,
        ),
        Kpi::new(
            "latency",
            "belief builds (total)",
            total("synthesis (all builds)"),
            Unit::Ms,
            Lower,
        ),
    ]);
    kpis
}

/// The KPIs as a markdown table, in the order `compute` groups them.
pub(crate) fn print(label: &str, kpis: &[Kpi]) {
    println!("\n## KPIs (`{label}`)\n");
    println!("| Group | KPI | Value | Better |");
    println!("| --- | --- | --- | --- |");
    for kpi in kpis {
        let better = match kpi.better {
            Better::Higher => "higher",
            Better::Lower => "lower",
            Better::Neither => "–",
        };
        println!(
            "| {} | {} | {} | {better} |",
            kpi.group,
            kpi.name,
            kpi.cell()
        );
    }
}

#[cfg(test)]
#[path = "kpi_tests.rs"]
mod tests;
