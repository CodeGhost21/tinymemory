//! Erasing a whole scope: `POST v1/erasures` with `confirm_all`.
//!
//! The one place this crate sends `confirm_all`, and never with a selector:
//! the request names exactly one scope, so it erases that scope's events
//! (deleted, not redacted, their write keys released). CortexDB only
//! redacts the scopes below it, which keep their keys; `engine::erase`
//! names kind scopes only (leaves), deepest first, so that never happens.

use serde_json::{Value, json};

use super::Log;
use crate::cortex::descriptor::Route;
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
}
