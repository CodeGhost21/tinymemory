//! Tests for the module-side config loader.

use std::path::PathBuf;

use tinymemory_api::host::MemoryConfig;
use tinymemory_core::config_loader::ConfigLoader;
use tinymemory_tinycortex::engine::EngineRuntimeConfig;

use super::{ModuleConfigLoader, FOREIGN_SNAPSHOT};
use crate::config::ModuleConfig;

fn module_config(workspace: &str) -> ModuleConfig {
    ModuleConfig {
        workspace_dir: PathBuf::from(workspace),
        memory_sources: serde_json::json!([{ "id": "gmail:1", "kind": "composio" }]),
        ..ModuleConfig::default()
    }
}

#[tokio::test]
async fn load_answers_from_the_module_config() {
    let loader = ModuleConfigLoader::new(&module_config("/tmp/module-workspace"));

    let config = loader.load().await.expect("the module always has a config");

    assert_eq!(
        config.workspace_dir(),
        &PathBuf::from("/tmp/module-workspace")
    );
    // The anchor `reload_snapshot` compares on, derived rather than configured:
    // the module has no config file of its own, so the path it reports has to
    // be the one the engine would look for inside its workspace.
    assert_eq!(
        config.config_path(),
        &PathBuf::from("/tmp/module-workspace/config.toml")
    );
    // The source registry has to survive verbatim: the periodic loops decide
    // which sources are enabled by decoding exactly this value, and an empty
    // one reads as "no source has an entry yet", which silently re-enables
    // sources the user switched off.
    let sources = config
        .memory_sources_json()
        .expect("memory sources round-trip");
    assert_eq!(sources[0]["id"], "gmail:1");
}

/// The two settings the periodic sync loops gate on reach them through here.
///
/// This is the end-to-end shape of the first two blockers: both loops reload
/// config on every tick through this loader, and both used to receive constants
/// instead of the host's answers — a cadence of `Some(0)`, which the contract
/// reads as manual-only and which skips every source with nothing logged, and an
/// empty Composio mode, which never selects the one branch a module can serve.
/// A regression here is invisible at runtime, so it is pinned at the seam.
#[tokio::test]
async fn the_loader_answers_the_hosts_cadence_and_composio_mode() {
    let mut config = module_config("/tmp/module-workspace");
    config.memory_sync_interval_secs = Some(3_600);
    config.composio_mode = "direct".to_string();
    config.composio_entity_id = "entity-7".to_string();

    let answered = ModuleConfigLoader::new(&config)
        .load()
        .await
        .expect("the module always has a config");

    assert_eq!(answered.memory_sync_interval_secs(), Some(3_600));
    assert!(answered.composio().is_direct());
    assert_eq!(answered.composio().entity_id, "entity-7");
}

/// "Manual only" has to survive as itself.
///
/// The cadence is the one field where two different values produce the same
/// observable behaviour — a loop that fires nothing — so a bug that turned a
/// user's `Some(0)` into a default cadence, or a default into `Some(0)`, would
/// be found only by a user noticing their data was wrong weeks later.
#[tokio::test]
async fn a_manual_only_cadence_survives_the_loader_intact() {
    let mut config = module_config("/tmp/module-workspace");
    config.memory_sync_interval_secs = Some(0);

    let answered = ModuleConfigLoader::new(&config)
        .load()
        .await
        .expect("the module always has a config");

    assert_eq!(answered.memory_sync_interval_secs(), Some(0));
}

/// The one field that would smuggle a credential back out.
#[tokio::test]
async fn the_loader_hands_back_no_carried_credential() {
    let mut config = module_config("/tmp/module-workspace");
    config.memory = MemoryConfig {
        agentmemory_secret: Some("remote-backend-token".to_string()),
        ..MemoryConfig::default()
    };

    let loader = ModuleConfigLoader::new(&config);

    // The input still carries it — so this asserts that the loader's copy
    // diverged, not that the fixture was empty to begin with.
    assert!(config.memory.agentmemory_secret.is_some());
    let answered = loader.load().await.expect("the module always has a config");
    assert!(answered.memory().agentmemory_secret.is_none());
}

#[tokio::test]
async fn reloading_our_own_snapshot_answers_with_the_module_config() {
    let loader = ModuleConfigLoader::new(&module_config("/tmp/module-workspace"));
    let snapshot = loader.load().await.expect("load");

    let reloaded = loader
        .reload_snapshot(&*snapshot)
        .await
        .expect("our own snapshot is re-readable");

    assert_eq!(
        reloaded.workspace_dir(),
        &PathBuf::from("/tmp/module-workspace")
    );
}

#[tokio::test]
async fn reloading_a_foreign_snapshot_is_refused_without_naming_a_path() {
    let loader = ModuleConfigLoader::new(&module_config("/tmp/module-workspace"));
    let foreign = EngineRuntimeConfig::from(&module_config("/tmp/somebody-elses-workspace"));

    let error = loader
        .reload_snapshot(&foreign)
        .await
        .expect_err("a snapshot from another workspace is refused");

    assert_eq!(error, FOREIGN_SNAPSHOT);
    // A workspace path identifies a user, and this string travels back across
    // the bus into logs this module does not own.
    assert!(!error.contains("somebody-elses-workspace"), "{error}");
    assert!(!error.contains("module-workspace"), "{error}");
}
