//! Removing named events.
//!
//! The selector's id field is `memory_ids`, and it is never empty: CortexDB
//! reads an unrecognised or empty selector as "the whole scope". Two
//! interlocks stop that being destructive on its own (an empty selector
//! needs `confirm_all`, and `confirm_all` with a selector is refused), and
//! this crate never writes `confirm_all` at all, so the two mistakes cannot
//! meet.

use serde_json::json;

use super::Log;
use crate::cortex::descriptor::{CortexWire, Route};
use crate::cortex::error::{Error, Result};
use crate::cortex::transport::Attempts;

/// The most event ids one removal names, so each body stays small.
pub(crate) const FORGET_BATCH: usize = 100;

/// How many times a hosted removal is sent before a transient fault surfaces.
const HOSTED_FORGET_ATTEMPTS: u32 = 3;

impl Log {
    /// Removes `ids` from `scope`, at most [`FORGET_BATCH`] per request. An
    /// empty `ids` sends nothing.
    pub(crate) async fn forget_events(&self, scope: &str, ids: &[String]) -> Result<()> {
        for batch in ids.chunks(FORGET_BATCH) {
            self.forget_batch(scope, batch).await?;
        }
        Ok(())
    }

    /// One removal. Hosted retries a transient fault: removing named events
    /// is idempotent, and a retry that finds them gone (404) has done its
    /// job. Direct sends it once.
    async fn forget_batch(&self, scope: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let body = json!({
            "scope": scope,
            "layers": ["events"],
            "selector": { "memory_ids": ids },
            "audit_note": "tinymemory: forget",
        });
        let path = self.client.wire().path(Route::Forget);
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .client
                .json(reqwest::Method::POST, path, Some(&body), Attempts::Once)
                .await
            {
                Ok(_) => return Ok(()),
                Err(Error::NotFound(_)) if attempt > 1 => return Ok(()),
                Err(error)
                    if self.client.wire() == CortexWire::TinyHumans
                        && attempt < HOSTED_FORGET_ATTEMPTS
                        && error.is_transient() =>
                {
                    tokio::time::sleep(self.timing.poll * 2_u32.pow(attempt - 1)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}
