//! Background work: what a host runs off the live turn, and when it likes.
//!
//! Lifecycle calls never block on slow work. Instead they hand back
//! [`BackgroundJob`]s — plain, serializable values a host can queue, persist,
//! compare to drop duplicates, and run later on whatever executor it owns.
//! This crate spawns nothing.
//!
//! - [`BackgroundJob::BuildBeliefs`] asks the engine to consolidate a scope
//!   ([`tinymemory_api::MemoryEngine::consolidate`]).
//! - [`BackgroundJob::IngestBrain`] stores brain documents the host chose
//!   not to ingest inline; running it yields the belief builds that follow.
//!
//! [`BackgroundRunner::run`] executes one job. An engine that does not
//! consolidate turns a build into [`JobOutcome::Skipped`], not an error, so
//! the same host code runs against any engine.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tinymemory_api::{
    ConsolidateReceipt, ConsolidateRequest, ConsolidateStatus, Error, MemoryEngine, Result,
    StoreReceipt,
};

use tinymemory_api::Consolidation;

use crate::brain::{Brain, BrainDocument};
use crate::layout::MemoryLayout;

/// Whether `engine` rebuilds beliefs on its own after writes
/// ([`Consolidation::Automatic`]), so the lifecycle and the brain hand back
/// no belief build after a turn or an ingest.
pub(crate) fn builds_on_its_own(engine: &dyn MemoryEngine) -> bool {
    engine.descriptor().consolidation == Consolidation::Automatic
}

/// One unit of deferred work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "job", rename_all = "snake_case")]
pub enum BackgroundJob {
    /// Build beliefs from a scope.
    BuildBeliefs {
        /// What to consolidate.
        request: ConsolidateRequest,
    },
    /// Store brain documents.
    IngestBrain {
        /// The documents, in order.
        documents: Vec<BrainDocument>,
    },
}

impl BackgroundJob {
    /// The job's name: `build_beliefs` or `ingest_brain`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::BuildBeliefs { .. } => "build_beliefs",
            Self::IngestBrain { .. } => "ingest_brain",
        }
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum JobOutcome {
    /// The work is done.
    Done,
    /// The engine took the work and is doing it in the background.
    Started,
    /// The engine does this on its own schedule; nothing was started.
    Scheduled,
    /// The engine cannot do this; nothing happened.
    Skipped {
        /// Why.
        reason: String,
    },
}

/// What running one job did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobReport {
    /// The job's name ([`BackgroundJob::name`]).
    pub job: &'static str,
    /// How it ended.
    pub outcome: JobOutcome,
    /// The engine's consolidation receipt, for a build it accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation: Option<ConsolidateReceipt>,
    /// Receipts of the documents an ingest stored.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stored: Vec<StoreReceipt>,
    /// Jobs this one gave rise to (an ingest's belief builds).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<BackgroundJob>,
}

/// Runs [`BackgroundJob`]s against one engine and layout. Cheap to clone.
#[derive(Clone)]
pub struct BackgroundRunner {
    engine: Arc<dyn MemoryEngine>,
    layout: MemoryLayout,
}

impl std::fmt::Debug for BackgroundRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundRunner")
            .field("engine", &self.engine.descriptor().id)
            .finish_non_exhaustive()
    }
}

impl BackgroundRunner {
    /// A runner for `layout` on `engine`.
    #[must_use]
    pub fn new(engine: Arc<dyn MemoryEngine>, layout: MemoryLayout) -> Self {
        Self { engine, layout }
    }

    /// Runs `job` to the point the engine takes it.
    ///
    /// # Errors
    ///
    /// Invalid jobs, and the engine's failures other than
    /// [`Error::Unsupported`] (which is [`JobOutcome::Skipped`]).
    pub async fn run(&self, job: BackgroundJob) -> Result<JobReport> {
        let name = job.name();
        match job {
            BackgroundJob::BuildBeliefs { request } => self.build(name, request).await,
            BackgroundJob::IngestBrain { documents } => {
                let batch = Brain::new(self.engine.clone(), self.layout.clone())
                    .ingest_many(documents)
                    .await?;
                Ok(JobReport {
                    job: name,
                    outcome: JobOutcome::Done,
                    consolidation: None,
                    stored: batch.receipts,
                    follow_ups: batch.jobs,
                })
            }
        }
    }

    async fn build(&self, name: &'static str, request: ConsolidateRequest) -> Result<JobReport> {
        let report = |outcome, consolidation| JobReport {
            job: name,
            outcome,
            consolidation,
            stored: Vec::new(),
            follow_ups: Vec::new(),
        };
        match self.engine.consolidate(request).await {
            Ok(receipt) => {
                let outcome = match receipt.status {
                    ConsolidateStatus::Started => JobOutcome::Started,
                    ConsolidateStatus::Scheduled => JobOutcome::Scheduled,
                    ConsolidateStatus::Completed => JobOutcome::Done,
                };
                Ok(report(outcome, Some(receipt)))
            }
            Err(Error::Unsupported(reason)) => {
                log::debug!("[background] belief build skipped reason={reason}");
                Ok(report(JobOutcome::Skipped { reason }, None))
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
