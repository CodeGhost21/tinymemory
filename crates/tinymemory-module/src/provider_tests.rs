//! Tests for the surrounding module: the config-path resolution the engine
//! provider is built from (openhuman#5820).

use super::host_config_path;
use crate::ModuleConfig;

fn config_with(
    workspace_dir: &std::path::Path,
    config_path: Option<std::path::PathBuf>,
) -> ModuleConfig {
    let mut config: ModuleConfig =
        serde_json::from_value(serde_json::json!({ "workspace_dir": workspace_dir }))
            .expect("a workspace alone deserializes");
    config.config_path = config_path;
    config
}

/// What the host sends wins, whether or not the file exists yet — the host
/// is about to write it.
#[test]
fn an_explicit_host_path_is_taken_verbatim() {
    let config = config_with(
        std::path::Path::new("/w/workspace"),
        Some("/elsewhere/config.toml".into()),
    );
    assert_eq!(
        host_config_path(&config),
        std::path::PathBuf::from("/elsewhere/config.toml")
    );
}

/// An older host sends nothing; the registry file beside `workspace/` is the
/// documented layout, so it is used when it exists.
#[test]
fn an_old_host_falls_back_to_the_file_beside_the_workspace() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(root.path().join("config.toml"), "[[memory_sources]]\n").unwrap();

    let config = config_with(&workspace, None);
    assert_eq!(host_config_path(&config), root.path().join("config.toml"));
}

/// With neither, the historical path stands — behaviour unchanged for a host
/// that never had a registry file at all.
#[test]
fn with_no_candidate_the_historical_path_is_kept() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = root.path().join("workspace");
    let config = config_with(&workspace, None);
    assert_eq!(host_config_path(&config), workspace.join("config.toml"));
}
