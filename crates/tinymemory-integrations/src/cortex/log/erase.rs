//! Erasing a whole scope: `POST v1/erasures` with `confirm_all`.
//!
//! The one place this crate sends `confirm_all`, and never with a selector:
//! the request names exactly one scope, so it erases that scope's events
//! (deleted, not redacted, their write keys released). CortexDB only
//! redacts the scopes below it, which keep their keys; `engine::erase`
//! names kind scopes only (leaves), deepest first, so that never happens.
//!
//! Both wires erase. Direct posts to CortexDB's own `v1/erasures`; hosted
//! posts to the TinyHumans backend's `memory/v1/erasures` passthrough,
//! which memory-api pins under the caller's tenant root and sends with the
//! tenant's own user-actor token. The passthrough answers in CortexDB's
//! dialect (no `{success,data}` envelope; see `transport`).
//!
//! An erasure is a job. CortexDB answers `completed` when it ran to the
//! end before answering; a `running` answer (memory-api may hand the job
//! back before it finishes) is polled at `v1/erasures/{id}` until it
//! settles, and anything but `completed` is an error: an erasure that did
//! not finish must never read as done.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{HOSTED_POLL_CEILING, Log};
use crate::cortex::descriptor::Route;
use crate::cortex::error::{Error, Result};
use crate::cortex::transport::{Attempts, urlencode};

/// How long a running erasure is polled before it is reported as not done.
const ERASURE_TIMEOUT: Duration = Duration::from_secs(300);

/// The one status that means the scope is gone.
const COMPLETED: &str = "completed";

/// Statuses of an erasure that has not settled yet.
const PENDING: [&str; 4] = ["running", "pending", "queued", "accepted"];

impl Log {
    /// Erases `scope`, returning CortexDB's erasure id once the erasure has
    /// completed. Sending it again erases what is there then, so the POST
    /// is sent once and a failure surfaces; the status poll is a read and
    /// retries transient faults.
    pub(crate) async fn erase(&self, scope: &str) -> Result<String> {
        let body = json!({
            "scope": scope,
            "confirm_all": true,
            "audit_note": "tinymemory: erase",
        });
        let path = self.client.wire().path(Route::Erasures);
        let answer = self
            .client
            .json(reqwest::Method::POST, path, Some(&body), Attempts::Once)
            .await?;
        let id = answer
            .get("erasure_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::Engine(format!("the erasure of {scope} answered no erasure_id"))
            })?;
        let mut status = status_of(&answer);
        let started = Instant::now();
        let mut gap = self.timing.poll;
        while PENDING.contains(&status.as_str()) {
            if started.elapsed() > ERASURE_TIMEOUT {
                return Err(Error::Unavailable(format!(
                    "the erasure of {scope} ({id}) was still {status} after {}s",
                    ERASURE_TIMEOUT.as_secs()
                )));
            }
            tokio::time::sleep(gap).await;
            gap = (gap * 2).min(HOSTED_POLL_CEILING);
            let job = self
                .client
                .json(
                    reqwest::Method::GET,
                    &format!("{path}/{}", urlencode(&id)),
                    None,
                    Attempts::RetryTransient,
                )
                .await?;
            status = status_of(&job);
            log::debug!("[cortex] erasure {id} of {scope} is {status}");
        }
        if status != COMPLETED {
            return Err(Error::Engine(format!(
                "the erasure of {scope} ({id}) ended {status}, not {COMPLETED}"
            )));
        }
        Ok(id)
    }
}

/// An erasure answer's status. CortexDB's synchronous answer may omit it;
/// an id with no status is an erasure that ran to completion.
fn status_of(answer: &Value) -> String {
    answer
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(COMPLETED)
        .to_string()
}
