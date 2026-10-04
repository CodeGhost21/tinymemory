//! Consolidate: CortexDB's on-demand belief build.
//!
//! **Direct.** CortexDB builds a scope's beliefs at `v1/beliefs/build`, one
//! scope per request (`{"scope": "<path>"}`), and answers once the build is
//! queued. A [`ConsolidateRequest`] covers a reach and some kinds, so the
//! engine first finds the kind scopes in reach that CortexDB actually holds
//! (`v1/scopes/list`, see `scopes::held`) and asks for each, in order. The
//! receipt is [`ConsolidateStatus::Started`] with whatever job handles the
//! answers carried. What gets built surfaces through recall's derived
//! layers (`facts`, `beliefs`), which fetch and recall already read.
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
    JOB_FIELDS.iter().find_map(|field| match answer.get(field)? {
        Value::String(id) if !id.is_empty() => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    })
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn build_beliefs(&self, req: ConsolidateRequest) -> Result<ConsolidateReceipt> {
        req.validate()?;
        let wire = self.wire();
        if wire == CortexWire::TinyHumans {
            return Ok(ConsolidateReceipt::scheduled());
        }
        let scopes = self.held(&req.reach, &req.admitted_kinds()).await?;
        let mut jobs = Vec::new();
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
            jobs.extend(job_id(&answer));
        }
        log::debug!(
            "[cortex] beliefs build requested reach={} scopes={} jobs={}",
            req.reach.at,
            scopes.len(),
            jobs.len()
        );
        Ok(ConsolidateReceipt {
            status: ConsolidateStatus::Started,
            jobs,
            scopes: scopes.len(),
        })
    }
}

#[cfg(test)]
#[path = "consolidate_tests.rs"]
mod tests;
