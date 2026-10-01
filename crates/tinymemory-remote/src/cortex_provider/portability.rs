//! Hosted export and import that stay linear in the size of the account.
//!
//! The mandatory implementations in `tinymemory_api::mandatory` compose export
//! from `namespace_summaries` and `list`, and import from one keyed store per
//! record. Over the hosted wire both are expensive in exactly the way a
//! migration notices, because every request is billed and a user gets 300 a
//! minute:
//!
//! - `namespace_summaries` folds every scope in the account, and the mandatory
//!   export asks for it on every page. With a namespace per ingested document,
//!   that is a walk of the whole account per page — quadratic in namespaces.
//! - a keyed store waits for its own event to become readable, polling the
//!   listing and then probing ranked recall, so an import pays several requests
//!   per record.
//!
//! Hosted mode therefore exports from the adapter's own scope listing (one
//! request per page, plus that namespace's events) and imports by appending
//! each record, then waiting once per scope for the last event it wrote — the
//! log is ordered, so that event becoming listable implies the earlier ones
//! are. The records and the cursor format are the mandatory ones.
//!
//! An append is a new version even when the record is already there, so an
//! import run again would write every record twice. Before a batch first
//! writes to a namespace, it reads what the namespace holds, with the fold
//! export reads, and skips each record held unchanged. A copy run again writes
//! only what changed since. A first copy pays one listing per namespace in
//! each batch, which answers empty.

use std::collections::{BTreeMap, HashMap};

use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::{engine_error, read_record, to_record};
use tinymemory_api::provider::types::{ExportPage, ExportRecord, ImportOutcome};

use crate::common::{Dialect, StoredEntry};
use crate::cortex::AppendedEvent;
use crate::hosted::error_code;

use super::CortexProvider;

/// Most failure reasons one import outcome keeps; they are logged.
const MAX_IMPORT_ERRORS: usize = 20;

/// Pauses between attempts at one record while the backend keeps answering
/// that it cannot serve right now. Together about a minute: one rate-limit
/// window, after which the outage is real and the batch fails.
pub(super) const IMPORT_PATIENCE: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(40),
];

/// Why one record was not imported.
enum RecordFault {
    /// The backend refused this record; the rest of the batch can go on.
    Refused(String),
    /// A failure that makes the whole batch meaningless.
    Batch(MemoryError),
}

impl CortexProvider {
    /// `export_page` for the hosted wire. See the module docs.
    pub(super) async fn hosted_export_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ExportPage, MemoryError> {
        if limit == 0 {
            return Err(MemoryError::Invalid(
                "export page limit must be greater than zero".to_string(),
            ));
        }
        let (mut index, mut offset) = parse_cursor(cursor)?;
        let mut namespaces = self.dialect.scopes().await.map_err(engine_error)?;
        // Sorted so the cursor's namespace index means the same thing on
        // every page, whatever order the engine lists scopes in.
        namespaces.sort();
        namespaces.dedup();
        if index >= namespaces.len() {
            // A start-of-export against an empty account lands here
            // legitimately; any other out-of-range index came from a cursor
            // this driver did not issue.
            if cursor.is_some() && !namespaces.is_empty() {
                return Err(MemoryError::Invalid(format!(
                    "export cursor names namespace #{index}, but this driver holds {}",
                    namespaces.len()
                )));
            }
            return Ok(ExportPage {
                records: Vec::new(),
                next_cursor: None,
            });
        }

        loop {
            let entries = self
                .dialect
                .namespace_entries(&namespaces[index])
                .await
                .map_err(engine_error)?;
            if offset > entries.len() {
                return Err(MemoryError::Invalid(format!(
                    "export cursor offset {offset} is past the end of namespace #{index}"
                )));
            }
            // A scope whose every key was forgotten folds to nothing. The
            // mandatory export never meets one (summaries only name namespaces
            // with records); stepping over it here keeps an empty page from
            // carrying a cursor, which a caller could not tell from a stall.
            if entries.is_empty() && index + 1 < namespaces.len() {
                index += 1;
                offset = 0;
                continue;
            }
            let end = offset.saturating_add(limit).min(entries.len());
            let records = entries[offset..end]
                .iter()
                .cloned()
                .map(|entry| to_record(entry.into_memory_entry()))
                .collect();
            let next_cursor = if end < entries.len() {
                Some(format!("{index}:{end}"))
            } else if index + 1 < namespaces.len() {
                Some(format!("{}:0", index + 1))
            } else {
                None
            };
            return Ok(ExportPage {
                records,
                next_cursor,
            });
        }
    }

    /// `import_records` for the hosted wire. See the module docs.
    ///
    /// A record the namespace already holds with the same content, category,
    /// session and taint is counted in [`ImportOutcome::skipped`] and not
    /// written. A record the backend refuses (a 400-class answer, or a
    /// namespace that cannot be a hosted scope) is counted in
    /// [`ImportOutcome::failed`] with a reason that names the record and the
    /// backend's code, never its content.
    /// A backend that stays unavailable through every import pause, or that
    /// refuses the credential or the credit balance, fails the batch:
    /// continuing would only fail every remaining record the same way.
    pub(super) async fn hosted_import_records(
        &self,
        records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError> {
        let mut outcome = ImportOutcome::default();
        // The last event appended to each scope: the one to wait for.
        let mut last: BTreeMap<String, AppendedEvent> = BTreeMap::new();
        // What each namespace holds, by key, read before its first write and
        // kept current with what this batch writes.
        let mut held: HashMap<String, HashMap<String, StoredEntry>> = HashMap::new();
        for record in records {
            let entry = match read_record(&record) {
                Ok(entry) => entry,
                Err(reason) => {
                    note_failure(&mut outcome, reason);
                    continue;
                }
            };
            let stored = StoredEntry::new(
                &entry.namespace,
                &entry.key,
                &entry.content,
                entry.category,
                entry.session_id.as_deref(),
                record.taint,
            );
            if !held.contains_key(&entry.namespace) {
                let entries = self.held_patiently(&entry.namespace).await?;
                held.insert(entry.namespace.clone(), entries);
            }
            let namespace = held.entry(entry.namespace.clone()).or_default();
            if namespace
                .get(&stored.key)
                .is_some_and(|current| unchanged(current, &stored))
            {
                outcome.skipped = outcome.skipped.saturating_add(1);
                continue;
            }
            match self.append_patiently(&stored).await {
                Ok(appended) => {
                    outcome.imported = outcome.imported.saturating_add(1);
                    if let Some(event) = appended {
                        last.insert(event.scope.clone(), event);
                    }
                    namespace.insert(stored.key.clone(), stored);
                }
                Err(RecordFault::Refused(why)) => {
                    note_failure(&mut outcome, format!("record {}: {why}", record.id));
                }
                Err(RecordFault::Batch(error)) => return Err(error),
            }
        }
        for event in last.values() {
            self.dialect
                .await_listed(&event.scope, &event.id)
                .await
                .map_err(engine_error)?;
        }
        Ok(outcome)
    }

    /// What `namespace` holds now, by key, pausing between attempts while the
    /// backend says it cannot serve right now, as [`Self::append_patiently`]
    /// does.
    ///
    /// Empty for a namespace that cannot be a hosted scope: nothing is held
    /// there, and writing its records refuses them one by one.
    async fn held_patiently(
        &self,
        namespace: &str,
    ) -> Result<HashMap<String, StoredEntry>, MemoryError> {
        if self.dialect.scope_for(namespace).is_err() {
            return Ok(HashMap::new());
        }
        let mut pauses = self.import_patience.iter();
        loop {
            let error = match self.dialect.namespace_entries(namespace).await {
                Ok(entries) => {
                    return Ok(entries
                        .into_iter()
                        .map(|entry| (entry.key.clone(), entry))
                        .collect());
                }
                Err(error) => engine_error(error),
            };
            let busy = matches!(
                error,
                MemoryError::Unavailable(_) | MemoryError::Timeout(_) | MemoryError::Unreachable(_)
            );
            match pauses.next() {
                Some(pause) if busy => tokio::time::sleep(*pause).await,
                _ => return Err(error),
            }
        }
    }

    /// Appends one record, pausing between attempts while the backend says it
    /// cannot serve right now.
    ///
    /// Each attempt already retries a transient fault three times within about
    /// a second, which rides out a blip but not a rate-limit window measured in
    /// a minute. Re-sending the record under a new claim is safe: it is a keyed
    /// version, and a duplicate version folds away on read.
    async fn append_patiently(
        &self,
        entry: &StoredEntry,
    ) -> Result<Option<AppendedEvent>, RecordFault> {
        if let Err(error) = self.dialect.scope_for(&entry.namespace) {
            return Err(RecordFault::Refused(format!(
                "its namespace cannot be a hosted scope ({error})"
            )));
        }
        let mut pauses = self.import_patience.iter();
        loop {
            let error = match self.dialect.append_entry(entry).await {
                Ok(appended) => return Ok(appended),
                Err(error) => engine_error(error),
            };
            match &error {
                MemoryError::Invalid(_) | MemoryError::NotFound(_) => {
                    let code = error_code(&error).unwrap_or("refused");
                    return Err(RecordFault::Refused(format!(
                        "the memory backend refused it ({code})"
                    )));
                }
                MemoryError::Unavailable(_)
                | MemoryError::Timeout(_)
                | MemoryError::Unreachable(_) => match pauses.next() {
                    Some(pause) => tokio::time::sleep(*pause).await,
                    None => return Err(RecordFault::Batch(error)),
                },
                _ => return Err(RecordFault::Batch(error)),
            }
        }
    }
}

/// Whether `held` already is `wanted`: the same content, category, session and
/// taint. Its id and timestamp are the write's, not the record's.
fn unchanged(held: &StoredEntry, wanted: &StoredEntry) -> bool {
    held.content == wanted.content
        && held.category == wanted.category
        && held.session_id == wanted.session_id
        && held.taint == wanted.taint
}

/// Counts one failed record and keeps a bounded number of reasons.
fn note_failure(outcome: &mut ImportOutcome, reason: String) {
    outcome.failed = outcome.failed.saturating_add(1);
    if outcome.errors.len() < MAX_IMPORT_ERRORS {
        outcome.errors.push(reason);
    }
}

/// Parses the mandatory export cursor, `"{namespace_index}:{offset}"`.
///
/// `None` means "start", i.e. `(0, 0)`.
fn parse_cursor(cursor: Option<&str>) -> Result<(usize, usize), MemoryError> {
    let Some(raw) = cursor else {
        return Ok((0, 0));
    };
    let invalid =
        || MemoryError::Invalid(format!("export cursor not issued by this driver: {raw}"));
    let (index, offset) = raw.split_once(':').ok_or_else(invalid)?;
    Ok((
        index.parse().map_err(|_| invalid())?,
        offset.parse().map_err(|_| invalid())?,
    ))
}
