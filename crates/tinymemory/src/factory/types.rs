//! Descriptor, config and credential types for the engine factory.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::BearerSource;

/// What the factory needs to know about an engine besides its credential.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Base URL. `None` selects the engine's default where it has one.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// `"cloud"` or `"self_hosted"` for engines with more than one deployment.
    #[serde(default)]
    pub deployment: Option<String>,
}

/// How the credential for an engine is supplied.
///
/// Its `Debug` output never shows a secret.
#[derive(Clone, Default)]
pub enum EngineCredential {
    /// No credential.
    #[default]
    None,
    /// A fixed API key or token.
    Static(String),
    /// A token resolved on every request (a refreshing session). Only engines
    /// that take a host-issued bearer (`tinyhumans`) accept it.
    Dynamic(Arc<dyn BearerSource>),
}

impl std::fmt::Debug for EngineCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("EngineCredential::None"),
            Self::Static(_) => f.write_str("EngineCredential::Static(<redacted>)"),
            Self::Dynamic(_) => f.write_str("EngineCredential::Dynamic(<source>)"),
        }
    }
}

impl EngineCredential {
    #[allow(dead_code)]
    /// The static value, treating an empty or blank string as absent.
    pub(super) fn static_value(&self) -> Option<&str> {
        match self {
            Self::Static(value) if !value.trim().is_empty() => Some(value.as_str()),
            _ => None,
        }
    }
}

/// One selectable engine, for a picker UI or a config validator.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EngineDescriptor {
    /// The stable id passed to [`super::build_provider`].
    pub id: &'static str,
    /// Human label for a picker.
    pub label: &'static str,
    /// One sentence on what the engine is.
    pub description: &'static str,
    /// Whether the user supplies a base URL.
    pub needs_endpoint: bool,
    /// Whether the engine takes a user-supplied key or token at all.
    pub needs_key: bool,
    /// Whether it works without one in at least one deployment.
    pub key_optional: bool,
    /// Selectable deployments; empty when there is only one.
    pub deployments: Vec<&'static str>,
    /// The endpoint to prefill or fall back to.
    pub default_endpoint: Option<&'static str>,
    /// Whether the engine is a hosted service whose credential the *host*
    /// issues (a session bearer) rather than one the user pastes.
    pub hosted: bool,
}

/// The engines compiled into this build, in display order.
#[must_use]
#[allow(clippy::vec_init_then_push, unused_mut)]
pub fn list_engines() -> Vec<EngineDescriptor> {
    let mut engines = Vec::new();
    #[cfg(feature = "tinycortex")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::TINYCORTEX_DRIVER_ID,
        label: "TinyCortex (local)",
        description: "The embedded TinyCortex engine over an in-memory store. No network, no key.",
        needs_endpoint: false,
        needs_key: false,
        key_optional: false,
        deployments: vec![],
        default_endpoint: None,
        hosted: false,
    });
    #[cfg(feature = "supermemory")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::SUPERMEMORY_DRIVER_ID,
        label: "Supermemory",
        description: "Supermemory, hosted or self-hosted.",
        needs_endpoint: true,
        needs_key: true,
        key_optional: true,
        deployments: vec![],
        default_endpoint: Some(tinymemory_remote::SUPERMEMORY_API_ENDPOINT),
        hosted: false,
    });
    #[cfg(feature = "mem0")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::MEM0_DRIVER_ID,
        label: "Mem0",
        description: "Mem0 Cloud or a self-hosted Mem0 server.",
        needs_endpoint: true,
        needs_key: true,
        key_optional: true,
        deployments: vec!["cloud", "self_hosted"],
        default_endpoint: Some(tinymemory_remote::MEM0_API_ENDPOINT),
        hosted: false,
    });
    #[cfg(feature = "cognee")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::COGNEE_DRIVER_ID,
        label: "Cognee",
        description: "Cognee Cloud or a self-hosted Cognee server.",
        needs_endpoint: true,
        needs_key: true,
        key_optional: true,
        deployments: vec!["cloud", "self_hosted"],
        default_endpoint: None,
        hosted: false,
    });
    #[cfg(feature = "cortex")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::CORTEX_DRIVER_ID,
        label: "CortexDB",
        description: "CortexDB, managed or self-hosted, with your own API key.",
        // Cloud needs none (it defaults); self-hosted supplies one. That
        // split is expressed by `deployments`, not by this flag.
        needs_endpoint: false,
        needs_key: true,
        key_optional: false,
        deployments: vec!["cloud", "self_hosted"],
        default_endpoint: Some(tinymemory_remote::CORTEX_API_ENDPOINT),
        hosted: false,
    });
    #[cfg(feature = "agentmemory")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::AGENTMEMORY_DRIVER_ID,
        label: "AgentMemory",
        description: "A local or self-hosted AgentMemory REST server.",
        needs_endpoint: true,
        needs_key: true,
        key_optional: true,
        deployments: vec![],
        default_endpoint: Some(tinymemory_remote::AGENTMEMORY_API_ENDPOINT),
        hosted: false,
    });
    #[cfg(feature = "tinyhumans")]
    engines.push(EngineDescriptor {
        id: tinymemory_api::drivers::TINYHUMANS_DRIVER_ID,
        label: "CortexDB (via TinyHumans)",
        description:
            "CortexDB hosted by TinyHumans. The credential is the host's signed-in session.",
        needs_endpoint: false,
        needs_key: false,
        key_optional: false,
        deployments: vec![],
        default_endpoint: Some(tinymemory_remote::TINYHUMANS_API_ENDPOINT),
        hosted: true,
    });
    engines
}
