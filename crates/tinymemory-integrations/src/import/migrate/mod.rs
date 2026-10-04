//! [`migrate`]: copy a legacy workspace into an engine, resumably.
//!
//! The driver streams [`LegacyWorkspace::items_from`] in batches of at most
//! [`MAX_STORE_MANY`] items and hands each to
//! [`MemoryEngine::store_many`]. After a batch is stored, the checkpoint of its
//! last item is *committed*: it is reported to the caller's callback
//! ([`migrate_with`]) and becomes the resume point.
//!
//! A failure never loses progress:
//!
//! - an engine failure is [`Error::Engine`], carrying the last committed
//!   checkpoint, so the host resumes from exactly there;
//! - a failed batch may have stored some of its items (`store_many` stores in
//!   order and stops at the error), and resuming re-sends them, which the
//!   engine answers as replays, not duplicates;
//! - a legacy read failure is returned as is; the callback has already seen
//!   every committed checkpoint, and resuming from any earlier one (or the
//!   start) only replays.
//!
//! The workspace is taken by value so the returned future is `Send` (the
//! legacy store's SQLite handle is not `Sync`), and a host can run a long
//! import on a spawned task.

use tinymemory_api::{MAX_STORE_MANY, MemoryEngine};

use crate::import::checkpoint::Checkpoint;
use crate::import::error::{Error, Result};
use crate::import::workspace::LegacyWorkspace;

/// What a [`migrate`] run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Items the engine stored for the first time.
    pub stored: usize,
    /// Items the engine already held (a re-run, or a resumed batch).
    pub replayed: usize,
    /// `store_many` calls made.
    pub batches: usize,
    /// The resume point after the last stored batch: the checkpoint the run
    /// started from when nothing was left to store.
    pub checkpoint: Checkpoint,
}

/// Stores every item of `workspace` after `from` (everything, for `None`)
/// into `engine`, in batches of at most [`MAX_STORE_MANY`].
///
/// # Errors
///
/// - [`Error::Engine`] when `store_many` fails, carrying the checkpoint of
///   the last stored batch (or `from`) to resume from.
/// - [`Error::Sqlite`] or [`Error::Io`] when the legacy store cannot be read.
pub async fn migrate(
    engine: &dyn MemoryEngine,
    workspace: LegacyWorkspace,
    from: Option<Checkpoint>,
) -> Result<MigrationReport> {
    migrate_with(engine, workspace, from, |_: &Checkpoint| {}).await
}

/// [`migrate`], calling `on_batch` with the committed checkpoint after each
/// stored batch so the host can persist it (with
/// [`Checkpoint::to_json`]) as it goes.
///
/// # Errors
///
/// As [`migrate`].
pub async fn migrate_with<F>(
    engine: &dyn MemoryEngine,
    workspace: LegacyWorkspace,
    from: Option<Checkpoint>,
    mut on_batch: F,
) -> Result<MigrationReport>
where
    F: FnMut(&Checkpoint) + Send,
{
    let mut report = MigrationReport {
        checkpoint: from.unwrap_or_default(),
        ..MigrationReport::default()
    };
    loop {
        // Read the batch before awaiting: the iterator borrows the
        // workspace, whose SQLite handle must not be held across an await.
        let mut items = Vec::with_capacity(MAX_STORE_MANY);
        let mut last = None;
        for imported in workspace
            .items_from(&report.checkpoint)
            .take(MAX_STORE_MANY)
        {
            let imported = imported?;
            items.push(imported.item);
            last = Some(imported.checkpoint);
        }
        let Some(last) = last else {
            return Ok(report);
        };
        let receipts = engine
            .store_many(items)
            .await
            .map_err(|source| Error::Engine {
                source,
                checkpoint: report.checkpoint.clone(),
            })?;
        let replayed = receipts.iter().filter(|receipt| receipt.replayed).count();
        report.replayed += replayed;
        report.stored += receipts.len() - replayed;
        report.batches += 1;
        report.checkpoint = last;
        on_batch(&report.checkpoint);
    }
}
