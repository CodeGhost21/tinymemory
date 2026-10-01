//! `MemoryMaintenance` over the hosted wire.
//!
//! The hosted service runs its own upkeep: it embeds, derives and compacts on
//! its side, and exposes none of it as an operation a client can drive. So this
//! family reports rather than works. Upkeep answers an empty report saying so,
//! and the health reads come from one probe — the adapter's existing health
//! call — classified into the host health vocabulary and cached, because a
//! status panel asks often and every hosted call is billed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::diagnosis::{
    DegradedCapabilities, Diagnosis, DiagnosisCounters, DiagnosisFailure, DiagnosisStage,
};
use tinymemory_api::provider::types::MaintenanceReport;
use tinymemory_api::provider::MemoryMaintenance;

use crate::common::Dialect;
use crate::cortex_provider::CortexProvider;

/// How long a healthy probe answer is reused.
pub(super) const HEALTHY_FOR: Duration = Duration::from_secs(60);

/// How long a failed probe answer is reused: briefly, so a recovery shows soon.
pub(super) const FAILED_FOR: Duration = Duration::from_secs(5);

/// The longest error text a failure carries into a diagnosis.
const DETAIL_CHARS: usize = 300;

/// The stage id this driver diagnoses.
const SERVICE_STAGE: &str = "service";

/// Why the probe failed, in the host health vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProbeFailure {
    /// The failure code: `auth_invalid`, `budget_exhausted`,
    /// `storage_unavailable` or `transient`.
    pub(super) code: &'static str,
    /// Whether waiting may fix it.
    pub(super) transient: bool,
    /// The error, bounded. It names neither a credential nor a namespace: the
    /// probe lists a scope the adapter never writes, and the transport never
    /// puts a credential in an error.
    pub(super) detail: String,
}

impl ProbeFailure {
    /// Classifies a failed probe.
    pub(super) fn of(error: &anyhow::Error) -> Self {
        let (code, transient) = match error.downcast_ref::<MemoryError>() {
            Some(MemoryError::Unauthorized(_)) => ("auth_invalid", false),
            Some(MemoryError::BudgetExceeded(_)) => ("budget_exhausted", false),
            Some(
                MemoryError::Unavailable(_) | MemoryError::Unreachable(_) | MemoryError::Timeout(_),
            ) => ("storage_unavailable", true),
            _ => ("transient", true),
        };
        Self {
            code,
            transient,
            detail: format!("{error:#}").chars().take(DETAIL_CHARS).collect(),
        }
    }

    fn as_diagnosis(&self) -> DiagnosisFailure {
        DiagnosisFailure {
            code: self.code.to_string(),
            class: Some(
                if self.transient {
                    "transient"
                } else {
                    "unrecoverable"
                }
                .to_string(),
            ),
            remediation_key: format!("memory.health.remediation.{}", self.code),
            detail: Some(self.detail.clone()),
        }
    }
}

/// The last probe answer, and how long each kind is reused.
#[derive(Debug)]
pub(crate) struct ProbeCache {
    healthy_for: Duration,
    failed_for: Duration,
    last: Mutex<Option<(Instant, Option<ProbeFailure>)>>,
}

impl ProbeCache {
    pub(crate) fn new(healthy_for: Duration, failed_for: Duration) -> Self {
        Self {
            healthy_for,
            failed_for,
            last: Mutex::new(None),
        }
    }

    fn fresh(&self) -> Option<Option<ProbeFailure>> {
        let last = self.last.lock().ok()?;
        let (at, reading) = last.as_ref()?;
        let reuse_for = if reading.is_some() {
            self.failed_for
        } else {
            self.healthy_for
        };
        (at.elapsed() < reuse_for).then(|| reading.clone())
    }

    fn keep(&self, reading: Option<ProbeFailure>) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some((Instant::now(), reading));
        }
    }
}

/// An upkeep report for work the hosted service does itself.
fn done_by_the_service(operation: &str) -> MaintenanceReport {
    MaintenanceReport {
        operation: operation.to_string(),
        examined: 0,
        changed: 0,
        findings: vec![format!(
            "the hosted memory service runs {operation} itself; nothing to do here"
        )],
    }
}

/// The degradation a probe answer means: a service that did not answer is
/// storage out of reach.
fn degraded(reading: Option<&ProbeFailure>) -> DegradedCapabilities {
    match reading {
        None => DegradedCapabilities::default(),
        Some(failure) => DegradedCapabilities {
            storage: true,
            cause: Some(failure.as_diagnosis()),
            ..DegradedCapabilities::default()
        },
    }
}

impl CortexProvider {
    /// The probe's answer, from the cache when it is fresh: `None` when the
    /// service answered.
    async fn probe(&self) -> Option<ProbeFailure> {
        if let Some(reading) = self.families.probe.fresh() {
            return reading;
        }
        let reading = self
            .dialect
            .health()
            .await
            .err()
            .map(|error| ProbeFailure::of(&error));
        self.families.probe.keep(reading.clone());
        reading
    }
}

#[async_trait]
impl MemoryMaintenance for CortexProvider {
    async fn reembed(&self) -> Result<MaintenanceReport, MemoryError> {
        Ok(done_by_the_service("reembed"))
    }

    async fn compact(&self) -> Result<MaintenanceReport, MemoryError> {
        Ok(done_by_the_service("compact"))
    }

    async fn consolidate(&self) -> Result<MaintenanceReport, MemoryError> {
        Ok(done_by_the_service("consolidate"))
    }

    async fn doctor(&self) -> Result<MaintenanceReport, MemoryError> {
        let reading = self.probe().await;
        Ok(MaintenanceReport {
            operation: "doctor".to_string(),
            examined: 1,
            changed: 0,
            findings: reading
                .map(|failure| vec![format!("{}: {}", failure.code, failure.detail)])
                .unwrap_or_default(),
        })
    }

    async fn diagnose(&self) -> Result<Diagnosis, MemoryError> {
        let reading = self.probe().await;
        let failure = reading.as_ref().map(ProbeFailure::as_diagnosis);
        let ok = reading.is_none();
        Ok(Diagnosis {
            healthy: ok,
            stages: vec![DiagnosisStage {
                stage: SERVICE_STAGE.to_string(),
                ok,
                failure: failure.clone(),
                note: if ok {
                    "the hosted memory service answered"
                } else {
                    "the hosted memory service did not answer"
                }
                .to_string(),
            }],
            first_blocking_cause: failure,
            degraded: degraded(reading.as_ref()),
            counters: DiagnosisCounters::default(),
        })
    }

    async fn degraded_state(&self) -> Result<DegradedCapabilities, MemoryError> {
        Ok(degraded(self.probe().await.as_ref()))
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod test;
