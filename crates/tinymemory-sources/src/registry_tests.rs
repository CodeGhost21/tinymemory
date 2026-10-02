//! Tests for the TOML-backed source registry.

use super::*;
use crate::types::SourceKind;
use tempfile::TempDir;

fn registry() -> (TempDir, SourceRegistry) {
    let tmp = TempDir::new().unwrap();
    let reg = SourceRegistry::new(tmp.path().join("config.toml"));
    (tmp, reg)
}

fn folder_entry(id: &str) -> MemorySourceEntry {
    let mut e = MemorySourceEntry {
        id: id.into(),
        kind: SourceKind::Folder,
        label: "Notes".into(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: Some("/tmp/notes".into()),
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_commits: None,
        max_issues: None,
        max_prs: None,
        max_items: None,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: None,
    };
    e.glob = Some("**/*.md".into());
    e
}

#[test]
fn list_is_empty_for_missing_file() {
    let (_tmp, reg) = registry();
    assert!(reg.list().unwrap().is_empty());
    assert!(reg.get("anything").unwrap().is_none());
}

#[test]
fn add_get_list_round_trip() {
    let (_tmp, reg) = registry();
    let added = reg.add(folder_entry("src_1")).unwrap();
    assert_eq!(added.id, "src_1");

    let got = reg.get("src_1").unwrap().unwrap();
    assert_eq!(got.kind, SourceKind::Folder);
    assert_eq!(got.path.as_deref(), Some("/tmp/notes"));

    let all = reg.list().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn add_rejects_duplicate_id() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_dup")).unwrap();
    assert!(reg.add(folder_entry("src_dup")).is_err());
}

#[test]
fn add_rejects_invalid_entry() {
    let (_tmp, reg) = registry();
    let mut bad = folder_entry("src_bad");
    bad.path = None; // folder requires a path
    assert!(reg.add(bad).is_err());
    assert!(reg.list().unwrap().is_empty());
}

#[test]
fn concurrent_registry_adds_preserve_every_source() {
    let (_tmp, reg) = registry();
    let writers = 24;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(writers));
    let mut threads = Vec::new();
    for index in 0..writers {
        let reg = reg.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            reg.add(folder_entry(&format!("src_concurrent_{index}")))
                .unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let mut ids: Vec<_> = reg
        .list()
        .unwrap()
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    ids.sort();
    assert_eq!(ids.len(), writers);
    for index in 0..writers {
        assert!(ids.contains(&format!("src_concurrent_{index}")));
    }
}

#[test]
fn update_applies_patch_and_persists() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_u")).unwrap();

    let patch = MemorySourcePatch {
        label: Some("Renamed".into()),
        enabled: Some(false),
        ..Default::default()
    };
    let updated = reg.update("src_u", patch).unwrap();
    assert_eq!(updated.label, "Renamed");
    assert!(!updated.enabled);

    // Re-read from disk to confirm persistence.
    let got = reg.get("src_u").unwrap().unwrap();
    assert_eq!(got.label, "Renamed");
    assert!(!got.enabled);
}

#[test]
fn update_missing_id_errors() {
    let (_tmp, reg) = registry();
    assert!(reg.update("nope", MemorySourcePatch::default()).is_err());
}

#[test]
fn remove_returns_whether_anything_was_removed() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_r")).unwrap();
    assert!(reg.remove("src_r").unwrap());
    assert!(!reg.remove("src_r").unwrap());
    assert!(reg.list().unwrap().is_empty());
}

#[test]
fn list_enabled_by_kind_filters() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_a")).unwrap();
    let mut disabled = folder_entry("src_b");
    disabled.enabled = false;
    reg.add(disabled).unwrap();

    let enabled = reg.list_enabled_by_kind(SourceKind::Folder).unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].id, "src_a");
    assert!(reg
        .list_enabled_by_kind(SourceKind::Conversation)
        .unwrap()
        .is_empty());
}

#[test]
fn write_preserves_other_top_level_config_keys() {
    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, "workspace = \"/data\"\n").unwrap();

    reg.add(folder_entry("src_keep")).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("workspace = \"/data\""));
    assert!(text.contains("[[memory_sources]]"));
}

#[test]
fn write_uses_atomic_temp_file_without_leaving_stale_temp() {
    let (tmp, reg) = registry();
    reg.add(folder_entry("src_atomic")).unwrap();

    let text = std::fs::read_to_string(reg.path()).unwrap();
    assert!(text.contains("src_atomic"));

    let stale_temp_files: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".config.toml.tmp-")
        })
        .collect();
    assert!(stale_temp_files.is_empty());
}

// ── Composio upsert ──

#[test]
fn composio_defaults_for_known_and_unknown_toolkits() {
    assert_eq!(
        memory_sync_defaults_for_toolkit("gmail"),
        (Some(100), Some(30))
    );
    assert_eq!(
        memory_sync_defaults_for_toolkit("slack"),
        (Some(50), Some(14))
    );
    assert_eq!(
        memory_sync_defaults_for_toolkit("unknown_xyz"),
        (Some(30), Some(14))
    );
}

#[test]
fn in_place_upsert_inserts_then_updates_label_only() {
    let mut sources: Vec<MemorySourceEntry> = vec![];
    let (entry, was_insert) =
        upsert_composio_entry_in_place(&mut sources, "gmail", "conn_a", "Gmail · conn_a");
    assert!(was_insert);
    assert_eq!(entry.toolkit.as_deref(), Some("gmail"));
    assert_eq!(entry.max_items, Some(100));
    assert_eq!(entry.sync_depth_days, Some(30));

    // User customises a cap, then a second upsert updates label only.
    sources[0].max_items = Some(7);
    let (entry, was_insert) =
        upsert_composio_entry_in_place(&mut sources, "gmail", "conn_a", "new label");
    assert!(!was_insert);
    assert_eq!(sources.len(), 1);
    assert_eq!(entry.label, "new label");
    assert_eq!(entry.max_items, Some(7));
}

#[test]
fn upsert_composio_source_persists_and_disconnect_removes() {
    let (_tmp, reg) = registry();
    reg.upsert_composio_source("gmail", "conn_a", "Gmail")
        .unwrap();
    reg.upsert_composio_source("slack", "conn_b", "Slack")
        .unwrap();
    assert_eq!(reg.list().unwrap().len(), 2);

    let removed = reg
        .remove_composio_source_by_connection_id("conn_a")
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(reg.list().unwrap().len(), 1);
}

#[test]
fn apply_all_in_enables_and_clears_caps() {
    let (_tmp, reg) = registry();
    let mut capped = folder_entry("src_capped");
    capped.enabled = false;
    capped.max_items = Some(5);
    capped.sync_depth_days = Some(3);
    reg.add(capped).unwrap();

    let updated = reg.apply_all_in().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0].enabled);
    assert!(updated[0].max_items.is_none());
    assert!(updated[0].sync_depth_days.is_none());
}

#[test]
fn memory_source_patch_deserializes_partial_and_github_fields() {
    let json = serde_json::json!({
        "label": "New label",
        "enabled": false,
        "max_commits": 100,
        "max_issues": 50,
        "max_prs": 25
    });
    let patch: MemorySourcePatch = serde_json::from_value(json).unwrap();
    assert_eq!(patch.label.as_deref(), Some("New label"));
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(patch.max_commits, Some(Some(100)));
    assert_eq!(patch.max_issues, Some(Some(50)));
    assert_eq!(patch.max_prs, Some(Some(25)));
    assert!(patch.toolkit.is_none());
}

#[test]
fn memory_source_patch_can_clear_optional_fields_with_null() {
    let (_tmp, reg) = registry();
    let mut entry = folder_entry("src_clear");
    entry.glob = Some("**/*.md".into());
    entry.max_items = Some(10);
    reg.add(entry).unwrap();

    let patch: MemorySourcePatch = serde_json::from_value(serde_json::json!({
        "glob": null,
        "max_items": null
    }))
    .unwrap();
    let updated = reg.update("src_clear", patch).unwrap();
    assert!(updated.glob.is_none());
    assert!(updated.max_items.is_none());
}

#[test]
fn update_rejects_fields_that_do_not_apply_to_source_kind() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_kind")).unwrap();
    let patch: MemorySourcePatch = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/repo"
    }))
    .unwrap();
    assert!(reg.update("src_kind", patch).is_err());
}

// ── File permissions ────────────────────────────────────────────────────────
//
// `atomic_write` renames its temp file over the host's `config.toml`, so the
// temp file's mode becomes the live config's mode. Created with plain
// `File::create` that was `0o666 & ~umask` — 0644 under the usual 022 — which
// silently re-widened a config the host had deliberately written owner-only,
// on every single source mutation. These tests pin the mode of the file this
// registry leaves behind, not the mode it was handed.

/// The mode bits of `path`, or `None` on a platform without file modes.
#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn first_write_creates_an_owner_only_config() {
    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");
    assert!(
        !path.exists(),
        "precondition: the config does not exist yet"
    );

    reg.add(folder_entry("src_1")).unwrap();

    let mode = mode_of(&path);
    assert_eq!(
        mode & 0o077,
        0,
        "a freshly created config.toml must not be group- or world-accessible, got {mode:o}"
    );
}

#[cfg(unix)]
#[test]
fn a_mutation_does_not_widen_an_owner_only_config() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");

    // Stand in for a host that wrote the config itself and hardened it, which
    // is exactly what OpenHuman's `Config::save` does.
    std::fs::write(&path, "[some_other_section]\nkept = true\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    reg.add(folder_entry("src_1")).unwrap();

    let mode = mode_of(&path);
    assert_eq!(
        mode & 0o077,
        0,
        "a source mutation must not re-widen a config the host hardened, got {mode:o}"
    );
    // The whole point of the preserving re-read: unrelated keys survive.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("[some_other_section]"),
        "unrelated config sections must survive the rewrite"
    );
}

#[cfg(unix)]
#[test]
fn a_mutation_narrows_a_config_that_was_already_world_readable() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");

    // A config left at 0644 by an older build. The mode under test belongs to
    // the temp file, not to this one, so the pre-existing width must not be
    // inherited through the rename.
    std::fs::write(&path, "[some_other_section]\nkept = true\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        mode_of(&path) & 0o077,
        0o044,
        "precondition: starts at 0644"
    );

    reg.add(folder_entry("src_1")).unwrap();

    let mode = mode_of(&path);
    assert_eq!(
        mode & 0o077,
        0,
        "the rename must not carry the old file's 0644 onto the new one, got {mode:o}"
    );
}

#[cfg(unix)]
#[test]
fn every_mutation_path_leaves_the_config_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");

    reg.add(folder_entry("src_1")).unwrap();
    reg.add(folder_entry("src_2")).unwrap();

    // Re-widen between mutations so each assertion is about the write that
    // follows it rather than about a mode set once at creation.
    let widen = |p: &std::path::Path| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap()
    };

    widen(&path);
    reg.update(
        "src_1",
        MemorySourcePatch {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(mode_of(&path) & 0o077, 0, "update() widened the config");

    widen(&path);
    assert!(reg.remove("src_2").unwrap());
    assert_eq!(mode_of(&path) & 0o077, 0, "remove() widened the config");
}

// ── apply_kind_defaults ─────────────────────────────────────────────────────
//
// Moved here from the engine crate in #5560 so a host can fill a new entry's
// caps without linking the engine. The defaults are the ones the retroactive
// Composio caps migration also applies, so any change here is a change to what
// already-registered sources are reconciled against.

fn entry_of_kind(kind: SourceKind) -> MemorySourceEntry {
    let mut entry = folder_entry("defaults");
    entry.kind = kind;
    entry
}

#[test]
fn github_defaults_fill_only_the_caps_left_unset() {
    let mut entry = entry_of_kind(SourceKind::GithubRepo);
    entry.max_issues = Some(3);
    apply_kind_defaults(&mut entry);
    assert_eq!(entry.max_prs, Some(10));
    assert_eq!(entry.max_issues, Some(3), "a user-set cap must survive");
    assert_eq!(entry.max_commits, Some(50));
}

#[test]
fn an_rss_feed_gets_an_item_cap() {
    let mut entry = entry_of_kind(SourceKind::RssFeed);
    apply_kind_defaults(&mut entry);
    assert_eq!(entry.max_items, Some(20));
}

#[test]
fn kinds_with_no_defaults_are_left_alone() {
    // Composio caps come from the toolkit slug at upsert time, which this
    // function does not have; folders and web pages have no caps at all.
    for kind in [
        SourceKind::Composio,
        SourceKind::Conversation,
        SourceKind::Folder,
        SourceKind::File,
        SourceKind::WebPage,
    ] {
        let mut entry = entry_of_kind(kind.clone());
        apply_kind_defaults(&mut entry);
        assert!(entry.max_items.is_none(), "{kind:?} gained an item cap");
        assert!(
            entry.max_prs.is_none(),
            "{kind:?} gained a pull-request cap"
        );
    }
}

#[test]
fn applying_the_defaults_twice_changes_nothing() {
    let mut once = entry_of_kind(SourceKind::GithubRepo);
    apply_kind_defaults(&mut once);
    let mut twice = once.clone();
    apply_kind_defaults(&mut twice);
    assert_eq!(twice.max_prs, once.max_prs);
    assert_eq!(twice.max_issues, once.max_issues);
    assert_eq!(twice.max_commits, once.max_commits);
}
