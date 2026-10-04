//! Tests of the comparison's verdicts and of reading run reports.

use super::*;

#[test]
fn a_rise_in_a_higher_is_better_kpi_beyond_noise_is_better() {
    assert_eq!(
        judge(60.0, 70.0, 0.0, Unit::Pct, Better::Higher),
        Move::Better
    );
    assert_eq!(
        judge(60.0, 50.0, 0.0, Unit::Pct, Better::Higher),
        Move::Worse
    );
}

#[test]
fn a_rise_in_cost_is_worse() {
    assert_eq!(judge(1.0, 1.5, 0.0, Unit::Usd, Better::Lower), Move::Worse);
    assert_eq!(judge(1.0, 0.5, 0.0, Unit::Usd, Better::Lower), Move::Better);
}

#[test]
fn a_delta_within_the_repeats_spread_is_noise() {
    assert_eq!(
        judge(60.0, 70.0, 12.0, Unit::Pct, Better::Higher),
        Move::Same
    );
}

#[test]
fn a_single_run_needs_more_than_the_floor_to_move() {
    // 3 points for a percentage, 10% of the baseline for a cost.
    assert_eq!(
        judge(60.0, 62.0, 0.0, Unit::Pct, Better::Higher),
        Move::Same
    );
    assert_eq!(judge(1.0, 1.05, 0.0, Unit::Usd, Better::Lower), Move::Same);
}

#[test]
fn a_kpi_with_no_direction_only_changes() {
    assert_eq!(
        judge(10.0, 20.0, 0.0, Unit::Count, Better::Neither),
        Move::Changed
    );
}

#[test]
fn deltas_read_in_points_or_percent() {
    assert_eq!(delta(60.0, 70.0, Unit::Pct), "+10 pp");
    assert_eq!(delta(2.0, 1.0, Unit::Usd), "-50%");
    assert_eq!(delta(0.0, 3.0, Unit::Count), "+3");
}

#[test]
fn profiles_average_their_repeats_and_spread_their_range() {
    let profile = Profile {
        name: "baseline".into(),
        flags: BTreeMap::new(),
        runs: 2,
        values: [("pack hit".to_string(), vec![60.0, 70.0])].into(),
    };
    assert_eq!(profile.mean("pack hit"), Some(65.0));
    assert_eq!(profile.spread("pack hit"), 10.0);
    assert_eq!(profile.mean("absent"), None);
    assert_eq!(profile.spread("absent"), 0.0);
}

#[test]
fn comparing_nothing_is_an_error() {
    assert!(run(&[]).is_err());
}

#[test]
fn a_report_that_is_not_json_names_its_file() {
    let dir = std::env::temp_dir().join(format!("memory-eval-compare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let path = dir.join("broken.json");
    std::fs::write(&path, "not json").expect("a temp file");
    let path = path.to_string_lossy().to_string();
    let error = run(std::slice::from_ref(&path)).expect_err("not a report");
    assert!(error.to_string().contains(&path), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}
