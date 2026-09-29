//! Config-driven engine construction.
//!
//! A host that binds its engine from configuration (an id, an endpoint, a
//! deployment and a credential) needs two things and nothing else: *which
//! engines can I offer* and *build me one*. [`list_engines`] answers the first
//! for exactly the engines compiled in; [`build_provider`] answers the second.
//!
//! Every engine arm is gated on that engine's own Cargo feature, so the module
//! compiles under any feature combination and an engine that was compiled out
//! is neither listed nor buildable (`build_provider` reports it as unknown, the
//! same as a typo).
//!
//! # Ids and deployments
//!
//! | id | deployments | endpoint | credential |
//! | --- | --- | --- | --- |
//! | `tinycortex` | — | none | none |
//! | `supermemory` | — | required | optional key |
//! | `mem0` | `cloud`, `self_hosted` | required | key (required for cloud) |
//! | `cognee` | `cloud`, `self_hosted` | required | key (required for cloud) |
//! | `cortex` | `cloud`, `self_hosted` | self-hosted only | key |
//! | `agentmemory` | — | default `http://localhost:3111` | optional secret |
//! | `tinyhumans` | — | default `https://api.tinyhumans.ai` | bearer from the host |
//!
//! Where `deployment` is `None`, `mem0` infers cloud from the endpoint being
//! Mem0's own API URL, `cognee` defaults to self-hosted, and `cortex` infers
//! cloud from a missing endpoint or CortexDB's own API URL.

mod build;
mod types;

pub use build::build_provider;
pub use types::{list_engines, EngineConfig, EngineCredential, EngineDescriptor};

/// A per-request bearer token supplier, re-exported so a host can implement it
/// without depending on the adapter crate.
pub use tinymemory_remote::{BearerSource, StaticBearer};

#[cfg(test)]
mod test;
