//! Erasing a whole scope: `POST v1/erasures` with `confirm_all`; and, on
//! the TinyHumans wire, the caller's entire memory: `DELETE memory`.
//!
//! The one place this crate sends `confirm_all`, and never with a selector:
//! the request names exactly one scope, so it erases that scope's events
//! (deleted, not redacted, their write keys released). CortexDB only
//! redacts the scopes below it, which keep their keys; `engine::erase`
//! names kind scopes only (leaves), deepest first, so that never happens.

use serde_json::{Value, json};

use super::Log;
use crate::cortex::descriptor::{CortexWire, Route};
use crate::cortex::error::{Error, Result};
use crate::cortex::transport::Attempts;

impl Log {
    /// Erases `scope`, returning CortexDB's erasure id. The execute runs to
    /// completion before it answers; sending it again erases what is there
    /// then, so it is sent once and a failure surfaces.
    pub(crate) async fn erase(&self, scope: &str) -> Result<String> {
        let body = json!({
            "scope": scope,
            "confirm_all": true,
            "audit_note": "tinymemory: erase",
        });
        let answer = self
            .client
            .json(
                reqwest::Method::POST,
                self.client.wire().path(Route::Erasures),
                Some(&body),
                Attempts::Once,
            )
            .await?;
        answer
            .get("erasure_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Engine(format!("the erasure of {scope} answered no erasure_id")))
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
        let scopes = answer.get("scopes").and_then(Value::as_u64).unwrap_or(0);
        Ok(usize::try_from(scopes).unwrap_or(usize::MAX))
    }
}
