//! Copying a store from one bound provider into another.
//!
//! Engine-neutral by construction: every step speaks contract families only,
//! so any provider can be the source or the target and the module needs no
//! engine feature.
//!
//! [`copy`] moves the keyed records, through the mandatory
//! [`MemoryPortability`](crate::provider::MemoryPortability) family: it walks
//! the source's export pages until the cursor ends and feeds each page to the
//! target's `import_records`.
//!
//! [`copy_all`] runs [`copy`] and then the steps the mandatory export cannot
//! carry, each over the families both sides serve (see [`MigrateStep`]):
//! document details, goals, the learned profile, the episodic record, and the
//! ingested content, re-sent raw so the target derives its own summary tree.
//! A step whose family one side does not serve is reported as skipped with
//! the reason, not failed.
//!
//! Every step is **at-least-once and non-destructive**: nothing is deleted
//! from the source, and a target that already holds what a step would write
//! reports it as unchanged rather than duplicating it, so a copy that stopped
//! part-way is finished by running it again. Partial failure inside a step is
//! reported in its counts, not raised, because one malformed record should not
//! abort a large move; a failure that makes the rest of a step meaningless —
//! the target is down — is raised.

mod content;
mod episodic;
mod families;

use crate::provider::MemoryProvider;

/// How many records one export page asks for.
pub const PAGE_LIMIT: usize = 500;

/// A hard stop on pages, so a source that never ends its cursor cannot loop
/// forever.
const MAX_PAGES: usize = 100_000;

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
/// the source's cursor never terminates (a cursor equal to the one just used,
/// or one already seen, is refused rather than followed). Records already imported stay
/// imported; rerunning is safe because targets skip records they recognise.
pub async fn copy(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    mut progress: impl FnMut(MigrateProgress),
) -> anyhow::Result<MigrateReport> {
    let mut report = MigrateReport::default();
    let mut cursor: Option<String> = None;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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
            Some(next) => {
                // A source that hands back a cursor it already issued would
                // otherwise re-export the same pages until the page cap.
                anyhow::ensure!(
                    cursor.as_deref() != Some(next.as_str()) && seen.insert(next.clone()),
                    "the source's export cursor repeated after {} pages; refusing to loop",
                    report.pages
                );
                cursor = Some(next);
            }
            None => return Ok(report),
        }
    }
}

/// One step of [`copy_all`], in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MigrateStep {
    /// Keyed records, through the mandatory export — [`copy`].
    Records,
    /// Document titles, tags, source types, priorities and metadata, which
    /// the mandatory export does not carry. Needs `Documents` on both sides.
    Documents,
    /// The goals document. Needs `Goals` on both sides.
    Goals,
    /// The learned profile's facets. Needs `Profile` on both sides.
    Profile,
    /// Turns, segments, events and segment embeddings. Needs
    /// `EpisodicPortability` on both sides.
    Episodic,
    /// Ingested content, re-sent raw for the target to chunk, embed and
    /// summarise itself — a summary tree is derived, so it is rebuilt rather
    /// than copied. Needs `Chunks` on the source and `Ingest` on the target.
    Content,
}

impl MigrateStep {
    /// Every step, in the order [`copy_all`] runs them.
    pub const ALL: [MigrateStep; 6] = [
        MigrateStep::Records,
        MigrateStep::Documents,
        MigrateStep::Goals,
        MigrateStep::Profile,
        MigrateStep::Episodic,
        MigrateStep::Content,
    ];

    /// Stable snake_case name, for logs and a host's progress surface.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Documents => "documents",
            Self::Goals => "goals",
            Self::Profile => "profile",
            Self::Episodic => "episodic",
            Self::Content => "content",
        }
    }
}

impl std::fmt::Display for MigrateStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one step of [`copy_all`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    /// Which step.
    pub step: MigrateStep,
    /// Why the step did not run — a side does not serve the family it needs,
    /// or the caller turned it off. `None` when it ran.
    pub skipped_because: Option<String>,
    /// Items read from the source.
    pub read: usize,
    /// Items the target wrote.
    pub written: usize,
    /// Items the target already held as the source has them.
    pub unchanged: usize,
    /// Items the target refused, or the source could not hand over.
    pub failed: usize,
    /// Operator-facing reasons for the failures, bounded (at most 20). They
    /// name items, never their content.
    pub errors: Vec<String>,
}

impl StepReport {
    fn new(step: MigrateStep) -> Self {
        Self {
            step,
            skipped_because: None,
            read: 0,
            written: 0,
            unchanged: 0,
            failed: 0,
            errors: Vec::new(),
        }
    }

    fn skipped(step: MigrateStep, reason: impl Into<String>) -> Self {
        Self {
            skipped_because: Some(reason.into()),
            ..Self::new(step)
        }
    }

    /// Counts one failure and keeps its reason while there is room.
    fn fail(&mut self, reason: String) {
        self.failed += 1;
        self.note(reason);
    }

    /// Keeps a reason while there is room, without counting a failure — for
    /// a reason that covers failures already counted.
    fn note(&mut self, reason: String) {
        if self.errors.len() < 20 {
            self.errors.push(reason);
        }
    }
}

/// The result of a completed [`copy_all`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyAllReport {
    /// What [`copy`] did with the keyed records.
    pub records: MigrateReport,
    /// Every later step, in [`MigrateStep::ALL`] order after
    /// [`MigrateStep::Records`].
    pub steps: Vec<StepReport>,
}

impl CopyAllReport {
    /// Items that failed across every step, the keyed records included.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.records.failed + self.steps.iter().map(|step| step.failed).sum::<usize>()
    }

    /// The report for `step`, when it is one of [`Self::steps`].
    #[must_use]
    pub fn step(&self, step: MigrateStep) -> Option<&StepReport> {
        self.steps.iter().find(|report| report.step == step)
    }
}

/// Progress through [`copy_all`]: the step under way and its running counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyProgress {
    /// The step under way.
    pub step: MigrateStep,
    /// Items this step has read so far.
    pub read: usize,
    /// Items this step has written so far.
    pub written: usize,
}

/// What [`copy_all`] moves beyond the keyed records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyOptions {
    /// Whether to run [`MigrateStep::Content`]. Re-sending content makes the
    /// target chunk, embed and summarise it again, which a hosted target
    /// bills for, so a host offers it as a choice.
    pub replay_content: bool,
    /// Logical sources whose id starts with one of these are not re-sent by
    /// [`MigrateStep::Content`]. A host that syncs some sources into the new
    /// driver itself, from scratch, names them here so their content is not
    /// sent twice.
    pub skip_source_prefixes: Vec<String>,
    /// Namespaces whose name starts with one of these are left out of
    /// [`MigrateStep::Documents`]. The defaults are where the embedded engine
    /// (`source:`) and hosted memory (`sources/`) keep synced items: their
    /// details describe a sync, and their content moves with the keyed
    /// records and the replay.
    pub skip_namespace_prefixes: Vec<String>,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            replay_content: true,
            skip_source_prefixes: Vec::new(),
            skip_namespace_prefixes: vec!["source:".to_string(), "sources/".to_string()],
        }
    }
}

/// Copies everything both providers can exchange from `from` into `to`:
/// the keyed records, then each later [`MigrateStep`] in order.
///
/// `progress` is called as each step moves, with that step's running counts.
///
/// # Errors
///
/// What [`copy`] raises, and a failure that makes a later step meaningless:
/// the source cannot be read, the target refuses a whole batch, or an export
/// cursor repeats. Steps that already ran stay done; rerunning is safe.
pub async fn copy_all(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    options: &CopyOptions,
    mut progress: impl FnMut(CopyProgress),
) -> anyhow::Result<CopyAllReport> {
    let records = copy(from, to, |page| {
        progress(CopyProgress {
            step: MigrateStep::Records,
            read: page.records,
            written: page.imported,
        });
    })
    .await?;
    let mut steps = Vec::new();
    steps.push(families::documents(from, to, options, &mut progress).await?);
    steps.push(families::goals(from, to, &mut progress).await?);
    steps.push(families::profile(from, to, &mut progress).await?);
    steps.push(episodic::copy(from, to, &mut progress).await?);
    steps.push(content::replay(from, to, options, &mut progress).await?);
    Ok(CopyAllReport { records, steps })
}

/// Why a step needing `family` on `side` cannot run.
fn unserved(side: &str, family: crate::capabilities::Capability) -> String {
    format!("the {side} does not serve {family}")
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_steps;

#[cfg(test)]
mod test_support;
