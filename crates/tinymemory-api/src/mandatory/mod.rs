//! The three mandatory capability families, composed over the storage trait.
//!
//! [`MemoryCore`](crate::provider::MemoryCore), [`MemoryRecall`](crate::provider::MemoryRecall) and [`MemoryPortability`](crate::provider::MemoryPortability) are supertraits of
//! [`MemoryProvider`](crate::provider::MemoryProvider): a driver
//! missing any of them cannot be constructed at all. For a backend that already
//! implements [`Memory`](crate::traits::Memory), almost all three are mechanical — and the parts that
//! are *not* mechanical are the parts every such backend gets wrong the same
//! way. So they live here once rather than in each driver.
//!
//! ## The four things that are not a straight delegation
//!
//! 1. **`store` maps onto [`Memory::store_with_taint`](crate::traits::Memory::store_with_taint), never [`Memory::store`](crate::traits::Memory::store).**
//!    The contract's `store` always carries a [`MemoryTaint`](crate::types::MemoryTaint), because
//!    provenance is stamped by the host's policy layer *before* the call.
//!    [`Memory::store`](crate::traits::Memory::store) hard-codes [`MemoryTaint::Internal`](crate::types::MemoryTaint::Internal), so routing through
//!    it would launder externally-sourced content into internal-trust content —
//!    the one failure mode a provenance guard exists to prevent. Note
//!    [`Memory::store_with_taint`](crate::traits::Memory::store_with_taint)'s *trait default* also silently drops the
//!    taint, so a backend that does not override it is unsafe here; that is a
//!    backend bug, not something this layer can paper over.
//!
//! 2. **`list(None, ..)` spans every namespace.** The contract says an
//!    all-`None` list returns everything the driver holds. A typical [`Memory`](crate::traits::Memory)
//!    implementation normalises a `None` namespace to
//!    [`GLOBAL_NAMESPACE`](crate::types::GLOBAL_NAMESPACE), so a naive delegation returns one namespace and
//!    calls it "everything". [`list_everything`](crate::mandatory::list_everything) composes `namespace_summaries`
//!    with a per-namespace `list` instead.
//!
//! 3. **A scoped recall is refused, not ignored.** See [`recall`].
//!
//! 4. **Import must not re-stamp provenance.** See [`import_records`](crate::mandatory::import_records).
//!
//! ## Why free functions rather than a blanket impl
//!
//! A blanket `impl<T: SomeHandleTrait> MemoryCore for T` would collide with any
//! driver that wants to override one method, and would force every driver to
//! resolve its handle through one shape. These are plain functions taking
//! `&dyn Memory`, so a driver delegates the parts it wants and keeps its own
//! logging, laziness, and error context. [`MemoryTraitProvider`](crate::mandatory::MemoryTraitProvider) is the
//! batteries-included alternative for a backend that wants all three whole.

use crate::error::MemoryError;
use crate::provider::types::{ExportPage, ExportRecord, ImportOutcome, SourceScope};
use crate::recall::OwnedRecallOpts;
use crate::traits::Memory;
use crate::types::{MemoryCategory, MemoryEntry, RecallOpts, GLOBAL_NAMESPACE};

mod provider;

pub use provider::MemoryTraitProvider;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;

/// The [`ExportRecord::kind`] emitted and accepted by the mandatory-only
/// export.
///
/// A driver whose export widens to documents or chunks emits additional kinds
/// alongside this one; it must keep accepting this one, or an export taken
/// before the widening stops importing.
pub const ENTRY_KIND: &str = "entry";

/// Refusal message for a scoped recall on a driver with no scope predicate.
///
/// A constant so a caller's test asserts the same string the caller sees.
pub const SCOPE_UNAPPLIED: &str =
    "source scope is not applied by this driver's recall path yet: the scope predicate belongs \
     inside the query and lands with the tree capability family";

/// Maps a storage-layer `anyhow` failure onto the contract's error type.
///
/// [`Memory`] is deliberately `anyhow`-typed — it is an internal storage
/// abstraction over heterogeneous backends — so everything it returns is opaque
/// and lands in [`MemoryError::Other`]. The typed variants (`Invalid`,
/// `NotFound`, `Unsupported`) are constructed by the driver, where the reason is
/// actually known.
#[must_use]
pub fn engine_error(error: anyhow::Error) -> MemoryError {
    // §A4: an adapter that already knows the failure's class attaches a typed
    // MemoryError as the anyhow payload (crates/tinymemory-remote/src/common.rs does
    // for transport and HTTP-status failures). Recover it here instead of
    // flattening everything to Other — this one line is what makes
    // "unauthorized" distinguishable from "unreachable" across every
    // `Memory`-backed provider without touching the trait's signatures.
    error
        .downcast::<MemoryError>()
        .unwrap_or_else(MemoryError::Other)
}

/// `MemoryCore::list` for the all-namespaces case.
///
/// Call this only when the caller passed `namespace: None`; a `Some` namespace
/// delegates straight to [`Memory::list`].
///
/// # Errors
///
/// Backend failures from `namespace_summaries` or any per-namespace `list`.
pub async fn list_everything(
    memory: &dyn Memory,
    category: Option<&MemoryCategory>,
    session_id: Option<&str>,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let summaries = memory.namespace_summaries().await.map_err(engine_error)?;
    log::debug!(
        "[tinymemory:mandatory] list spanning {} namespace(s)",
        summaries.len()
    );
    let mut entries = Vec::new();
    for summary in summaries {
        let mut page = memory
            .list(Some(&summary.namespace), category, session_id)
            .await
            .map_err(engine_error)?;
        entries.append(&mut page);
    }
    Ok(entries)
}

/// `MemoryRecall::recall` over a [`Memory`] backend with no scope predicate.
///
/// The contract's `scope` is a **query predicate the driver must apply
/// internally**. Applying it after the fact would let `limit` be consumed by
/// rows the caller may not see, and an empty scope must deny all
/// source-attributed content rather than wave it through.
///
/// A [`Memory`] backend has no such predicate: `recall` ranks over a namespace
/// and consults nothing resembling a [`SourceScope`]. That leaves three
/// possible treatments of `Some(scope)`, and two are wrong — silently ignoring
/// it is an invisible leak, and post-filtering is the failure mode above. So
/// this refuses.
///
/// [`MemoryError::Invalid`] is the right variant rather than `Unsupported`:
/// `Unsupported` names a whole capability *family*, and recall is advertised.
///
/// # Errors
///
/// [`MemoryError::Invalid`] when `scope` is `Some`; otherwise backend failures.
pub async fn recall(
    memory: &dyn Memory,
    query: &str,
    limit: usize,
    opts: &OwnedRecallOpts,
    scope: Option<&SourceScope>,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    if scope.is_some() {
        log::warn!("[tinymemory:mandatory] recall refused: {SCOPE_UNAPPLIED}");
        return Err(MemoryError::Invalid(SCOPE_UNAPPLIED.to_string()));
    }

    // Zero-copy for the string filters; `RecallOpts::from` destructures
    // exhaustively inside the contract crate, so a new filter cannot be
    // dropped silently here.
    let borrowed = RecallOpts::from(opts);
    memory
        .recall(query, limit, borrowed)
        .await
        .map_err(engine_error)
}

/// Parses an export cursor of the form `"{namespace_index}:{offset}"`.
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

/// Renders one entry as an export record.
///
/// A record round-trips the five fields [`MemoryCore`](crate::provider::MemoryCore)
/// owns — `key`, `content`, `category`, `session_id`, `taint` — plus its
/// namespace and timestamp. Document-tier attributes (`title`, `tags`,
/// `metadata`, `source_type`, `priority`) belong to the `Documents` family and
/// are out of scope; a re-import synthesises them exactly as a normal store
/// does. That is a stated limitation of a mandatory-only export, not an
/// oversight — it widens when a driver's Documents family joins the export.
#[must_use]
pub fn to_record(entry: MemoryEntry) -> ExportRecord {
    ExportRecord {
        kind: ENTRY_KIND.to_string(),
        id: entry.id,
        namespace: entry.namespace,
        taint: entry.taint,
        payload: serde_json::json!({
            "key": entry.key,
            "content": entry.content,
            "category": entry.category.to_string(),
            "session_id": entry.session_id,
            "timestamp": entry.timestamp,
        }),
    }
}

/// What [`import_records`] needs out of one record's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedEntry {
    /// Owning namespace, defaulted to [`GLOBAL_NAMESPACE`] when the record has
    /// none.
    pub namespace: String,
    /// The entry key.
    pub key: String,
    /// The entry body.
    pub content: String,
    /// The entry category.
    pub category: MemoryCategory,
    /// The originating session, when the record carried one.
    pub session_id: Option<String>,
}

/// Reads a record into the fields a [`Memory`] backend needs to store it.
///
/// # Errors
///
/// An operator-facing reason with **no record content in it** — these strings
/// land in [`ImportOutcome::errors`], which is logged.
pub fn read_record(record: &ExportRecord) -> Result<ImportedEntry, String> {
    if record.kind != ENTRY_KIND {
        return Err(format!(
            "record {}: unsupported kind '{}' (this driver exports '{ENTRY_KIND}')",
            record.id, record.kind
        ));
    }
    let string_field = |name: &str| -> Result<&str, String> {
        record
            .payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("record {}: payload is missing a string '{name}'", record.id))
    };
    let key = string_field("key")?;
    let content = string_field("content")?;
    let category: MemoryCategory = string_field("category")?
        .parse()
        .map_err(|_| format!("record {}: category is not a known category", record.id))?;

    Ok(ImportedEntry {
        namespace: record
            .namespace
            .clone()
            .unwrap_or_else(|| GLOBAL_NAMESPACE.to_string()),
        key: key.to_string(),
        content: content.to_string(),
        category,
        session_id: record
            .payload
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// `MemoryPortability::export_page` over a [`Memory`] backend.
///
/// Composed from `namespace_summaries()` and `list()`. Deliberately *not* from
/// a document-listing query: those typically select metadata and no `content`,
/// so an export built on one round-trips titles and loses every byte of memory.
///
/// The cursor is `"{namespace_index}:{offset}"`, indexing into
/// `namespace_summaries()`, whose ordering is stable. An empty page is **not**
/// a terminator — only a `None` next-cursor is — so an empty namespace advances
/// the index and keeps paging.
///
/// # Errors
///
/// [`MemoryError::Invalid`] for a zero limit or a cursor this driver did not
/// issue; otherwise backend failures.
pub async fn export_page(
    memory: &dyn Memory,
    cursor: Option<&str>,
    limit: usize,
) -> Result<ExportPage, MemoryError> {
    if limit == 0 {
        return Err(MemoryError::Invalid(
            "export page limit must be greater than zero".to_string(),
        ));
    }
    let (index, offset) = parse_cursor(cursor)?;
    let summaries = memory.namespace_summaries().await.map_err(engine_error)?;

    if index >= summaries.len() {
        // A start-of-export against an empty store lands here legitimately; any
        // other out-of-range index came from a cursor we did not issue.
        if cursor.is_some() && !summaries.is_empty() {
            return Err(MemoryError::Invalid(format!(
                "export cursor names namespace #{index}, but this driver holds {}",
                summaries.len()
            )));
        }
        return Ok(ExportPage {
            records: Vec::new(),
            next_cursor: None,
        });
    }

    let namespace = &summaries[index].namespace;
    let entries = memory
        .list(Some(namespace), None, None)
        .await
        .map_err(engine_error)?;
    if offset > entries.len() {
        return Err(MemoryError::Invalid(format!(
            "export cursor offset {offset} is past the end of namespace #{index}"
        )));
    }

    let end = offset.saturating_add(limit).min(entries.len());
    let records: Vec<ExportRecord> = entries[offset..end]
        .iter()
        .cloned()
        .map(to_record)
        .collect();

    let next_cursor = if end < entries.len() {
        Some(format!("{index}:{end}"))
    } else if index + 1 < summaries.len() {
        Some(format!("{}:0", index + 1))
    } else {
        None
    };

    log::debug!(
        "[tinymemory:mandatory] export_page index={index} offset={offset} emitted={} more={}",
        records.len(),
        next_cursor.is_some()
    );
    Ok(ExportPage {
        records,
        next_cursor,
    })
}

/// `MemoryPortability::import_records` over a [`Memory`] backend.
///
/// Each record is stored with its **own** [`MemoryTaint`](crate::types::MemoryTaint) via
/// [`Memory::store_with_taint`]: an importing driver must persist the
/// provenance it is given and must not re-stamp it. [`Memory::store`] would
/// stamp [`MemoryTaint::Internal`](crate::types::MemoryTaint::Internal), quietly upgrading the trust of every
/// externally-sourced record in a restore.
///
/// Per-record rejection is reported in [`ImportOutcome`], never fatal: a
/// million-record restore must not abort on one malformed row.
///
/// # Errors
///
/// Reserved for failures that make the whole batch meaningless — a backend
/// write that fails aborts the batch, because continuing would report a partial
/// restore as a successful one.
pub async fn import_records(
    memory: &dyn Memory,
    records: Vec<ExportRecord>,
) -> Result<ImportOutcome, MemoryError> {
    let mut outcome = ImportOutcome::default();

    for record in records {
        let entry = match read_record(&record) {
            Ok(entry) => entry,
            Err(reason) => {
                outcome.failed = outcome.failed.saturating_add(1);
                outcome.errors.push(reason);
                continue;
            }
        };

        memory
            .store_with_taint(
                &entry.namespace,
                &entry.key,
                &entry.content,
                entry.category,
                entry.session_id.as_deref(),
                record.taint,
            )
            .await
            .map_err(engine_error)?;
        outcome.imported = outcome.imported.saturating_add(1);
    }

    log::debug!(
        "[tinymemory:mandatory] import_records imported={} failed={}",
        outcome.imported,
        outcome.failed
    );
    Ok(outcome)
}
