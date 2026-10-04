//! [`Items`]: the streaming iterator over a legacy workspace.
//!
//! The iterator walks the sections in their fixed order (documents, chunks,
//! conversations, learnings, profile), fetching one page of keys at a time,
//! so memory stays bounded by the page size (and, for a conversation or a
//! chunk source, by that one thread or source).
//!
//! It keeps two positions. The *scan* position advances over every key read,
//! skipped or not, so the next page starts after it. The *checkpoint*
//! advances only when an item is yielded and is what each [`ImportedItem`]
//! carries; resuming from it re-reads at most the skipped rows after the last
//! yielded item, which are skipped again.

use std::collections::VecDeque;
use std::iter::FusedIterator;

use crate::import::checkpoint::{Checkpoint, ImportedItem};
use crate::import::error::Result;
use crate::import::sections::{ORDER, Scanned};
use crate::import::workspace::LegacyWorkspace;

/// Keys fetched per query unless [`Items::with_page_size`] says otherwise.
pub const DEFAULT_PAGE_SIZE: usize = 256;

/// Every importable item of a [`LegacyWorkspace`], in a deterministic order,
/// each with the [`Checkpoint`] to persist after storing it.
///
/// Yields `Err` at most once: after an error the iterator is exhausted. A host
/// that retries resumes from the last checkpoint it persisted.
#[derive(Debug)]
pub struct Items<'w> {
    workspace: &'w LegacyWorkspace,
    section: usize,
    scan: Checkpoint,
    checkpoint: Checkpoint,
    buffer: VecDeque<Scanned>,
    page_size: usize,
    finished: bool,
}

impl<'w> Items<'w> {
    pub(crate) fn new(workspace: &'w LegacyWorkspace, checkpoint: Checkpoint) -> Self {
        Self {
            workspace,
            section: 0,
            scan: checkpoint.clone(),
            checkpoint,
            buffer: VecDeque::new(),
            page_size: DEFAULT_PAGE_SIZE,
            finished: false,
        }
    }

    /// Sets how many keys each query fetches (at least one). The order and
    /// content of what is yielded do not depend on it.
    #[must_use]
    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size.max(1);
        self
    }

    fn step(&mut self) -> Option<Result<ImportedItem>> {
        loop {
            if let Some(scanned) = self.buffer.pop_front() {
                scanned.mark.clone().apply(&mut self.scan);
                if let Some(item) = scanned.item {
                    scanned.mark.apply(&mut self.checkpoint);
                    return Some(Ok(ImportedItem {
                        item,
                        checkpoint: self.checkpoint.clone(),
                    }));
                }
                continue;
            }
            let section = *ORDER.get(self.section)?;
            match section.page(self.workspace, &self.scan, self.page_size) {
                Ok(page) if page.is_empty() => self.section += 1,
                Ok(page) => self.buffer.extend(page),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

impl Iterator for Items<'_> {
    type Item = Result<ImportedItem>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let next = self.step();
        if matches!(next, Some(Err(_)) | None) {
            self.finished = true;
            self.buffer.clear();
        }
        next
    }
}

impl FusedIterator for Items<'_> {}
