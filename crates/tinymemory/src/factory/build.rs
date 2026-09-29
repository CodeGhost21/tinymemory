//! [`build_provider`]: one place that turns an id + config + credential into a
//! bound [`MemoryProvider`].

use std::sync::Arc;

use anyhow::bail;

use crate::provider::MemoryProvider;

use super::{EngineConfig, EngineCredential};

#[allow(unused_imports)]
use tinymemory_api::drivers as ids;

/// The endpoint from `config`, treating blank as absent.
#[cfg(any(
    feature = "supermemory",
    feature = "mem0",
    feature = "cognee",
    feature = "cortex",
    feature = "agentmemory",
    feature = "tinyhumans"
))]
fn endpoint_of(config: &EngineConfig) -> Option<&str> {
    config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Refuses a dynamic credential for an engine that takes a fixed key.
#[cfg(any(
    feature = "supermemory",
    feature = "mem0",
    feature = "cognee",
    feature = "cortex",
    feature = "agentmemory"
))]
fn reject_dynamic(id: &str, credential: &EngineCredential) -> anyhow::Result<()> {
    if matches!(credential, EngineCredential::Dynamic(_)) {
        bail!("{id} takes a fixed key; a dynamic bearer source is only supported by tinyhumans");
    }
    Ok(())
}

/// Builds the provider for engine `id`.
///
/// The per-engine construction is the one the testing harness has always used:
/// Mem0 also advertises Graph through its client-side heuristic, Cognee through
/// its graph provider, CortexDB is the full provider with native ingestion and
/// answers, and `tinyhumans` is CortexDB over the TinyHumans backend.
///
/// # Errors
///
/// Fails when `id` is unknown or compiled out, a required endpoint or key is
/// missing, the deployment name is not recognised, the credential kind does not
/// suit the engine, or the endpoint is invalid (including cleartext HTTP with a
/// credential off loopback). Messages never contain the credential.
#[allow(unused_variables)]
pub fn build_provider(
    id: &str,
    config: &EngineConfig,
    credential: EngineCredential,
) -> anyhow::Result<Arc<dyn MemoryProvider>> {
    match id {
        #[cfg(feature = "tinycortex")]
        ids::TINYCORTEX_DRIVER_ID => {
            let memory: Arc<dyn tinymemory_tinycortex::tinycortex::memory::Memory> =
                Arc::new(tinymemory_tinycortex::InMemoryMemoryStore::new());
            Ok(Arc::new(tinymemory_tinycortex::provider(memory)))
        }
        #[cfg(feature = "supermemory")]
        ids::SUPERMEMORY_DRIVER_ID => {
            reject_dynamic(id, &credential)?;
            let endpoint = endpoint_of(config)
                .ok_or_else(|| anyhow::anyhow!("supermemory requires an endpoint URL"))?;
            let memory =
                tinymemory_remote::SupermemoryMemory::new(endpoint, credential.static_value())?;
            Ok(Arc::new(tinymemory_remote::supermemory_provider(memory)))
        }
        #[cfg(feature = "mem0")]
        ids::MEM0_DRIVER_ID => {
            reject_dynamic(id, &credential)?;
            let endpoint = endpoint_of(config)
                .ok_or_else(|| anyhow::anyhow!("mem0 requires an endpoint URL"))?;
            let key = credential.static_value();
            let is_cloud = match config.deployment.as_deref() {
                Some("cloud") => true,
                Some("self_hosted") => false,
                None => endpoint == tinymemory_remote::MEM0_API_ENDPOINT,
                Some(other) => bail!("unknown Mem0 deployment: {other}"),
            };
            let memory = if is_cloud {
                tinymemory_remote::Mem0Memory::api(
                    endpoint,
                    key.ok_or_else(|| anyhow::anyhow!("Mem0 Cloud requires an API key"))?,
                )
            } else {
                tinymemory_remote::Mem0Memory::new(endpoint, key)
            }?;
            // Also advertises Graph via `Mem0Graph`: a client-side heuristic
            // over the same stored entries, not Mem0's native Graph Memory.
            Ok(Arc::new(tinymemory_remote::mem0_graph_provider(memory)))
        }
        #[cfg(feature = "cognee")]
        ids::COGNEE_DRIVER_ID => {
            reject_dynamic(id, &credential)?;
            let endpoint = endpoint_of(config)
                .ok_or_else(|| anyhow::anyhow!("cognee requires an endpoint URL"))?;
            let key = credential.static_value();
            let is_cloud = match config.deployment.as_deref() {
                Some("cloud") => true,
                Some("self_hosted") | None => false,
                Some(other) => bail!("unknown Cognee deployment: {other}"),
            };
            let memory = if is_cloud {
                tinymemory_remote::CogneeMemory::api(
                    endpoint,
                    key.ok_or_else(|| anyhow::anyhow!("Cognee Cloud requires an API key"))?,
                )
            } else {
                tinymemory_remote::CogneeMemory::new(endpoint, key)
            }?;
            // Cognee is graph-native, so its provider also advertises Graph
            // (relations only; see `CogneeGraph` for the exact split).
            let provider = if is_cloud {
                tinymemory_remote::cognee_api_graph_provider(
                    memory,
                    endpoint,
                    key.unwrap_or_default(),
                )
            } else {
                tinymemory_remote::cognee_graph_provider(memory, endpoint, key)
            }?;
            Ok(Arc::new(provider))
        }
        #[cfg(feature = "cortex")]
        ids::CORTEX_DRIVER_ID => {
            reject_dynamic(id, &credential)?;
            let key = credential
                .static_value()
                .ok_or_else(|| anyhow::anyhow!("CortexDB requires an API key"))?;
            let endpoint = endpoint_of(config);
            let is_cloud = match config.deployment.as_deref() {
                Some("cloud") => true,
                // `api` is the adapter's own name for the same thing.
                Some("self_hosted" | "api") => false,
                None => endpoint.is_none_or(|e| e == tinymemory_remote::CORTEX_API_ENDPOINT),
                Some(other) => bail!("unknown CortexDB deployment: {other}"),
            };
            let memory = if is_cloud {
                tinymemory_remote::CortexMemory::cloud(key)
            } else {
                let endpoint = endpoint.ok_or_else(|| {
                    anyhow::anyhow!("self-hosted CortexDB requires an endpoint URL")
                })?;
                tinymemory_remote::CortexMemory::self_hosted(endpoint, key)
            }?;
            Ok(Arc::new(tinymemory_remote::cortex_provider(memory)))
        }
        #[cfg(feature = "agentmemory")]
        ids::AGENTMEMORY_DRIVER_ID => {
            reject_dynamic(id, &credential)?;
            let endpoint =
                endpoint_of(config).unwrap_or(tinymemory_remote::AGENTMEMORY_API_ENDPOINT);
            let memory =
                tinymemory_remote::AgentMemoryMemory::new(endpoint, credential.static_value())?;
            Ok(Arc::new(tinymemory_remote::agentmemory_provider(memory)))
        }
        #[cfg(feature = "tinyhumans")]
        ids::TINYHUMANS_DRIVER_ID => {
            let source: Arc<dyn tinymemory_remote::BearerSource> = match credential {
                EngineCredential::Dynamic(source) => source,
                EngineCredential::Static(ref value) if !value.trim().is_empty() => {
                    Arc::new(tinymemory_remote::StaticBearer::new(value.trim()))
                }
                _ => bail!(
                    "tinyhumans requires a bearer credential (a session token or API key, \
                     or a dynamic source)"
                ),
            };
            let endpoint =
                endpoint_of(config).unwrap_or(tinymemory_remote::TINYHUMANS_API_ENDPOINT);
            Ok(Arc::new(tinymemory_remote::tinyhumans_provider(
                endpoint, source,
            )?))
        }
        other => bail!("unknown engine: {other}"),
    }
}
