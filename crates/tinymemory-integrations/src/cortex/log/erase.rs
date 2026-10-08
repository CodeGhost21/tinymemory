//! Erasing a whole scope: `POST v1/erasures` with `confirm_all`; and, on
//! the TinyHumans wire, the caller's entire memory: `DELETE memory`.
//!
//! The one place this crate sends `confirm_all`, and never with a selector:
//! the request names exactly one scope, so it erases that scope's events
//! (deleted, not redacted, their write keys released). CortexDB only
//! redacts the scopes below it, which keep their keys; `engine::erase`
//! names kind scopes only (leaves), deepest first, so that never happens.
//!
//! Both wires erase a scope. Direct posts `{scope, confirm_all}` to
//! CortexDB's own `v1/erasures`; hosted posts `{scope, audit_note}` to the
//! TinyHumans backend's `memory/v1/erasures` passthrough (memory-api's
//! scoped erasure, which takes no `confirm_all` and refuses an unknown
//! field). memory-api pins the scope under the caller's tenant root, erases
//! it and every scope below it with the tenant's own user-actor token, and
//! answers synchronously in its own dialect (no `{success,data}` envelope;
//! see `transport`): `{erased: true, scopes, erasure_ids}`, where `scopes:
//! 0` (nothing stored) is still success. `502 ERASURE_INCOMPLETE` is
//! retriable and retried; re-erasing a scope is safe. A backend without the
//! route answers 404, reported as [`Error::Unsupported`]: nothing was erased.
//!
//! A Direct erasure is a job. CortexDB answers `completed` when it ran to
//! the end before answering; a `running` answer is polled at
//! `v1/erasures/{id}` until it settles, and anything but `completed` is an
//! error: an erasure that did not finish must never read as done.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{HOSTED_POLL_CEILING, Log};
use crate::cortex::descriptor::{CortexWire, Route};
use crate::cortex::error::{Error, Result};
use crate::cortex::transport::{Attempts, urlencode};

/// How long a running erasure is polled before it is reported as not done.
const ERASURE_TIMEOUT: Duration = Duration::from_secs(300);

/// How many times a hosted erasure is sent before an incomplete or
/// transient failure surfaces.
const HOSTED_ERASE_ATTEMPTS: u32 = 3;

/// The one status that means the scope is gone.
const COMPLETED: &str = "completed";

/// Statuses of an erasure that has not settled yet.
const PENDING: [&str; 4] = ["running", "pending", "queued", "accepted"];

/// The audit note every erasure carries.
const AUDIT_NOTE: &str = "tinymemory: erase";

impl Log {
    /// Erases `scope`, returning the erasure ids once it has completed
    /// (none for a hosted scope that held nothing).
    pub(crate) async fn erase(&self, scope: &str) -> Result<Vec<String>> {
        match self.client.wire() {
            CortexWire::Direct => self.erase_direct(scope).await.map(|id| vec![id]),
            CortexWire::TinyHumans => self.erase_hosted(scope).await,
        }
    }

    /// Hosted: memory-api's synchronous scoped erasure. Retried on a
    /// transient failure (`502 ERASURE_INCOMPLETE` among them), since
    /// erasing a scope again erases only what is left.
    async fn erase_hosted(&self, scope: &str) -> Result<Vec<String>> {
        let body = json!({ "scope": scope, "audit_note": AUDIT_NOTE });
        let path = self.client.wire().path(Route::Erasures);
        let mut attempt = 0;
        let answer = loop {
            attempt += 1;
            match self
                .client
                .json(reqwest::Method::POST, path, Some(&body), Attempts::Once)
                .await
            {
                Ok(answer) => break answer,
                Err(Error::NotFound(message)) => {
                    return Err(Error::Unsupported(format!(
                        "the TinyHumans backend has no scoped erasure route ({message})"
                    )));
                }
                Err(error) if error.is_transient() && attempt < HOSTED_ERASE_ATTEMPTS => {
                    log::debug!("[cortex] erasure of {scope} incomplete; retrying ({attempt})");
                    tokio::time::sleep(self.timing.poll * 2_u32.pow(attempt - 1)).await;
                }
                Err(error) => return Err(error),
            }
        };
        if answer.get("erased").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Engine(format!(
                "the erasure of {scope} answered without `erased: true`"
            )));
        }
        let ids = answer
            .get("erasure_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(ids)
    }

    /// Direct: CortexDB's `v1/erasures`, sent once (a failure surfaces),
    /// then polled while it runs.
    async fn erase_direct(&self, scope: &str) -> Result<String> {
        let body = json!({
            "scope": scope,
            "confirm_all": true,
            "audit_note": AUDIT_NOTE,
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

    /// Erases the caller's entire hosted memory (`DELETE memory`), every
    /// scope under its tenant whoever wrote it, returning how many scopes the
    /// backend erased. TinyHumans only. Sent once, like a scope erasure.
    ///
    /// A backend without the route answers 404, which is reported as
    /// [`Error::Unsupported`]: nothing was erased.
    pub(crate) async fn erase_all(&self) -> Result<usize> {
        if self.client.wire() != CortexWire::TinyHumans {
            return Err(Error::Unsupported(
                "only the TinyHumans backend erases a whole memory in one request".to_string(),
            ));
        }
        let answer = self
            .client
            .json(
                reqwest::Method::DELETE,
                self.client.wire().path(Route::EraseAll),
                None,
                Attempts::Once,
            )
            .await
            .map_err(|error| match error {
                Error::NotFound(message) => Error::Unsupported(format!(
                    "the TinyHumans backend has no whole-memory erase route ({message})"
                )),
                other => other,
            })?;
        if answer.get("erased").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Engine(
                "the memory erasure answered without `erased: true`".to_string(),
            ));
        }
        // A count the backend did not send (or sent as anything but a
        // non-negative integer) is a malformed answer, not zero scopes.
        let scopes = answer
            .get("scopes")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                Error::Engine(
                    "the memory erasure answered without a numeric `scopes` count".to_string(),
                )
            })?;
        Ok(usize::try_from(scopes).unwrap_or(usize::MAX))
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
