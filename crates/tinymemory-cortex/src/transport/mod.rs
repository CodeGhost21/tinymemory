//! The HTTP transport both wires share.
//!
//! [`HttpClient`] resolves a route against the endpoint, attaches the bearer
//! (resolved per attempt and marked sensitive), sends, reads the body against
//! a cap, unwraps the TinyHumans envelope when the wire has one, and types
//! every failure (see `failure`).
//!
//! **Reads retry; writes do not.** Every call states its [`Attempts`]. A read
//! (listing, recall) is retried up to three times with 250ms·2ⁿ backoff on
//! [`crate::Error::Unavailable`]; a write is sent once, because a timeout on a
//! write leaves whether it applied unknown. Hosted writes recover from that
//! one level up, with an `Idempotency-Key` claim (see `log::write`).

mod body;
mod failure;

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Method, RequestBuilder, Url};
use serde_json::Value;

use crate::credential::CortexCredential;
use crate::descriptor::CortexWire;
use crate::error::{Error, Result};

pub(crate) use failure::health_reason;

/// Default per-request deadline.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Ceiling on the connect phase, so a black-holed endpoint does not spend the
/// whole request budget before the first byte.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Attempts a retrying read makes.
const READ_ATTEMPTS: u32 = 3;

/// First read-retry gap; it doubles per attempt.
const READ_BACKOFF: Duration = Duration::from_millis(250);

/// Whether a call may be repeated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Attempts {
    /// A read: retried on a transient failure.
    RetryTransient,
    /// A write: one attempt, whatever the failure.
    Once,
}

/// HTTP transport for one endpoint, wire and credential. `Debug` shows the
/// endpoint origin, never the credential.
#[derive(Clone)]
pub(crate) struct HttpClient {
    inner: reqwest::Client,
    endpoint: Url,
    credential: CortexCredential,
    wire: CortexWire,
    read_backoff: Duration,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("endpoint", &self.endpoint.origin().ascii_serialization())
            .field("wire", &self.wire)
            .finish_non_exhaustive()
    }
}

/// Refuses cleartext HTTP to a non-loopback host. Every engine here is
/// credentialed, so a cleartext endpoint would put the bearer on the wire.
pub(crate) fn ensure_secure_endpoint(url: &Url) -> Result<()> {
    if url.scheme() != "http" {
        return Ok(());
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::Config("memory endpoint has no host".to_string()))?;
    let ip_host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || ip_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(Error::Config(
            "credentialed memory endpoints must use https unless they are loopback".to_string(),
        ))
    }
}

impl HttpClient {
    /// A client for `endpoint` on `wire`.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for an unparsable or non-HTTP(S) endpoint, a
    /// cleartext non-loopback endpoint, or a blank static credential.
    pub(crate) fn new(
        wire: CortexWire,
        endpoint: &str,
        credential: CortexCredential,
    ) -> Result<Self> {
        let mut url = Url::parse(endpoint)
            .map_err(|_| Error::Config("memory endpoint is not a valid URL".to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Config(
                "memory endpoint must use http or https".to_string(),
            ));
        }
        ensure_secure_endpoint(&url)?;
        if let CortexCredential::Static(key) = &credential
            && key.trim().is_empty()
        {
            return Err(Error::Config("the API key must not be empty".to_string()));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(Self {
            inner: build_inner(DEFAULT_TIMEOUT)?,
            endpoint: url,
            credential,
            wire,
            read_backoff: READ_BACKOFF,
        })
    }

    /// Rebuilds the client with a different per-request deadline. A retrying
    /// read can take about three times this plus 750ms of backoff.
    pub(crate) fn set_timeout(&mut self, timeout: Duration) -> Result<()> {
        self.inner = build_inner(timeout)?;
        Ok(())
    }

    /// Shortens the read-retry backoff (tests).
    #[cfg(test)]
    pub(crate) fn set_read_backoff(&mut self, backoff: Duration) {
        self.read_backoff = backoff;
    }

    /// The wire this client speaks.
    pub(crate) fn wire(&self) -> CortexWire {
        self.wire
    }

    /// The endpoint's origin, for `Debug` output.
    pub(crate) fn origin(&self) -> String {
        self.endpoint.origin().ascii_serialization()
    }

    fn host(&self) -> &str {
        self.endpoint.host_str().unwrap_or("<endpoint>")
    }

    /// Resolves `path` and attaches the bearer.
    ///
    /// The bearer is resolved here, on every attempt, so a refreshed token is
    /// used at once. A source failure, a blank token, or a token that cannot
    /// be a header value (CR/LF) is [`Error::Unauthorized`] and no request is
    /// sent; no message carries the token.
    async fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let url = self
            .endpoint
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::Engine(format!("memory API path `{}` is invalid", label(path))))?;
        let token = self.credential.resolve().await.map_err(|error| {
            Error::Unauthorized(format!(
                "the bearer source could not supply a credential: {error}"
            ))
        })?;
        let header = credential_header(&token)?;
        Ok(self
            .inner
            .request(method, url)
            .header(AUTHORIZATION, header))
    }

    /// Sends a JSON request and returns the decoded body (unwrapped from the
    /// hosted envelope). A hosted `POST` sent [`Attempts::Once`] carries a
    /// fresh `Idempotency-Key` claim.
    pub(crate) async fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        attempts: Attempts,
    ) -> Result<Value> {
        match attempts {
            Attempts::Once => {
                let key = (self.wire == CortexWire::TinyHumans && method == Method::POST)
                    .then(fresh_idempotency_key);
                self.attempt(method, path, body, key.as_deref()).await
            }
            Attempts::RetryTransient => {
                let mut tried = 0;
                loop {
                    tried += 1;
                    match self.attempt(method.clone(), path, body, None).await {
                        Err(Error::Unavailable(_)) if tried < READ_ATTEMPTS => {
                            tokio::time::sleep(self.read_backoff * 2_u32.pow(tried - 1)).await;
                        }
                        other => return other,
                    }
                }
            }
        }
    }

    /// One attempt of a hosted write under a caller-chosen `Idempotency-Key`,
    /// so the caller can reuse one claim across its own retries.
    pub(crate) async fn json_keyed(&self, path: &str, body: &Value, key: &str) -> Result<Value> {
        self.attempt(Method::POST, path, Some(body), Some(key))
            .await
    }

    /// GETs `path` and checks it succeeds (and, hosted, that the envelope
    /// says so). One attempt: a probe reports what it saw.
    pub(crate) async fn probe(&self, path: &str) -> Result<()> {
        self.attempt(Method::GET, path, None, None)
            .await
            .map(|_| ())
    }

    /// One send.
    async fn attempt(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        idempotency: Option<&str>,
    ) -> Result<Value> {
        let label = label(path);
        let mut request = self.request(method, path).await?;
        if let Some(key) = idempotency.filter(|_| self.wire == CortexWire::TinyHumans) {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| failure::transport_error(self.host(), &error))?;
        let status = response.status();
        if !status.is_success() {
            let text = body::read_error_body(response).await;
            return Err(match self.wire {
                CortexWire::Direct => {
                    failure::direct_status_error(self.host(), label, status, &text)
                }
                CortexWire::TinyHumans => {
                    failure::hosted_status_error(self.host(), label, status, &text)
                }
            });
        }
        let bytes = body::read_capped(response, label).await?;
        match self.wire {
            CortexWire::TinyHumans => failure::unwrap_envelope(self.host(), label, status, &bytes),
            CortexWire::Direct if bytes.is_empty() => Ok(Value::Null),
            CortexWire::Direct => serde_json::from_slice(&bytes).map_err(|_| {
                Error::Engine(format!(
                    "memory API {label} on {} returned invalid JSON",
                    self.host()
                ))
            }),
        }
    }
}

/// The route part of a path, for messages: query strings carry scopes and
/// cursors, which are noise in an error.
fn label(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// One place builds the reqwest client, so the two timeouts stay paired.
fn build_inner(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT.min(timeout))
        .build()
        .map_err(|_| Error::Config("the HTTP client could not be built".to_string()))
}

/// The `Authorization` value for `token`, marked sensitive so nothing that
/// formats the request prints it.
///
/// Parsed up front: a token holding CR/LF (header injection) or another byte
/// no header may carry is refused here as a credential fault, before any
/// request, and the refusal carries no part of the token.
pub(crate) fn credential_header(token: &str) -> Result<HeaderValue> {
    let token = token.trim();
    if token.is_empty() {
        return Err(Error::Unauthorized("the credential is empty".to_string()));
    }
    let mut header = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        Error::Unauthorized("the credential is not a valid HTTP header value".to_string())
    })?;
    header.set_sensitive(true);
    Ok(header)
}

/// A fresh key for every write: the body's `idempotency_key` and the hosted
/// `Idempotency-Key` claim.
///
/// Never derived from content. CortexDB keeps a forgotten event's
/// idempotency record, so a content-derived key would make re-storing an item
/// after forgetting it a silent no-op; and the hosted memory API answers
/// every replay of a claim with 409, so a content-derived claim would refuse
/// an identical re-store. Store detects a replay itself, by looking the item
/// up, before it writes.
///
/// Three parts: a per-process salt from the OS-seeded `RandomState`, the
/// wall-clock nanoseconds, and a counter, so neither two writes in one
/// process nor two processes writing at once can mint the same key.
pub(crate) fn fresh_idempotency_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    static SALT: OnceLock<u64> = OnceLock::new();

    let salt = *SALT.get_or_init(|| {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    });
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "tm-{salt:016x}-{nanos}-{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Percent-encodes everything outside the URI unreserved set, byte by byte.
/// A cursor is opaque engine output, and a `+`, `&`, `=` or `#` in one would
/// silently reshape the query string.
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
