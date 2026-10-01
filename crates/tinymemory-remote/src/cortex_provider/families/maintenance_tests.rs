//! Maintenance over the hosted wire: reports, not work.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use tinymemory_api::provider::MemoryMaintenance;

use super::*;
use crate::cortex_provider::families::test_support::{hosted, requests};
use crate::cortex_provider::families::FamilyState;

fn probes(state: &crate::hosted_test_support::Shared) -> usize {
    requests(state)
        .iter()
        .filter(|r| r.starts_with("GET /memory/scopes?") && r.contains("tmh%3Aprobe"))
        .count()
}

#[tokio::test]
async fn upkeep_is_the_services_own() {
    let (provider, state) = hosted().await;
    for report in [
        provider.reembed().await.expect("reembed"),
        provider.compact().await.expect("compact"),
        provider.consolidate().await.expect("consolidate"),
    ] {
        assert_eq!(report.examined, 0);
        assert_eq!(report.changed, 0);
        assert_eq!(report.findings.len(), 1);
    }
    assert!(requests(&state).is_empty(), "upkeep sends nothing");
}

#[tokio::test]
async fn a_service_that_answers_is_healthy() {
    let (provider, _state) = hosted().await;
    let diagnosis = provider.diagnose().await.expect("diagnose");
    assert!(diagnosis.healthy);
    assert_eq!(diagnosis.stages.len(), 1);
    assert_eq!(diagnosis.stages[0].stage, "service");
    assert!(diagnosis.stages[0].ok);
    assert!(diagnosis.first_blocking_cause.is_none());
    assert_eq!(
        provider.degraded_state().await.expect("degraded"),
        tinymemory_api::provider::diagnosis::DegradedCapabilities::default()
    );
    let doctor = provider.doctor().await.expect("doctor");
    assert!(doctor.findings.is_empty());
    assert_eq!(doctor.changed, 0);
}

#[tokio::test]
async fn each_failure_is_named_in_the_host_vocabulary() {
    for ((status, code), (named, class)) in [
        ((401, "UNAUTHORIZED"), ("auth_invalid", "unrecoverable")),
        (
            (402, "USER_INSUFFICIENT_CREDITS"),
            ("budget_exhausted", "unrecoverable"),
        ),
        ((503, "UNAVAILABLE"), ("storage_unavailable", "transient")),
        ((418, "TEAPOT"), ("transient", "transient")),
    ] {
        let (provider, state) = hosted().await;
        *state.fail_all.lock().expect("fail") = Some((status, code));
        let diagnosis = provider.diagnose().await.expect("diagnose");
        assert!(!diagnosis.healthy, "{status}");
        let cause = diagnosis.first_blocking_cause.expect("cause");
        assert_eq!(cause.code, named, "{status}");
        assert_eq!(cause.class.as_deref(), Some(class), "{status}");
        assert_eq!(
            cause.remediation_key,
            format!("memory.health.remediation.{named}")
        );
        let degraded = provider.degraded_state().await.expect("degraded");
        assert!(degraded.storage, "{status}");
        assert_eq!(degraded.cause.expect("cause").code, named);
        let doctor = provider.doctor().await.expect("doctor");
        assert!(
            doctor.findings[0].starts_with(named),
            "{:?}",
            doctor.findings
        );
    }
}

#[tokio::test]
async fn a_healthy_answer_is_reused_and_a_failed_one_is_not() {
    let (provider, state) = hosted().await;
    provider.diagnose().await.expect("first");
    provider.degraded_state().await.expect("second");
    provider.doctor().await.expect("third");
    assert_eq!(probes(&state), 1, "a healthy answer is reused");

    let (provider, state) = hosted().await;
    *state.fail_all.lock().expect("fail") = Some((503, "UNAVAILABLE"));
    provider.diagnose().await.expect("first");
    let after_one = probes(&state);
    provider.diagnose().await.expect("second");
    assert!(probes(&state) > after_one, "a failed answer is asked again");
}

#[tokio::test]
async fn a_healthy_answer_expires() {
    let (provider, state) = hosted().await;
    let provider = provider.with_families(FamilyState::with(
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
    ));
    provider.diagnose().await.expect("first");
    provider.diagnose().await.expect("second");
    assert_eq!(probes(&state), 2);
}

#[test]
fn a_failure_detail_is_bounded() {
    let error = anyhow::anyhow!("{}", "x".repeat(2_000));
    let failure = ProbeFailure::of(&error);
    assert_eq!(failure.detail.chars().count(), 300);
    assert_eq!(failure.code, "transient");
}
