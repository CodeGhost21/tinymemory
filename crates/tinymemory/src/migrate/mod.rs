//! Copying every record from one bound provider into another.
//!
//! Engine-neutral by construction: it speaks only the mandatory
//! [`MemoryPortability`](crate::provider::MemoryPortability) family, so any
//! provider can be the source or the target and the module needs no engine
//! feature.
//!
//! [`copy`] walks the source's export pages until the cursor ends and feeds each
//! page to the target's `import_records`. It is **at-least-once and
//! non-destructive**: nothing is deleted from the source, and a target that
//! recognises a record reports it as skipped rather than duplicating it.
//! Partial failure inside a page is reported through [`MigrateReport::failed`],
//! not raised, because one malformed record should not abort a large move.

use crate::provider::MemoryProvider;

/// How many records one export page asks for.
pub const PAGE_LIMIT: usize = 500;

/// A hard stop on pages, so a source that never ends its cursor cannot loop
/// forever.
const MAX_PAGES: usize = 1_000_000;

/// Progress after each page is imported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrateProgress {
    /// Pages processed so far, including this one.
    pub pages: usize,
    /// Records read from the source so far.
    pub records: usize,
    /// Records the target reports written so far.
    pub imported: usize,
}

/// The result of a completed [`copy`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrateReport {
    /// Export pages read.
    pub pages: usize,
    /// Records read from the source.
    pub records: usize,
    /// Records the target reported written.
    pub imported: usize,
    /// Records the target recognised as already present.
    pub skipped: usize,
    /// Records the target rejected.
    pub failed: usize,
    /// Operator-facing reasons for the failures, bounded (at most 20).
    pub errors: Vec<String>,
}

/// Copies every exportable record from `from` into `to`.
///
/// `progress` is called once per page, after that page has been imported.
///
/// # Errors
///
/// Fails if the source cannot be exported, the target cannot import a page, or
/// the source's cursor never terminates. Records already imported stay
/// imported; rerunning is safe because targets skip records they recognise.
pub async fn copy(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    mut progress: impl FnMut(MigrateProgress),
) -> anyhow::Result<MigrateReport> {
    let mut report = MigrateReport::default();
    let mut cursor: Option<String> = None;
    loop {
        anyhow::ensure!(
            report.pages < MAX_PAGES,
            "the source's export did not terminate after {MAX_PAGES} pages"
        );
        let page = from.export_page(cursor.as_deref(), PAGE_LIMIT).await?;
        report.pages += 1;
        report.records += page.records.len();
        if !page.records.is_empty() {
            let outcome = to.import_records(page.records).await?;
            report.imported += outcome.imported as usize;
            report.skipped += outcome.skipped as usize;
            report.failed += outcome.failed as usize;
            for error in outcome.errors {
                if report.errors.len() < 20 {
                    report.errors.push(error);
                }
            }
        }
        progress(MigrateProgress {
            pages: report.pages,
            records: report.records,
            imported: report.imported,
        });
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(report),
        }
    }
}

#[cfg(test)]
mod test;
