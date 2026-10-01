//! Tests for the surrounding module.
//!
//! The classifier table moved here with `is_sqlite_corrupt` (it grew up in
//! `queue::worker` for #4048 / Sentry TAURI-RUST-E93); the recovery tests
//! exercise the shared `report_and_recover` every detecting path now calls.

use super::*;
use crate::events::MemoryEvent;
use tempfile::TempDir;
use tinymemory_api::host::test_support::TestHostConfig;

fn test_config() -> (TempDir, TestHostConfig) {
    crate::test_seams::init();
    let tmp = TempDir::new().unwrap();
    let mut cfg = TestHostConfig::default();
    cfg.workspace_dir = tmp.path().to_path_buf();
    cfg.memory_tree.embedding_endpoint = None;
    cfg.memory_tree.embedding_model = None;
    cfg.memory_tree.embedding_strict = false;
    (tmp, cfg)
}

// ── is_sqlite_corrupt (#4048 / Sentry TAURI-RUST-E93) ────────────────────

/// `SQLITE_CORRUPT` (primary code `DatabaseCorrupt`, code 11) is the
/// malformed-image signal; it must classify so detectors escalate through
/// quarantine + rebuild instead of retrying or paging forever.
#[test]
fn is_sqlite_corrupt_matches_database_corrupt_code() {
    let raw = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseCorrupt,
            extended_code: 11,
        },
        Some("database disk image is malformed".into()),
    );
    assert!(is_sqlite_corrupt(&anyhow::Error::from(raw)));
}

/// `SQLITE_NOTADB` (code `NotADatabase`, 26 — header unreadable) is the
/// same broad on-disk-damage class and must classify too.
#[test]
fn is_sqlite_corrupt_matches_not_a_database_code() {
    let raw = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::NotADatabase,
            extended_code: 26,
        },
        Some("file is not a database".into()),
    );
    assert!(is_sqlite_corrupt(&anyhow::Error::from(raw)));
}

/// The rusqlite error sits a few `.context()` layers deep when it bubbles
/// out of `claim_next` → `with_connection`; the downcast must still find
/// the `DatabaseCorrupt` code.
#[test]
fn is_sqlite_corrupt_matches_through_context_layers() {
    let raw = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseCorrupt,
            extended_code: 11,
        },
        Some("database disk image is malformed".into()),
    );
    let wrapped = anyhow::Error::from(raw)
        .context("Failed to claim next mem_tree_jobs row")
        .context("with_connection closure failed");
    assert!(is_sqlite_corrupt(&wrapped));
}

/// Text fallback: the exact flattened Sentry string (TAURI-RUST-E93) must
/// classify even when no rusqlite error is available to downcast.
#[test]
fn is_sqlite_corrupt_text_fallback() {
    let err = anyhow::anyhow!(
        "Failed to claim next mem_tree_jobs row: database disk image is malformed: \
         Error code 11: The database disk image is malformed"
    );
    assert!(is_sqlite_corrupt(&err));
}

/// The tree-ingest boundary flattens the engine error into a plain
/// `anyhow!("memory-tree ingest failed for source `…`: {error}")` string —
/// the exact shape of the openhuman#5820 incident's 747 warns. The classifier
/// must see through that flattening, because this path is why corruption ran
/// as "non-fatal" for 34 minutes.
#[test]
fn is_sqlite_corrupt_matches_the_flattened_ingest_shape() {
    let err = anyhow::anyhow!(
        "memory-tree ingest failed for source `github:owner/repo:42`: \
         database disk image is malformed"
    );
    assert!(is_sqlite_corrupt(&err));
}

/// Busy/locked, disk-full, constraint violations, and unrelated errors must
/// NOT be swallowed as corruption — quarantining on those would destroy a
/// perfectly good DB.
#[test]
fn is_sqlite_corrupt_does_not_match_other_errors() {
    let busy = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        Some("database is locked".into()),
    );
    assert!(!is_sqlite_corrupt(&anyhow::Error::from(busy)));

    let disk_full = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DiskFull,
            extended_code: 13,
        },
        Some("database or disk is full".into()),
    );
    assert!(!is_sqlite_corrupt(&anyhow::Error::from(disk_full)));

    let constraint = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::ConstraintViolation,
            extended_code: 19,
        },
        Some("UNIQUE constraint failed: mem_tree_jobs.dedupe_key".into()),
    );
    assert!(!is_sqlite_corrupt(&anyhow::Error::from(constraint)));

    assert!(!is_sqlite_corrupt(&anyhow::anyhow!(
        "upstream returned 500: internal server error"
    )));
}

/// The string half classifies the same two SQLite phrases, for errors that
/// only exist as text (pipeline failure messages, wire error bodies).
#[test]
fn is_corrupt_text_matches_both_phrases_and_nothing_else() {
    assert!(is_corrupt_text(
        "composio sync failed: database disk image is malformed"
    ));
    assert!(is_corrupt_text("open failed: File is NOT a Database"));
    assert!(!is_corrupt_text("database or disk is full"));
    assert!(!is_corrupt_text("connection refused"));
}

// ── report_and_recover ───────────────────────────────────────────────────

/// The shared recovery must quarantine a malformed image, rebuild an empty
/// queryable schema, publish `StoreCorruptQuarantined` naming the quarantined
/// file, and clear the storage degradation once recovery settles — exercising
/// the path every detector (worker, ingest, startup) now runs.
#[tokio::test]
async fn report_and_recover_quarantines_rebuilds_and_announces() {
    let (_tmp, cfg) = test_config();
    let sink = crate::events::RecordingSink::install();
    // Lay down a malformed `chunks.db` (garbage header) at the canonical path.
    let db_path = cfg.workspace_dir.join("memory_tree").join("chunks.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    std::fs::write(&db_path, b"not a sqlite database, just garbage bytes").unwrap();

    let err =
        anyhow::anyhow!("Failed to claim next mem_tree_jobs row: database disk image is malformed");
    report_and_recover("jobs worker 0", "tree_jobs_worker_corrupt", &err, &cfg);

    // Corrupt bytes are preserved alongside (never silently dropped) ...
    let quarantined = latest_quarantined_path(&cfg).expect("quarantined copy exists");
    assert!(quarantined
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("chunks.db.corrupt-"));

    // ... the event names that file so a host can tell the user ...
    let events = sink.drain();
    let announced = events.iter().any(|event| {
        matches!(
            event,
            MemoryEvent::StoreCorruptQuarantined { origin, quarantined_path }
                if origin == "jobs worker 0"
                    && quarantined_path.as_deref()
                        == Some(quarantined.display().to_string().as_str())
        )
    });
    assert!(
        announced,
        "StoreCorruptQuarantined must be published with the quarantined path; got {events:?}"
    );

    // ... recovery settled, so the storage degradation does not outlive it ...
    assert!(
        !crate::tree::health::current_degraded_state().storage,
        "a settled recovery must clear the storage degradation"
    );

    // ... and the rebuilt queue DB is healthy and empty.
    let processed = crate::queue::worker::run_once(&cfg).await.unwrap();
    assert!(!processed, "rebuilt queue starts empty");
}

/// `latest_quarantined_path` picks the newest timestamped copy and ignores
/// side-file quarantines (`chunks.db-wal.corrupt-…`).
#[test]
fn latest_quarantined_path_picks_newest_main_copy() {
    let (_tmp, cfg) = test_config();
    let dir = cfg.workspace_dir.join("memory_tree");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(latest_quarantined_path(&cfg).is_none());
    std::fs::write(dir.join("chunks.db.corrupt-20260101T000000Z"), b"old").unwrap();
    std::fs::write(dir.join("chunks.db.corrupt-20260827T120000Z"), b"new").unwrap();
    std::fs::write(dir.join("chunks.db-wal.corrupt-20261231T235959Z"), b"wal").unwrap();
    let newest = latest_quarantined_path(&cfg).expect("a main quarantined copy");
    assert_eq!(
        newest.file_name().unwrap().to_string_lossy(),
        "chunks.db.corrupt-20260827T120000Z"
    );
}

// ── startup_integrity_check (openhuman#5820 item 5) ──────────────────────

/// A garbage `chunks.db` found at startup is quarantined immediately instead
/// of surfacing hours later through whichever call walks the damage first.
#[test]
fn startup_integrity_check_quarantines_a_corrupt_db() {
    let (_tmp, cfg) = test_config();
    let db_path = cfg.workspace_dir.join("memory_tree").join("chunks.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    std::fs::write(&db_path, b"not a sqlite database, just garbage bytes").unwrap();

    startup_integrity_check(&cfg);

    assert!(
        latest_quarantined_path(&cfg).is_some(),
        "startup check must quarantine a corrupt image"
    );
}

/// A healthy DB passes untouched, and a missing DB (first boot) is a no-op —
/// the check must never quarantine what it cannot prove corrupt.
#[test]
fn startup_integrity_check_leaves_healthy_and_missing_dbs_alone() {
    let (_tmp, cfg) = test_config();
    // Missing: no-op.
    startup_integrity_check(&cfg);
    assert!(latest_quarantined_path(&cfg).is_none());

    // Healthy: create a real empty SQLite DB, check, and expect it in place.
    let db_path = cfg.workspace_dir.join("memory_tree").join("chunks.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE probe (id INTEGER PRIMARY KEY);")
        .unwrap();
    drop(conn);

    startup_integrity_check(&cfg);

    assert!(db_path.exists(), "healthy DB must stay in place");
    assert!(latest_quarantined_path(&cfg).is_none());
}

// ── escalate_or_count (the tree-ingest sinks' shared arm) ────────────────

/// Non-corrupt failures are tolerated and counted; corruption aborts with the
/// recovery run and does NOT count — the two sinks (`PipelineHost`,
/// `HostSyncAdapter`) share this arm precisely so they cannot disagree again.
#[test]
fn escalate_or_count_splits_corrupt_from_tolerated() {
    let (_tmp, cfg) = test_config();
    let counter = std::sync::atomic::AtomicU32::new(0);

    // Tolerated: an ordinary ingest failure returns Ok and increments.
    let plain = anyhow::anyhow!("memory-tree ingest failed for source `x`: no such directory");
    assert!(escalate_or_count("test ingest", &cfg, plain, &counter).is_ok());
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 1);

    // Corrupt: the flattened incident shape returns Err and does not count.
    let corrupt = anyhow::anyhow!(
        "memory-tree ingest failed for source `github:o/r:42`: database disk image is malformed"
    );
    let err = escalate_or_count("test ingest", &cfg, corrupt, &counter)
        .expect_err("corruption must abort the run");
    assert!(
        format!("{err:#}").contains("corrupt"),
        "the abort must say why: {err:#}"
    );
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "corruption is fatal, not a tolerated count"
    );
}
