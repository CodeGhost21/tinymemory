//! The engine registry: [`list_engines`] and [`build_engine`].
//!
//! Two engines are registered, both served by `tinymemory-cortex`:
//!
//! | Id | Engine | Endpoint | Credential |
//! | --- | --- | --- | --- |
//! | `cortexdb` | CortexDB's own `/v1/*` API | defaults to the managed API | API key |
//! | `tinyhumans` | CortexDB behind the TinyHumans backend `/memory/*` | defaults to `api.tinyhumans.ai` | session JWT or `tiny_live_` key, usually dynamic |
//!
//! [`build_engine`] refuses an unknown id, a missing required endpoint or
//! credential, and a credentialed cleartext endpoint that is not loopback,
//! all as [`Error::Config`]. Messages never carry the credential.

use std::net::IpAddr;
use std::sync::Arc;

use crate::cortex::{
    BearerSource, CORTEXDB_ENGINE_ID, CortexCredential, CortexEngine, StaticBearer,
    TINYHUMANS_ENGINE_ID,
};
use tinymemory_api::{EngineDescriptor, Error, MemoryEngine, Result};

use crate::config::EngineSettings;

/// How an engine authenticates.
#[derive(Clone, Default)]
pub enum EngineCredential {
    /// No credential, for an engine that needs none.
    #[default]
    None,
    /// One fixed token, for example an API key.
    Static(String),
    /// A token resolved before every request, so a refreshed session is used
    /// at once.
    Dynamic(Arc<dyn BearerSource>),
}

impl std::fmt::Debug for EngineCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "EngineCredential::None",
            Self::Static(_) => "EngineCredential::Static(<redacted>)",
            Self::Dynamic(_) => "EngineCredential::Dynamic(<source>)",
        })
    }
}

impl EngineCredential {
    fn is_present(&self) -> bool {
        match self {
            Self::None => false,
            Self::Static(token) => !token.trim().is_empty(),
            Self::Dynamic(_) => true,
        }
    }
}

/// Every engine this build can construct.
#[must_use]
pub fn list_engines() -> Vec<EngineDescriptor> {
    vec![
        crate::cortex::cortexdb_descriptor(),
        crate::cortex::tinyhumans_descriptor(),
    ]
}

/// Builds the engine `id` from `settings` and `credential`.
///
/// # Errors
///
/// [`Error::Config`] for an unknown id, a missing required endpoint or
/// credential, an endpoint that is not an HTTP(S) URL, or a credentialed
/// cleartext (`http://`) endpoint that is not loopback.
pub fn build_engine(
    id: &str,
    settings: &EngineSettings,
    credential: EngineCredential,
) -> Result<Arc<dyn MemoryEngine>> {
    let descriptor = list_engines()
        .into_iter()
        .find(|descriptor| descriptor.id == id)
        .ok_or_else(|| Error::Config(format!("unknown memory engine `{id}`")))?;
    let endpoint = settings
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .or(descriptor.default_endpoint)
        .ok_or_else(|| Error::Config(format!("memory engine `{id}` needs an endpoint")))?;
    if descriptor.needs_endpoint
        && settings
            .endpoint
            .as_deref()
            .is_none_or(|e| e.trim().is_empty())
    {
        return Err(Error::Config(format!(
            "memory engine `{id}` needs an endpoint"
        )));
    }
    if descriptor.needs_key && !credential.is_present() {
        return Err(Error::Config(format!(
            "memory engine `{id}` needs a credential"
        )));
    }
    if credential.is_present() {
        ensure_secure_endpoint(endpoint)?;
    }
    let engine = match (id, credential) {
        (CORTEXDB_ENGINE_ID, EngineCredential::Static(key)) => {
            CortexEngine::direct(endpoint, CortexCredential::Static(key))?
        }
        (CORTEXDB_ENGINE_ID, EngineCredential::Dynamic(source)) => {
            CortexEngine::direct(endpoint, CortexCredential::Dynamic(source))?
        }
        (TINYHUMANS_ENGINE_ID, EngineCredential::Static(token)) => {
            CortexEngine::tinyhumans(endpoint, Arc::new(StaticBearer::new(token)))?
        }
        (TINYHUMANS_ENGINE_ID, EngineCredential::Dynamic(source)) => {
            CortexEngine::tinyhumans(endpoint, source)?
        }
        (_, EngineCredential::None) => {
            return Err(Error::Config(format!(
                "memory engine `{id}` needs a credential"
            )));
        }
        _ => return Err(Error::Config(format!("unknown memory engine `{id}`"))),
    };
    Ok(Arc::new(engine))
}

/// Refuses a cleartext endpoint off loopback, and anything that is not an
/// HTTP(S) URL with a host.
fn ensure_secure_endpoint(endpoint: &str) -> Result<()> {
    let invalid = || Error::Config("memory endpoint is not an http(s) url".to_string());
    let (scheme, rest) = endpoint.split_once("://").ok_or_else(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err(invalid());
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Ok(()),
        "http" => {
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
            if loopback {
                Ok(())
            } else {
                Err(Error::Config(
                    "credentialed memory endpoints must use https unless they are loopback"
                        .to_string(),
                ))
            }
        }
        _ => Err(invalid()),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
