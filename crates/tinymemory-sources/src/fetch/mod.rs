//! Fetching one URL into a [`RawDocument`] or a link [`StoreItem`].
//!
//! A URL a user types is an SSRF vector: `http://169.254.169.254/` is a cloud
//! metadata endpoint, `http://localhost:6379/` is somebody's Redis, and a
//! hostname that resolves publicly on the first lookup can resolve to a private
//! address on the second. Every fetch here goes through the same guard as the
//! RSS and web-page readers ([`crate::readers::ssrf`]): a scheme and host
//! policy, a resolver that pins connections to globally routable addresses,
//! and per-hop redirect re-checks.
//!
//! No scheduling, no retries, no credentials, no robots.txt: this fetches one
//! URL, once, when asked. Conversion to markdown is `tinymemory-documents`'.

use tinymemory_api::{MemoryMeta, SourceKind, StoreItem};
use tinymemory_documents::{document_item, DocumentConverter, RawDocument, MAX_DOCUMENT_BYTES};

use crate::error::{Error, Result};
use crate::readers::ssrf::{build_client, is_url_allowed, read_body_capped};

/// Fetch `url` and return its body as a [`RawDocument`].
///
/// The response's `Content-Type` becomes the document's declared MIME type and
/// the URL becomes its origin; the last path segment, when it has an
/// extension, becomes its filename so format detection can fall back to it.
///
/// # Errors
///
/// - [`Error::Invalid`] for a malformed URL, one the SSRF guard refuses, or an
///   empty body.
/// - [`Error::Unreachable`] when the request never completed or the body read
///   was interrupted.
/// - [`Error::Upstream`] for a non-success status.
/// - [`Error::TooLarge`] for a body over [`MAX_DOCUMENT_BYTES`].
pub async fn fetch_url(url: &str) -> Result<RawDocument> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| Error::Invalid(format!("invalid url {url:?}: {error}")))?;
    if !is_url_allowed(&parsed) {
        return Err(Error::Invalid(format!(
            "url {url:?} is not an allowed fetch target"
        )));
    }

    let client = build_client().map_err(Error::Reader)?;
    tracing::debug!(
        host = %parsed.host_str().unwrap_or(""),
        "[memory_sources:fetch] fetching url"
    );
    let response = client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|error| Error::Unreachable(format!("fetching {url:?}: {error}")))?;

    response_to_document(url, parsed, response).await
}

/// Fetch `url`, convert it through `converter`, and wrap it as a
/// [`StoreItem::Document`] with `source.kind = Link` and `url` set.
///
/// `source_id` is the configured source's id, when the fetch is for one.
///
/// # Errors
///
/// Whatever [`fetch_url`] returns, plus [`Error::Document`] when conversion
/// fails.
pub async fn link_item(
    url: &str,
    source_id: Option<String>,
    converter: &dyn DocumentConverter,
) -> Result<StoreItem> {
    let document = fetch_url(url).await?;
    let mut meta = MemoryMeta::from_source(SourceKind::Link, source_id);
    meta.url = document.origin.clone().or_else(|| Some(url.to_string()));
    Ok(document_item(converter, &document, meta).await?)
}

/// Validate and convert a completed HTTP response.
async fn response_to_document(
    url: &str,
    parsed: reqwest::Url,
    response: reqwest::Response,
) -> Result<RawDocument> {
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Upstream(format!(
            "fetching {url:?} answered {status}"
        )));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // The cap is applied while reading, not after: a body that would not fit is
    // one this process should never have finished buffering.
    let bytes = read_body_capped(response, MAX_DOCUMENT_BYTES as u64)
        .await
        .map_err(|error| read_error(url, &error))?;

    if bytes.is_empty() {
        return Err(Error::Invalid(format!("{url:?} returned no body")));
    }

    let mut document = RawDocument::new(bytes).with_origin(parsed.to_string());
    if let Some(content_type) = content_type {
        document = document.with_mime(content_type);
    }
    // A URL's last path segment is often the only filename there is, and format
    // detection falls back to it when the server sent no useful type.
    if let Some(name) = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty() && name.contains('.'))
    {
        document = document.with_filename(name.to_string());
    }
    Ok(document)
}

/// Turn a `read_body_capped` failure into the right [`Error`] variant.
///
/// `read_body_capped` collapses two failures into one `String`: a body over
/// the size cap, and a stream that failed mid-read. They need different retry
/// policies, so this tells them apart by the message `read_body_capped` always
/// uses for the size case.
fn read_error(url: &str, error: &str) -> Error {
    if error.contains("exceeds") && error.contains("-byte limit") {
        Error::TooLarge(format!("reading {url:?}: {error}"))
    } else {
        Error::Unreachable(format!("reading {url:?}: {error}"))
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
