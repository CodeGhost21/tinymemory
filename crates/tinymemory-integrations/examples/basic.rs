//! Choose and build a memory engine the way a host does.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p tinymemory-integrations --example basic
//! ```
//!
//! It needs no network: building an engine validates configuration and
//! prepares the client, but sends nothing until the first call.

use std::sync::Arc;

use async_trait::async_trait;
use tinymemory_integrations::{BearerSource, EngineCredential, MemoryConfig, list_engines};

/// A host's session store: the token is looked up on every request, so a
/// refreshed session is picked up without rebuilding the engine.
struct Session;

#[async_trait]
impl BearerSource for Session {
    async fn bearer(&self) -> tinymemory_integrations::Result<String> {
        Ok("session-jwt-from-the-host".to_string())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for engine in list_engines() {
        let modes: Vec<&str> = engine.fetch_modes.iter().map(|m| m.as_str()).collect();
        println!(
            "{:<11} hosted={:<5} default={:<28} fetch={modes:?}",
            engine.id,
            engine.hosted,
            engine.default_endpoint.unwrap_or("-"),
        );
    }

    // A host keeps `MemoryConfig` in its config file and the credential in
    // its secret store.
    let config: MemoryConfig = toml::from_str(r#"engine = "tinyhumans""#)?;
    let engine = config.build(EngineCredential::Dynamic(Arc::new(Session)))?;
    println!("built `{}`", engine.descriptor().id);

    // Misconfiguration is refused up front, never at the first write.
    let refused = MemoryConfig {
        engine: "cortexdb".to_string(),
        ..MemoryConfig::default()
    }
    .build(EngineCredential::None);
    if let Err(error) = refused {
        println!("refused: {error}");
    }
    Ok(())
}
