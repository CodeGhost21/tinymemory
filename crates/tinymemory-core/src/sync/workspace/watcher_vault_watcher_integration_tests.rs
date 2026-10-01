use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

use crate::sync::workspace::watcher::state::WatcherStateStore;

// ── helpers ───────────────────────────────────────────────────────────

fn mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ── state store ───────────────────────────────────────────────────────

#[test]
fn state_store_persists_across_reopen() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.db");
    let note = Path::new("/vault/note.md");

    {
        let mut store = WatcherStateStore::open(&db).unwrap();
        store.record_seen(note, 1_700_000_000).unwrap();
    }
    // Re-open simulates a process restart.
    {
        let store = WatcherStateStore::open(&db).unwrap();
        assert_eq!(store.last_mtime(note).unwrap(), Some(1_700_000_000));
    }
}

#[test]
fn state_store_deleted_survives_reopen() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.db");
    let note = Path::new("/vault/deleted.md");

    {
        let mut store = WatcherStateStore::open(&db).unwrap();
        store.record_seen(note, 1_000).unwrap();
        store.record_deleted(note).unwrap();
    }
    {
        let store = WatcherStateStore::open(&db).unwrap();
        // `last_mtime` returns None for deleted entries.
        assert_eq!(store.last_mtime(note).unwrap(), None);
        // But the row is still there (deleted=1).
        let rows = store.load_all().unwrap();
        assert!(rows.iter().any(|r| r.path == note && r.deleted));
    }
}

// ── mtime-guard: skip unchanged file ─────────────────────────────────

#[test]
fn mtime_guard_skips_file_with_same_mtime() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.db");
    let note = tmp.path().join("note.md");
    fs::write(&note, "initial").unwrap();

    let mtime = mtime_secs(&note);

    let mut store = WatcherStateStore::open(&db).unwrap();
    store.record_seen(&note, mtime).unwrap();

    // Simulate the in-memory cache check: stored mtime == current mtime
    // → the watcher should skip this file.
    let cached = store.last_mtime(&note).unwrap();
    assert_eq!(
        cached,
        Some(mtime),
        "cache should report the file as already seen at this mtime"
    );
}

// ── mtime-guard: process changed file ────────────────────────────────

#[test]
fn mtime_guard_processes_file_with_new_mtime() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.db");
    let note = tmp.path().join("note.md");

    fs::write(&note, "version 1").unwrap();
    let mtime_v1 = mtime_secs(&note);

    let mut store = WatcherStateStore::open(&db).unwrap();
    store.record_seen(&note, mtime_v1).unwrap();

    // Simulate a file edit: write new content and sleep 1s to bump mtime.
    // On most filesystems 1-second granularity is the minimum resolution.
    std::thread::sleep(Duration::from_secs(1));
    fs::write(&note, "version 2").unwrap();
    let mtime_v2 = mtime_secs(&note);

    assert_ne!(
        mtime_v1, mtime_v2,
        "mtime must advance when file is modified"
    );

    // The cache holds mtime_v1; current file has mtime_v2 → watcher proceeds.
    let cached = store.last_mtime(&note).unwrap().unwrap();
    assert!(
        mtime_v2 > cached,
        "new mtime should be greater than cached mtime"
    );
}

// ── source_id format ─────────────────────────────────────────────────

#[test]
fn source_id_is_stable_and_version_scoped() {
    let rel = "journal/2024-01-01.md";
    let mtime: u64 = 1_700_000_000;

    let id_v1 = format!("vault_watcher:{rel}@{mtime}");
    let id_v2 = format!("vault_watcher:{rel}@{}", mtime + 1);

    // Same path, different mtime → different source_id → bypasses dedup.
    assert_ne!(id_v1, id_v2);

    // Stable format — the ingest pipeline stores this as the document key.
    assert_eq!(id_v1, "vault_watcher:journal/2024-01-01.md@1700000000");
}

// ── extension filter ─────────────────────────────────────────────────

#[test]
fn extension_filter_accepts_md_and_txt_only() {
    let accepted = ["note.md", "draft.txt"];
    let rejected = ["image.png", "data.json", "script.js", "Makefile"];

    let is_watched = |name: &str| {
        std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| ["md", "txt"].contains(&e))
            .unwrap_or(false)
    };

    for name in accepted {
        assert!(is_watched(name), "{name} should be watched");
    }
    for name in rejected {
        assert!(!is_watched(name), "{name} should not be watched");
    }
}

// ── multiple files: only changed one gets ingested ───────────────────

#[test]
fn only_changed_file_gets_new_source_id() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.db");

    let file_a = tmp.path().join("a.md");
    let file_b = tmp.path().join("b.md");
    fs::write(&file_a, "content a").unwrap();
    fs::write(&file_b, "content b").unwrap();

    let mtime_a = mtime_secs(&file_a);
    let mtime_b = mtime_secs(&file_b);

    let mut store = WatcherStateStore::open(&db).unwrap();
    store.record_seen(&file_a, mtime_a).unwrap();
    store.record_seen(&file_b, mtime_b).unwrap();

    // Modify only file_b.
    std::thread::sleep(Duration::from_secs(1));
    fs::write(&file_b, "content b v2").unwrap();
    let new_mtime_b = mtime_secs(&file_b);

    // file_a: cached == current → skip.
    let cached_a = store.last_mtime(&file_a).unwrap().unwrap();
    assert_eq!(cached_a, mtime_a, "file_a should still be cached at v1");

    // file_b: cached < current → process.
    let cached_b = store.last_mtime(&file_b).unwrap().unwrap();
    assert!(
        new_mtime_b > cached_b,
        "file_b new mtime should exceed cached mtime"
    );
}
