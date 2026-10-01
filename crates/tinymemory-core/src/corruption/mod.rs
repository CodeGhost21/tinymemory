//! One classification and one recovery path for a corrupt chunk store.
//!
//! `SQLITE_CORRUPT` used to be handled per call site, and the sites disagreed:
//! the queue worker treated it as fatal (report once, quarantine + rebuild,
//! long backoff) while the tree-ingest paths logged it at `warn` as
//! "non-fatal" and carried on — which let a malformed `chunks.db` fail every
//! ingest for 34 minutes while the sync surfaces reported success, until the
//! job-claim path finally hit the same damage and quarantined the file
//! (openhuman#5820). This module is the single answer both kinds of site call:
//! [`is_sqlite_corrupt`] to classify, [`report_and_recover`] to escalate.
//!
//! Recovery is deliberately the queue worker's proven sequence: report to the
//! host once per corruption episode (process-wide latch), mark the tree
//! degraded so status surfaces stop reading healthy, quarantine + rebuild via
//! [`recover_corrupt_db`](crate::store::chunks::store::recover_corrupt_db),
//! and announce the outcome as a [`MemoryEvent`] so a host can tell the user
//! what happened and where the quarantined file is.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::events::{self, MemoryEvent};
use crate::tree::health::{clear_storage_degraded, mark_storage_degraded, FailureCode};
use crate::Config;

/// Process-wide latch so a `SQLITE_CORRUPT` flood is reported to the host
/// **once** per corruption episode, not once per failing call. One corrupt
/// file fails every ingest and every queue poll until recovery settles, so
/// without the latch a single episode pages hundreds of times (Sentry
/// TAURI-RUST-E93: ~1.6k events in ~17 min from one host). Cleared when a
/// recovery attempt settles (quarantine + rebuild, or a quick_check that now
/// passes) so a genuinely-new, later corruption can page again.
static CORRUPT_REPORTED: AtomicBool = AtomicBool::new(false);

/// Classify whether an error is a `SQLITE_CORRUPT` malformed-image condition
/// (primary code `DatabaseCorrupt`, code 11) or the closely-related
/// `NotADatabase` (code 26 — the header itself is unreadable).
///
/// Unlike busy/locked, the transient I/O family, or `SQLITE_FULL`, a malformed
/// image is **persistent on-disk damage**: no retry of the failing call can
/// ever succeed, so callers must escalate through [`report_and_recover`]
/// rather than logging and continuing.
///
/// Matching on the error code is rusqlite-version-stable and, because anyhow
/// downcasts through `context` layers, survives wrapping. The text fallback
/// covers the case where the rusqlite error was flattened into a plain
/// `anyhow!("…: {error}")` string at a module boundary — SQLite renders these
/// as "database disk image is malformed" (code 11) and "file is not a
/// database" (code 26).
pub(crate) fn is_sqlite_corrupt(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(sqlite_err, _)) =
        err.downcast_ref::<rusqlite::Error>()
    {
        if matches!(
            sqlite_err.code,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
        ) {
            return true;
        }
    }
    is_corrupt_text(&format!("{err:#}"))
}

/// The text half of [`is_sqlite_corrupt`], for errors that only exist as
/// strings (a pipeline failure message, a wire error body).
pub(crate) fn is_corrupt_text(message: &str) -> bool {
    let msg = message.to_ascii_lowercase();
    msg.contains("database disk image is malformed") || msg.contains("file is not a database")
}

/// Handle a confirmed `SQLITE_CORRUPT` on the chunk store, from any path.
///
/// Reports to the host once per episode (see [`CORRUPT_REPORTED`]), marks the
/// tree storage-degraded so status surfaces read `error` instead of healthy,
/// then drives the quarantine + rebuild recovery. On a settled recovery the
/// degraded flag and the latch clear — the rebuilt store works, and the
/// durable "your memory tree was quarantined" message is the
/// [`MemoryEvent::StoreCorruptQuarantined`] this publishes, not a stuck
/// banner. A failed recovery leaves both set: the store really is unusable.
///
/// `origin` names the detecting path for logs and event payloads
/// (`"jobs worker 0"`, `"composio tree ingest"`, `"startup integrity check"`);
/// `report_key` is the host-facing operation tag, kept caller-chosen so the
/// queue worker's long-standing `tree_jobs_worker_corrupt` Sentry grouping
/// survives the consolidation.
pub(crate) fn report_and_recover(
    origin: &str,
    report_key: &str,
    err: &anyhow::Error,
    config: &Config,
) {
    if !CORRUPT_REPORTED.swap(true, Ordering::Relaxed) {
        crate::observability::report_error(err, "memory", report_key, &[("origin", origin)]);
    }
    mark_storage_degraded(FailureCode::StorageUnavailable);
    log::error!(
        "[memory:corruption] {origin} hit SQLITE_CORRUPT (malformed chunk DB image), \
         attempting quarantine + rebuild recovery: {err:#}"
    );
    match crate::store::chunks::store::recover_corrupt_db(config) {
        Ok(true) => {
            let quarantined = latest_quarantined_path(config);
            match quarantined.as_deref() {
                Some(path) => log::error!(
                    "[memory:corruption] {origin}: quarantined corrupt mem_tree DB to \
                     {path} and rebuilt an empty schema. The quarantined file is preserved, \
                     not deleted; previously ingested sources must re-sync to repopulate \
                     the tree",
                    path = path.display()
                ),
                None => log::error!(
                    "[memory:corruption] {origin}: quarantined corrupt mem_tree DB and \
                     rebuilt an empty schema; previously ingested sources must re-sync"
                ),
            }
            events::publish(MemoryEvent::StoreCorruptQuarantined {
                origin: origin.to_string(),
                quarantined_path: quarantined.map(|path| path.display().to_string()),
            });
            // Recovery settled: the rebuilt store is usable again, so the
            // degraded flag must not outlive the damage, and a future,
            // genuinely-new corruption may page once more.
            clear_storage_degraded();
            CORRUPT_REPORTED.store(false, Ordering::Relaxed);
        }
        Ok(false) => {
            log::info!(
                "[memory:corruption] {origin}: corruption recovery ran but quick_check \
                 now passes; no quarantine needed"
            );
            clear_storage_degraded();
            CORRUPT_REPORTED.store(false, Ordering::Relaxed);
        }
        Err(rec_err) => {
            log::error!(
                "[memory:corruption] {origin}: corruption recovery FAILED, store stays \
                 degraded: {rec_err:#}"
            );
        }
    }
}

/// The tree-ingest sinks' shared error policy: escalate corruption, count and
/// tolerate everything else (openhuman#5820).
///
/// `Ok(())` means the failure was tolerated — logged by the caller, recorded
/// in `counter` for the run's verdict, sync continues. `Err` means the store
/// is corrupt: the shared recovery has run and the caller must abort its run,
/// because every later item fails identically against a malformed image.
/// Lives here rather than on each sink so `PipelineHost` and
/// `HostSyncAdapter` cannot drift apart on the classification again — the
/// drift IS the incident this module exists for.
pub(crate) fn escalate_or_count(
    origin: &str,
    config: &Config,
    error: anyhow::Error,
    counter: &std::sync::atomic::AtomicU32,
) -> anyhow::Result<()> {
    if is_sqlite_corrupt(&error) {
        report_and_recover(origin, "tree_ingest_corrupt", &error, config);
        return Err(error.context(
            "memory-tree store is corrupt; aborting this sync run \
             (the store was quarantined and rebuilt — re-sync to repopulate)",
        ));
    }
    counter.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// The most recent quarantined chunk-DB copy in this workspace, if any.
///
/// The quarantine renames `chunks.db` to `chunks.db.corrupt-<UTC timestamp>`
/// (`%Y%m%dT%H%M%SZ`), so the lexically greatest matching name is the newest.
/// Side files quarantine as `chunks.db-wal.corrupt-<ts>` and never match the
/// main file's prefix.
pub(crate) fn latest_quarantined_path(config: &Config) -> Option<PathBuf> {
    let dir = config.workspace_dir().join("memory_tree");
    let entries = std::fs::read_dir(&dir).ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("chunks.db.corrupt-"))
        })
        .max_by_key(std::fs::DirEntry::file_name)
        .map(|entry| entry.path())
}

/// Startup integrity check for the chunk store (openhuman#5820 item 5).
///
/// Workspaces written before the two-engines-over-one-file fix
/// (openhuman#5725) can carry latent page damage that only surfaces when some
/// later call happens to walk a damaged b-tree — in the incident, 10 hours
/// after boot, via whichever path hit it first. Running `PRAGMA
/// quick_check(1)` once at queue start moves that discovery to a defined
/// moment with a defined owner: damage found here goes straight through
/// [`report_and_recover`] instead of failing arbitrary calls first.
///
/// A missing file is healthy (first boot creates it). A failing pragma is
/// treated as corrupt only when the failure itself classifies as corruption
/// (`NotADatabase` is what a destroyed header raises) — a plain open failure
/// can be a lock or a permission problem, and quarantining on those would
/// rename a healthy file. The scan reads the whole file, so callers run this
/// on a blocking thread, off the async workers.
pub(crate) fn startup_integrity_check(config: &Config) {
    let db_path = config.workspace_dir().join("memory_tree").join("chunks.db");
    if !db_path.exists() {
        return;
    }
    let verdict = (|| -> anyhow::Result<String> {
        let conn = rusqlite::Connection::open(&db_path)?;
        let _ = conn.busy_timeout(std::time::Duration::from_secs(15));
        Ok(conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?)
    })();
    match verdict {
        Ok(result) if result.eq_ignore_ascii_case("ok") => {
            log::debug!(
                "[memory:corruption] startup quick_check passed for {}",
                db_path.display()
            );
        }
        Ok(result) => {
            let err = anyhow::anyhow!(
                "startup quick_check found a malformed chunk DB image at {}: {result}",
                db_path.display()
            );
            report_and_recover(
                "startup integrity check",
                "tree_startup_corrupt",
                &err,
                config,
            );
        }
        Err(error) => {
            let err = error.context(format!(
                "startup quick_check could not scan {}",
                db_path.display()
            ));
            if is_sqlite_corrupt(&err) {
                report_and_recover(
                    "startup integrity check",
                    "tree_startup_corrupt",
                    &err,
                    config,
                );
            } else {
                // A lock, a permission problem, a dying disk — not proven
                // corruption. Quarantining here would rename a file that may
                // be fine; leave it for the runtime classifiers to judge from
                // a real call's error.
                log::warn!(
                    "[memory:corruption] startup quick_check could not scan the chunk DB \
                     (not classified as corruption, leaving the file in place): {err:#}"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
