//! Reading response bodies against a byte cap.
//!
//! The endpoint is operator-supplied, so a broken or hostile server must not
//! be able to exhaust the host's memory. `Response::json()` and `text()`
//! buffer the whole body before any check, which a server that omits or
//! understates `Content-Length` defeats, so bodies are read chunk by chunk.

use futures::StreamExt;

use crate::error::{Error, Result};

/// Largest success body accepted. Far above any real page of events, far
/// below a size that threatens a process.
pub(crate) const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Largest error body read. Only a short excerpt of it is ever shown, so
/// 64 KiB keeps every real message while denying an endless one.
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Reads a success body, failing once it would exceed [`MAX_RESPONSE_BYTES`].
pub(crate) async fn read_capped(response: reqwest::Response, label: &str) -> Result<Vec<u8>> {
    read_limited(response, label, MAX_RESPONSE_BYTES).await
}

/// [`read_capped`] with the limit as an argument, so the cap is testable
/// without a 64 MiB body.
pub(crate) async fn read_limited(
    response: reqwest::Response,
    label: &str,
    limit: usize,
) -> Result<Vec<u8>> {
    if let Some(len) = response.content_length()
        && len > limit as u64
    {
        return Err(Error::Engine(format!(
            "memory API {label} response exceeds the {limit}-byte limit (Content-Length {len})"
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            Error::Unavailable(format!("memory API {label} body was cut off while reading"))
        })?;
        // Checked before appending: one oversized chunk would otherwise be
        // allocated in full before the limit is noticed.
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(Error::Engine(format!(
                "memory API {label} response exceeds the {limit}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads at most [`MAX_ERROR_BODY_BYTES`] of a non-success body, never
/// failing: the caller is already returning the status error, and a read
/// fault must not mask it. Truncation is silent; an error body is diagnostic
/// text, not data.
pub(crate) async fn read_error_body(response: reqwest::Response) -> String {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let room = MAX_ERROR_BODY_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MAX_ERROR_BODY_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}
