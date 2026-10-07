//! Which CortexDB scopes an operation reads.
//!
//! Every item lives in the scope of its kind under its namespace node (see
//! `envelope`). A [`MetaFilter`] names the kinds and the [`Reach`]; this
//! module turns them into the exact scopes to read, ordered by kind
//! ([`ItemKind::ALL`]) and then by namespace, so a cursor can resume by
//! position.
//!
//! - **A reach without descendants** reads `at` and, when it inherits, each
//!   ancestor: the nodes are known, so no request is needed. A node nothing
//!   was written to lists empty.
//! - **A subtree reach, or no reach at all,** needs the nodes below, which
//!   only the engine knows: they are discovered once per call from the
//!   registered scopes under the TinyMemory root. The root's own kind scopes
//!   are always read.
//!
//! Reads are always exact: every pack names one scope with
//! `view: "granular"`. Server-side traversal is never relied on (CortexDB's
//! public recall defaults to `holistic`, which also reads ancestors and
//! descendants). An inherited ancestor is still read, deliberately, but as a
//! scope of its own in the list above, never through traversal from a scope
//! below it. So one agent's read never strays into a sibling's scope, and no
//! pack is CortexDB's storage-order sample of a parent's children.

use std::collections::BTreeSet;

use tinymemory_api::{ItemKind, MetaFilter, Namespace, Reach};

use super::CortexEngine;
use super::items::admitted;
use crate::cortex::envelope::{ROOT_SCOPE, parse_scope, scope_path};
use crate::cortex::error::Result;

/// One scope to read: a kind at a namespace node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct KindScope {
    /// Position of the kind in [`ItemKind::ALL`]; the primary sort key.
    order: usize,
    /// The node.
    pub(crate) namespace: Namespace,
    /// The kind.
    pub(crate) kind: ItemKind,
    /// The scope path.
    pub(crate) path: String,
}

impl KindScope {
    /// The scope of `kind` at `namespace`.
    pub(crate) fn new(namespace: Namespace, kind: ItemKind) -> Self {
        Self {
            order: ItemKind::ALL
                .iter()
                .position(|k| *k == kind)
                .unwrap_or_default(),
            path: scope_path(&namespace, kind),
            namespace,
            kind,
        }
    }
}

/// The scopes `reach` reads exactly (no discovery): its nodes for each kind.
pub(crate) fn known(reach: &Reach, kinds: &[ItemKind]) -> Vec<KindScope> {
    let mut scopes: Vec<KindScope> = reach
        .nodes()
        .into_iter()
        .flat_map(|node| {
            kinds
                .iter()
                .map(move |kind| KindScope::new(node.clone(), *kind))
        })
        .collect();
    scopes.sort();
    scopes
}

/// How a discovery treats a scope listing that may be missing scopes.
#[derive(Clone, Copy)]
enum Listing {
    /// Reads what was listed (the listing logs what it could not list).
    Lenient,
    /// Refuses: the caller must see every scope.
    Complete,
}

/// Whether reading `reach` needs the engine's list of nodes.
fn needs_discovery(reach: Option<&Reach>) -> bool {
    reach.is_none_or(|reach| reach.descendants)
}

impl CortexEngine {
    /// The scopes `filter` reads, kind first then namespace. See the module
    /// docs.
    pub(super) async fn scopes_for(&self, filter: &MetaFilter) -> Result<Vec<KindScope>> {
        self.scopes_listed(filter, Listing::Lenient).await
    }

    /// As [`Self::scopes_for`], refusing when the engine cannot list every
    /// scope (see `log::read::all_scopes`): for an export, which must not
    /// silently miss items.
    ///
    /// # Errors
    ///
    /// [`crate::cortex::Error::Engine`] when the scope listing reaches its
    /// limit and may be missing scopes, and the backend failures of the
    /// listing itself.
    pub(super) async fn all_scopes_for(&self, filter: &MetaFilter) -> Result<Vec<KindScope>> {
        self.scopes_listed(filter, Listing::Complete).await
    }

    async fn scopes_listed(&self, filter: &MetaFilter, listing: Listing) -> Result<Vec<KindScope>> {
        let kinds = admitted(filter);
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let reach = filter.reach.as_ref();
        let Some(base) = reach.filter(|_| !needs_discovery(reach)) else {
            return self.discovered(reach, &kinds, listing).await;
        };
        Ok(known(base, &kinds))
    }

    /// The scopes of `kinds` in `reach` that CortexDB has registered — only
    /// those with something written — kind first then namespace. Unlike a
    /// read, nothing is assumed to exist: a build of an empty scope would be
    /// wasted model time.
    pub(super) async fn held(&self, reach: &Reach, kinds: &[ItemKind]) -> Result<Vec<KindScope>> {
        let mut found = BTreeSet::new();
        for path in self.log.scopes(ROOT_SCOPE).await? {
            let Some((namespace, kind)) = parse_scope(&path) else {
                continue;
            };
            if reach.admits(&namespace) && kinds.contains(&kind) {
                found.insert(KindScope::new(namespace, kind));
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Every scope of `kinds` the engine holds, in reach.
    async fn discovered(
        &self,
        reach: Option<&Reach>,
        kinds: &[ItemKind],
        listing: Listing,
    ) -> Result<Vec<KindScope>> {
        let mut found: BTreeSet<KindScope> = match reach {
            Some(reach) => known(reach, kinds).into_iter().collect(),
            None => known(&Reach::exact(Namespace::ROOT), kinds)
                .into_iter()
                .collect(),
        };
        let prefix = match reach {
            Some(reach) if !reach.at.is_root() => {
                let mut path = scope_path(&reach.at, ItemKind::Document);
                path.truncate(path.rfind('/').unwrap_or(path.len()));
                path
            }
            _ => ROOT_SCOPE.to_string(),
        };
        let paths = match listing {
            Listing::Lenient => self.log.scopes(&prefix).await?,
            Listing::Complete => self.log.all_scopes(&prefix).await?,
        };
        for path in paths {
            let Some((namespace, kind)) = parse_scope(&path) else {
                continue;
            };
            let in_reach = reach.is_none_or(|reach| reach.admits(&namespace));
            if in_reach && kinds.contains(&kind) {
                found.insert(KindScope::new(namespace, kind));
            }
        }
        Ok(found.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "scopes_tests.rs"]
mod tests;
