use super::*;

#[test]
fn normalize_key_replaces_non_alnum() {
    assert_eq!(normalize_key("hello/world", 0), "hello_world");
}

#[test]
fn normalize_key_trims_edges_and_numbers_blank_keys() {
    assert_eq!(normalize_key("  /a b/  ", 3), "a_b");
    assert_eq!(normalize_key("   ", 7), "openclaw_7");
}

#[test]
fn parse_category_maps_known_names_and_keeps_unknown_ones_custom() {
    assert_eq!(parse_category(" Core "), MemoryCategory::Core);
    assert_eq!(parse_category("daily"), MemoryCategory::Daily);
    assert_eq!(parse_category("conversation"), MemoryCategory::Conversation);
    assert_eq!(
        parse_category("project"),
        MemoryCategory::Custom("project".to_string())
    );
    assert_eq!(
        parse_category("unknown"),
        MemoryCategory::Custom("unknown".to_string())
    );
}

#[test]
fn paths_equal_resolves_symlink_free_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    let direct = tmp.path().to_path_buf();
    let dotted = tmp.path().join(".");
    assert!(paths_equal(&direct, &dotted));
    assert!(!paths_equal(&direct, &tmp.path().join("other")));
}

#[test]
fn backup_is_skipped_when_the_target_holds_no_memory() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(backup_target_memory(tmp.path()).unwrap().is_none());
}

#[test]
fn backup_copies_markdown_and_database_files() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("memory")).unwrap();
    fs::write(tmp.path().join("MEMORY.md"), "top").unwrap();
    fs::write(tmp.path().join("memory").join("note.md"), "note").unwrap();
    fs::write(tmp.path().join("memory").join("brain.db"), "db").unwrap();
    fs::write(tmp.path().join("memory").join("skip.txt"), "skip").unwrap();

    let dir = backup_target_memory(tmp.path()).unwrap().unwrap();
    assert_eq!(dir, tmp.path().join("memory_backup"));
    assert_eq!(fs::read_to_string(dir.join("MEMORY.md")).unwrap(), "top");
    assert_eq!(fs::read_to_string(dir.join("brain.db")).unwrap(), "db");
    assert_eq!(
        fs::read_to_string(dir.join("memory").join("note.md")).unwrap(),
        "note"
    );
    assert!(!dir.join("memory").join("skip.txt").exists());
}
