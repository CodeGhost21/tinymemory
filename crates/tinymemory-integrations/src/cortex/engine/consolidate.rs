//! Consolidate: CortexDB's on-demand belief build.
//!
//! **Direct.** CortexDB builds a scope's beliefs at `v1/beliefs/build`, one
//! scope per request (`{"scope": "<path>"}`). A [`ConsolidateRequest`] covers
//! a reach and some kinds, so the engine first finds the kind scopes in reach
//! that CortexDB actually holds (`v1/scopes/list`, see `scopes::held`) and
//! asks for each, in order.
//!
//! CortexDB v0.10 builds within the request, from the facts it has already
//! extracted, and answers with the count (`{"built": 2, "items": [...]}`):
//! the receipt is then [`ConsolidateStatus::Completed`] with the summed
//! count. An answer that names a job instead (`job_id`, `build_id` or `id`)
//! means the build was queued, and the receipt is
//! [`ConsolidateStatus::Started`] with the handles. What gets built lands in
//! recall's derived layers (`facts`, `beliefs`); the answer route reads them,
//! fetch does not (it ranks stored items only).
//!
//! Every build is sent once: a build is not idempotent work to repeat on a
//! timeout, and the host can always ask again. A failure part way leaves the
//! earlier scopes' builds queued.
//!
//! **TinyHumans.** The backend has no build route and CortexDB consolidates
//! behind it on its own schedule, so the receipt is
//! [`ConsolidateStatus::Scheduled`] and nothing is sent.

use reqwest::Method;
use serde_json::{Value, json};
use tinymemory_api::{ConsolidateReceipt, ConsolidateRequest, ConsolidateStatus};

use super::CortexEngine;
use crate::cortex::descriptor::{CortexWire, Route};
use crate::cortex::error::Result;
use crate::cortex::transport::Attempts;

/// The fields a build answer may name its job by, in preference order.
const JOB_FIELDS: [&str; 3] = ["job_id", "build_id", "id"];

/// The job handle a build answer carries, if any.
pub(super) fn job_id(answer: &Value) -> Option<String> {
    JOB_FIELDS
        .iter()
        .find_map(|field| match answer.get(field)? {
            Value::String(id) if !id.is_empty() => Some(id.clone()),
            Value::Number(id) => Some(id.to_string()),
            _ => None,
        })
}

/// The receipt for the answers of builds over `scopes` scopes: completed
/// when every answer reports what it built, started when any was queued.
pub(super) fn receipt(answers: &[Value], scopes: usize) -> ConsolidateReceipt {
    let jobs: Vec<String> = answers.iter().filter_map(job_id).collect();
    let counts: Vec<usize> = answers
        .iter()
        .filter_map(|answer| answer.get("built")?.as_u64())
        .filter_map(|built| usize::try_from(built).ok())
        .collect();
    let completed = jobs.is_empty() && counts.len() == answers.len();
    ConsolidateReceipt {
        status: if completed {
            ConsolidateStatus::Completed
        } else {
            ConsolidateStatus::Started
        },
        jobs,
        scopes,
        built: completed.then(|| counts.iter().sum()),
    }
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn build_beliefs(
        &self,
        req: ConsolidateRequest,
    ) -> Result<ConsolidateReceipt> {
        req.validate()?;
        let wire = self.wire();
        if wire == CortexWire::TinyHumans {
            return Ok(ConsolidateReceipt::scheduled());
        }
        let scopes = self.held(&req.reach, &req.admitted_kinds()).await?;
        let mut answers = Vec::new();
        for scope in &scopes {
            let body = json!({ "scope": scope.path });
            let answer = self
                .log
                .client
                .json(
                    Method::POST,
                    wire.path(Route::BuildBeliefs),
                    Some(&body),
                    Attempts::Once,
                )
                .await?;
            answers.push(answer);
        }
        Ok(receipt(&answers, scopes.len()))
    }
}

#[cfg(test)]
#[path = "consolidate_tests.rs"]
mod tests;
