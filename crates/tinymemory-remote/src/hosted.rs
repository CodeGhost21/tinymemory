//! The TinyHumans backend's envelope and error dialect for hosted CortexDB.
//!
//! The backend fronts CortexDB behind `/memory/*` and wraps every response:
//!
//! - success: `200 {"success": true, "data": <CortexDB body verbatim>}`
//! - failure: `{"success": false, "error": "<message>", "errorCode": "<CODE>"}`
//!
//! Failures are mapped onto the existing [`MemoryError`] taxonomy rather than a
//! parallel error type, so `engine_error`, the retry policy and the bus wire
//! names keep working unchanged:
//!
//! | HTTP | `errorCode` (typical) | [`MemoryError`] |
//! | --- | --- | --- |
//! | 401 / 403 | `UNAUTHORIZED` | `Unauthorized` (session expired or key rejected) |
//! | 402 | `USER_INSUFFICIENT_CREDITS` | `BudgetExceeded` |
//! | 429, 502, 503, 504 | `RATE_LIMITED`, ... | `Unavailable` (retried on reads) |
//! | 400, 409, 413, 422 | `VALIDATION_ERROR`, `CONFLICT` | `Invalid` |
//! | 404 | | `NotFound` |
//!
//! The backend's `errorCode` is carried in the message as a `[CODE]` prefix,
//! and [`error_code`] reads it back; [`is_insufficient_credits`] is the check a
//! host needs to show a "top up" prompt.

use reqwest::{StatusCode, Url};
use serde_json::Value;
use tinymemory_api::error::MemoryError;

/// Default origin of the TinyHumans backend that hosts CortexDB.
pub const TINYHUMANS_API_ENDPOINT: &str = "https://api.tinyhumans.ai";

/// The backend's code for an exhausted credit balance (HTTP 402).
pub const INSUFFICIENT_CREDITS_CODE: &str = "USER_INSUFFICIENT_CREDITS";

/// Longest error message kept from a backend body.
const MAX_MESSAGE_CHARS: usize = 300;

/// Sanitises a backend code so the `[CODE]` prefix stays parseable.
fn clean_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn message_of(error: &MemoryError) -> Option<&str> {
    match error {
        MemoryError::Unauthorized(m)
        | MemoryError::BudgetExceeded(m)
        | MemoryError::Unavailable(m)
        | MemoryError::Invalid(m)
        | MemoryError::NotFound(m)
        | MemoryError::Backend(m) => Some(m),
        _ => None,
    }
}

/// The TinyHumans `errorCode` a hosted failure carried, when it has one.
///
/// `MemoryError` has no field for it, so hosted failures carry the code as a
/// `[CODE] ` prefix on their message and this function parses that prefix back
/// out. It returns `None` for any error that did not come from the hosted
/// dialect, and for a message that was rewritten so it no longer starts with
/// the prefix.
#[must_use]
pub fn error_code(error: &MemoryError) -> Option<&str> {
    let message = message_of(error)?;
    let rest = message.strip_prefix('[')?;
    let (code, _) = rest.split_once("] ")?;
    (!code.is_empty()).then_some(code)
}

/// Whether `error` is the hosted backend's "not enough credits" refusal.
#[must_use]
pub fn is_insufficient_credits(error: &MemoryError) -> bool {
    matches!(error, MemoryError::BudgetExceeded(_))
        && error_code(error) == Some(INSUFFICIENT_CREDITS_CODE)
}

/// Maps a failure (`success:false` body or non-2xx status) to a typed error.
fn typed(host: &str, path: &str, status: StatusCode, code: &str, message: &str) -> anyhow::Error {
    let mut shown: String = message.trim().chars().take(MAX_MESSAGE_CHARS).collect();
    if message.trim().chars().count() > MAX_MESSAGE_CHARS {
        shown.push('…');
    }
    let tagged = format!("[{code}] memory API {path} on {host} (HTTP {status}): {shown}");
    anyhow::Error::new(match status.as_u16() {
        401 | 403 => MemoryError::Unauthorized(format!(
            "{tagged} — the session expired or the API key was rejected; re-authenticate"
        )),
        402 => MemoryError::BudgetExceeded(format!("{tagged} — insufficient credits")),
        404 => MemoryError::NotFound(tagged),
        400 | 409 | 413 | 422 => MemoryError::Invalid(tagged),
        // 500 is retryable on reads too: the hosted proxy answers it for a
        // transient upstream fault.
        429 | 500 | 502 | 503 | 504 => MemoryError::Unavailable(tagged),
        _ => MemoryError::Backend(tagged),
    })
}

/// Builds the error for a non-success response.
pub(crate) fn status_error(
    host: &str,
    path: &str,
    status: StatusCode,
    body: &str,
) -> anyhow::Error {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("errorCode"))
        .and_then(Value::as_str)
        .map(clean_code)
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| default_code(status));
    let message = parsed.as_ref().and_then(|v| v.get("error")).map_or_else(
        || body.to_string(),
        |e| e.as_str().map_or_else(|| e.to_string(), str::to_string),
    );
    typed(host, path, status, &code, &message)
}

fn default_code(status: StatusCode) -> String {
    match status.as_u16() {
        401 | 403 => "UNAUTHORIZED".to_string(),
        402 => INSUFFICIENT_CREDITS_CODE.to_string(),
        429 => "RATE_LIMITED".to_string(),
        other => format!("HTTP_{other}"),
    }
}

/// Unwraps `{success:true,data}`; `{success:false,...}`, a missing `data` and a
/// bare body are all errors.
pub(crate) fn unwrap_envelope(
    endpoint: &Url,
    path: &str,
    status: StatusCode,
    body: &[u8],
) -> anyhow::Result<Value> {
    let host = endpoint.host_str().unwrap_or("<endpoint>");
    let mut value = parse_envelope(endpoint, path, status, body)?;
    match value.get_mut("data") {
        Some(data) => Ok(data.take()),
        None => Err(anyhow::Error::new(MemoryError::Backend(format!(
            "memory API {path} on {host} answered success without a `data` field"
        )))),
    }
}

/// Checks only that a 2xx body is a `{success:true}` envelope, for calls whose
/// response body is not needed.
pub(crate) fn check_envelope(
    endpoint: &Url,
    path: &str,
    status: StatusCode,
    body: &[u8],
) -> anyhow::Result<()> {
    parse_envelope(endpoint, path, status, body).map(|_| ())
}

fn parse_envelope(
    endpoint: &Url,
    path: &str,
    status: StatusCode,
    body: &[u8],
) -> anyhow::Result<Value> {
    let host = endpoint.host_str().unwrap_or("<endpoint>");
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| anyhow::anyhow!("memory API {path} returned invalid JSON"))?;
    match value.get("success").and_then(Value::as_bool) {
        Some(true) => Ok(value),
        Some(false) => Err(status_error(host, path, status, &value.to_string())),
        None => Err(anyhow::Error::new(MemoryError::Backend(format!(
            "memory API {path} on {host} answered without the success envelope"
        )))),
    }
}
