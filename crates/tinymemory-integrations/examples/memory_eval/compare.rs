//! Comparing runs: which CortexDB flags move which KPI.
//!
//! `memory_eval compare <run.json>…` reads the `--json` reports of several
//! runs, groups them by flag profile (the `profile` each records; the label
//! when there is none), averages repeats, and prints one table per KPI
//! group with every profile's delta from the baseline (the `baseline`
//! profile, else the first file).
//!
//! A delta is called a move only when it is larger than the noise: the
//! spread (max − min) the repeats of either profile show, and never less
//! than a floor for a single run (3 points for a percentage, 0.03 for a
//! score, 10% of the baseline otherwise). A move is marked `▲` when it is an
//! improvement, `▼` when it is a regression, and `~` when it is within noise.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::kpi::{Better, Kpi, Unit, format};

type Error = Box<dyn std::error::Error>;

/// The parts of a run's report a comparison reads.
#[derive(Deserialize)]
struct Run {
    label: String,
    #[serde(default)]
    profile: Option<String>,
    /// The flags the profile set on the server.
    #[serde(default)]
    flags: BTreeMap<String, String>,
    #[serde(default)]
    kpis: Vec<Kpi>,
}

/// Every run of one profile.
struct Profile {
    name: String,
    flags: BTreeMap<String, String>,
    runs: usize,
    /// Each KPI's values across the runs, by name.
    values: BTreeMap<String, Vec<f64>>,
}

impl Profile {
    fn mean(&self, kpi: &str) -> Option<f64> {
        let values = self.values.get(kpi)?;
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }

    fn spread(&self, kpi: &str) -> f64 {
        self.values.get(kpi).map_or(0.0, |values| {
            let max = values.iter().copied().fold(f64::MIN, f64::max);
            let min = values.iter().copied().fold(f64::MAX, f64::min);
            if values.is_empty() { 0.0 } else { max - min }
        })
    }
}

/// How a profile's KPI compares with the baseline's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Move {
    Better,
    Worse,
    /// Within noise.
    Same,
    /// Changed, but the KPI has no better direction.
    Changed,
}

/// The smallest delta a single run can call a move.
fn floor(unit: Unit, baseline: f64) -> f64 {
    match unit {
        Unit::Pct | Unit::Points => 3.0,
        Unit::Score => 0.03,
        Unit::Usd | Unit::Count | Unit::Ms => 0.1 * baseline.abs(),
    }
}

/// Whether `value` moved from `baseline` by more than `noise`, and which way.
pub(crate) fn judge(baseline: f64, value: f64, noise: f64, unit: Unit, better: Better) -> Move {
    let delta = value - baseline;
    if delta.abs() <= noise.max(floor(unit, baseline)) {
        return Move::Same;
    }
    match (better, delta > 0.0) {
        (Better::Higher, true) | (Better::Lower, false) => Move::Better,
        (Better::Higher, false) | (Better::Lower, true) => Move::Worse,
        (Better::Neither, _) => Move::Changed,
    }
}

/// `value - baseline`, in a form readable next to `value`.
fn delta(baseline: f64, value: f64, unit: Unit) -> String {
    match unit {
        Unit::Pct | Unit::Points => format!("{:+.0} pp", value - baseline),
        Unit::Score => format!("{:+.2}", value - baseline),
        _ if baseline == 0.0 => format!("{:+.0}", value - baseline),
        _ => format!("{:+.0}%", 100.0 * (value - baseline) / baseline.abs()),
    }
}

/// Reads `paths` and prints the comparison.
///
/// # Errors
///
/// A file that cannot be read or is not a run's report.
pub(crate) fn run(paths: &[String]) -> Result<(), Error> {
    if paths.is_empty() {
        return Err("compare needs at least one run's --json report".into());
    }
    let mut profiles: Vec<Profile> = Vec::new();
    // The KPIs in the order the first run that has them lists them.
    let mut order: Vec<Kpi> = Vec::new();
    for path in paths {
        let run: Run = serde_json::from_str(&std::fs::read_to_string(path)?)
            .map_err(|error| format!("{path}: {error}"))?;
        let name = run.profile.clone().unwrap_or_else(|| run.label.clone());
        let at = match profiles.iter().position(|p| p.name == name) {
            Some(at) => at,
            None => {
                profiles.push(Profile {
                    name,
                    flags: run.flags.clone(),
                    runs: 0,
                    values: BTreeMap::new(),
                });
                profiles.len() - 1
            }
        };
        let profile = &mut profiles[at];
        profile.runs += 1;
        for kpi in run.kpis {
            if !order.iter().any(|k| k.group == kpi.group && k.name == kpi.name) {
                order.push(kpi.clone());
            }
            if let Some(value) = kpi.value {
                profile.values.entry(kpi.name).or_default().push(value);
            }
        }
    }
    let base = profiles
        .iter()
        .position(|p| p.name == "baseline")
        .unwrap_or(0);
    profiles.swap(0, base);
    let baseline = &profiles[0];

    println!("# CortexDB flag comparison\n");
    println!(
        "Deltas are against `{}`. ▲ better, ▼ worse, ~ within noise (the repeats' \
         spread, at least 3 pp / 0.03 / 10%).\n",
        baseline.name
    );
    println!("| Profile | Runs | Flags over the baseline |");
    println!("| --- | --- | --- |");
    for profile in &profiles {
        let flags: Vec<String> = profile
            .flags
            .iter()
            .filter(|(key, value)| baseline.flags.get(*key) != Some(*value))
            .map(|(key, value)| format!("`{key}={value}`"))
            .collect();
        println!(
            "| {} | {} | {} |",
            profile.name,
            profile.runs,
            if flags.is_empty() {
                "–".to_string()
            } else {
                flags.join(" ")
            }
        );
    }

    let mut moved: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut groups: Vec<&str> = order.iter().map(|k| k.group.as_str()).collect();
    groups.dedup();
    for group in groups {
        let kpis: Vec<&Kpi> = order.iter().filter(|k| k.group == group).collect();
        println!("\n## {group}\n");
        let names: Vec<&str> = kpis.iter().map(|k| k.name.as_str()).collect();
        println!("| Profile | {} |", names.join(" | "));
        println!("| --- |{}", " --- |".repeat(kpis.len()));
        for profile in &profiles {
            let mut cells = Vec::new();
            for kpi in &kpis {
                let Some(value) = profile.mean(&kpi.name) else {
                    cells.push("–".to_string());
                    continue;
                };
                let shown = format(value, kpi.unit);
                let compared = baseline
                    .mean(&kpi.name)
                    .filter(|_| !std::ptr::eq(profile, baseline));
                cells.push(match compared {
                    None => shown,
                    Some(base) => {
                        let noise = baseline.spread(&kpi.name).max(profile.spread(&kpi.name));
                        let verdict = judge(base, value, noise, kpi.unit, kpi.better);
                        let mark = match verdict {
                            Move::Better => "▲",
                            Move::Worse => "▼",
                            Move::Same => "~",
                            Move::Changed => "Δ",
                        };
                        if matches!(verdict, Move::Better | Move::Worse) {
                            moved.entry(&profile.name).or_default().push(format!(
                                "{mark} {} {}",
                                kpi.name,
                                delta(base, value, kpi.unit)
                            ));
                        }
                        format!("{shown} ({} {mark})", delta(base, value, kpi.unit))
                    }
                });
            }
            println!("| {} | {} |", profile.name, cells.join(" | "));
        }
    }

    println!("\n## What moved\n");
    for profile in profiles.iter().skip(1) {
        match moved.get(profile.name.as_str()) {
            Some(changes) => println!("- **{}**: {}", profile.name, changes.join(", ")),
            None => println!("- **{}**: nothing beyond noise", profile.name),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
