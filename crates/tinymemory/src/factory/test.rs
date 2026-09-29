//! Factory tests: listing, per-engine construction, and refusals.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;

use super::*;
#[allow(unused_imports)]
use crate::provider::MemoryCore;

#[allow(dead_code)]
fn config(endpoint: Option<&str>, deployment: Option<&str>) -> EngineConfig {
    EngineConfig {
        endpoint: endpoint.map(str::to_owned),
        deployment: deployment.map(str::to_owned),
    }
}

fn key(value: &str) -> EngineCredential {
    EngineCredential::Static(value.to_owned())
}

fn build(
    id: &str,
    config: &EngineConfig,
    credential: EngineCredential,
) -> anyhow::Result<Arc<dyn crate::provider::MemoryProvider>> {
    build_provider(id, config, credential)
}

#[test]
fn an_unknown_engine_is_refused() {
    let error = build("nonesuch", &EngineConfig::default(), EngineCredential::None)
        .err()
        .expect("unknown id");
    assert!(error.to_string().contains("unknown engine"), "{error}");
}

/// The smallest config and credential that lets each engine build.
fn minimal(id: &str) -> (EngineConfig, EngineCredential) {
    match id {
        "supermemory" | "cognee" => (
            config(Some("http://127.0.0.1:9"), None),
            EngineCredential::None,
        ),
        "mem0" => (
            config(Some("http://127.0.0.1:9"), Some("self_hosted")),
            EngineCredential::None,
        ),
        "cortex" | "tinyhumans" => (EngineConfig::default(), key("a-key")),
        _ => (EngineConfig::default(), EngineCredential::None),
    }
}

#[test]
fn every_listed_engine_has_a_unique_id_and_builds_with_a_minimal_config() {
    let engines = list_engines();
    let mut ids: Vec<_> = engines.iter().map(|e| e.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), engines.len(), "ids are unique");
    for engine in &engines {
        let (config, credential) = minimal(engine.id);
        let provider = build(engine.id, &config, credential)
            .unwrap_or_else(|error| panic!("{} did not build: {error}", engine.id));
        assert_eq!(provider.driver_id(), engine.id);
    }
}

#[cfg(feature = "tinycortex")]
#[tokio::test]
async fn tinycortex_builds_in_memory_and_round_trips() {
    use crate::types::{MemoryCategory, MemoryTaint};
    let provider = build(
        "tinycortex",
        &EngineConfig::default(),
        EngineCredential::None,
    )
    .expect("builds");
    assert_eq!(provider.driver_id(), "tinycortex");
    provider
        .store(
            "ns",
            "k",
            "v",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    assert!(provider.get("ns", "k").await.expect("get").is_some());
    let listed = list_engines();
    let d = listed
        .iter()
        .find(|e| e.id == "tinycortex")
        .expect("listed");
    assert_eq!(d.label, "TinyCortex (local)");
    assert!(!d.needs_endpoint && !d.needs_key && !d.hosted);
}

#[cfg(feature = "supermemory")]
#[test]
fn supermemory_needs_an_endpoint() {
    let error = build(
        "supermemory",
        &EngineConfig::default(),
        EngineCredential::None,
    )
    .err()
    .expect("endpoint required");
    assert!(error.to_string().contains("endpoint"), "{error}");
    let provider = build(
        "supermemory",
        &config(Some("http://127.0.0.1:9"), None),
        EngineCredential::None,
    )
    .expect("builds");
    assert_eq!(provider.driver_id(), "supermemory");
}

#[cfg(feature = "mem0")]
#[test]
fn mem0_picks_cloud_or_self_hosted_and_advertises_graph() {
    // Cloud (explicit or inferred from the endpoint) needs a key.
    assert!(build(
        "mem0",
        &config(Some("https://api.mem0.ai"), None),
        EngineCredential::None
    )
    .is_err());
    let cloud = build(
        "mem0",
        &config(Some("https://api.mem0.ai"), None),
        key("m0-key"),
    )
    .expect("cloud builds");
    assert_eq!(cloud.driver_id(), "mem0");
    assert!(cloud.as_graph().is_some());
    let hosted = build(
        "mem0",
        &config(Some("http://127.0.0.1:9"), Some("self_hosted")),
        EngineCredential::None,
    )
    .expect("self-hosted needs no key");
    assert!(hosted.as_graph().is_some());
    let error = build(
        "mem0",
        &config(Some("http://127.0.0.1:9"), Some("nope")),
        EngineCredential::None,
    )
    .err()
    .expect("bad deployment");
    assert!(error.to_string().contains("deployment"), "{error}");
}

#[cfg(feature = "cognee")]
#[test]
fn cognee_defaults_to_self_hosted_and_cloud_needs_a_key() {
    let hosted = build(
        "cognee",
        &config(Some("http://127.0.0.1:9"), None),
        EngineCredential::None,
    )
    .expect("builds");
    assert_eq!(hosted.driver_id(), "cognee");
    assert!(hosted.as_graph().is_some());
    assert!(build(
        "cognee",
        &config(Some("https://cloud.example"), Some("cloud")),
        EngineCredential::None
    )
    .is_err());
    assert!(build(
        "cognee",
        &config(Some("https://cloud.example"), Some("cloud")),
        key("k")
    )
    .is_ok());
}

#[cfg(feature = "cortex")]
#[test]
fn cortex_needs_a_key_and_a_self_hosted_endpoint() {
    assert!(build("cortex", &EngineConfig::default(), EngineCredential::None).is_err());
    let cloud = build("cortex", &EngineConfig::default(), key("cx-key")).expect("cloud default");
    assert_eq!(cloud.driver_id(), "cortex");
    assert!(cloud.as_answer().is_some());
    assert!(build("cortex", &config(None, Some("self_hosted")), key("cx-key")).is_err());
    // A custom endpoint under `cloud` is honoured, not silently discarded.
    assert!(
        build(
            "cortex",
            &config(Some("http://memory.example.com"), Some("cloud")),
            key("cx-key")
        )
        .is_err(),
        "the endpoint was used, so cleartext HTTP was refused"
    );
    assert!(build(
        "cortex",
        &config(Some("https://staging.example.com"), Some("cloud")),
        key("cx-key")
    )
    .is_ok());
    assert!(build(
        "cortex",
        &config(Some("http://127.0.0.1:3141"), Some("self_hosted")),
        key("cx-key")
    )
    .is_ok());
    // Cleartext to a remote host with a credential is refused.
    assert!(build(
        "cortex",
        &config(Some("http://memory.example.com"), Some("self_hosted")),
        key("cx-key")
    )
    .is_err());
}

#[cfg(feature = "agentmemory")]
#[test]
fn agentmemory_defaults_to_the_local_endpoint() {
    let provider = build(
        "agentmemory",
        &EngineConfig::default(),
        EngineCredential::None,
    )
    .expect("builds");
    assert_eq!(provider.driver_id(), "agentmemory");
}

#[allow(dead_code)]
struct Fixed(&'static str);

#[async_trait]
impl BearerSource for Fixed {
    async fn bearer(&self) -> anyhow::Result<String> {
        Ok(self.0.to_owned())
    }
}

#[cfg(feature = "tinyhumans")]
#[test]
fn tinyhumans_takes_a_static_or_dynamic_bearer_and_defaults_its_endpoint() {
    let listed = list_engines();
    let d = listed
        .iter()
        .find(|e| e.id == "tinyhumans")
        .expect("listed");
    assert_eq!(d.label, "CortexDB (via TinyHumans)");
    assert!(d.hosted && !d.needs_key && !d.needs_endpoint);
    assert_eq!(d.default_endpoint, Some("https://api.tinyhumans.ai"));

    assert!(build(
        "tinyhumans",
        &EngineConfig::default(),
        EngineCredential::None
    )
    .is_err());
    let with_static =
        build("tinyhumans", &EngineConfig::default(), key("tiny_live_x")).expect("static bearer");
    assert_eq!(with_static.driver_id(), "tinyhumans");
    let dynamic = build(
        "tinyhumans",
        &EngineConfig::default(),
        EngineCredential::Dynamic(Arc::new(Fixed("jwt"))),
    )
    .expect("dynamic bearer");
    assert!(dynamic.as_answer().is_some());
    // The credential is not a fixed-key engine's business.
    #[cfg(feature = "cortex")]
    assert!(build(
        "cortex",
        &EngineConfig::default(),
        EngineCredential::Dynamic(Arc::new(Fixed("jwt")))
    )
    .is_err());
}

#[test]
fn credential_debug_never_shows_the_secret() {
    let rendered = format!("{:?}", key("super-secret-token"));
    assert!(!rendered.contains("super-secret-token"), "{rendered}");
}

#[test]
fn descriptors_serialize() {
    let json = serde_json::to_string(&list_engines()).expect("serializes");
    assert!(json.starts_with('['));
}
