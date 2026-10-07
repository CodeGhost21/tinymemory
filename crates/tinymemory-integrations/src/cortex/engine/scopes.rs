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
//!   are always read. Neither enters a service sandbox below its node
//!   ([`Reach::admitted_by`]); only [`CortexEngine::every_scope`], which looks
//!   ids up wherever they live, does.
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
use crate::cortex::envelope::ScopeLayout;
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
    /// The scope of `kind` at `namespace` in `layout`.
    pub(crate) fn new(layout: &ScopeLayout, namespace: Namespace, kind: ItemKind) -> Self {
        Self {
            order: ItemKind::ALL
                .iter()
                .position(|k| *k == kind)
                .unwrap_or_default(),
            path: layout.path(&namespace, kind),
            namespace,
            kind,
        }
    }
}

/// The scopes `reach` reads exactly (no discovery): its nodes for each kind.
pub(crate) fn known(layout: &ScopeLayout, reach: &Reach, kinds: &[ItemKind]) -> Vec<KindScope> {
    let mut scopes: Vec<KindScope> = reach
        .nodes()
        .into_iter()
        .flat_map(|node| {
            kinds
                .iter()
                .map(move |kind| KindScope::new(layout, node.clone(), *kind))
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
        Ok(known(&self.layout, base, &kinds))
    }

    /// The scopes of `kinds` in `reach` that CortexDB has registered — only
    /// those with something written — kind first then namespace. Unlike a
    /// read, nothing is assumed to exist: a build of an empty scope would be
    /// wasted model time.
    pub(super) async fn held(&self, reach: &Reach, kinds: &[ItemKind]) -> Result<Vec<KindScope>> {
        let mut found = BTreeSet::new();
        for path in self.log.scopes(self.layout.root()).await? {
            let Some((namespace, kind)) = self.layout.parse(&path) else {
                continue;
            };
            if reach.admits(&namespace) && kinds.contains(&kind) {
                found.insert(KindScope::new(&self.layout, namespace, kind));
            }
        }
        Ok(found.into_iter().collect())
    }

    /// The scopes of `kinds` the engine has registered at the nodes `reach`
    /// admits (`inherit` ignored), refusing when it cannot list them all:
    /// for an erasure, which must name every scope it removes.
    pub(super) async fn held_all(
        &self,
        reach: &Reach,
        kinds: &[ItemKind],
    ) -> Result<Vec<KindScope>> {
        let reach = Reach {
            inherit: false,
            ..reach.clone()
        };
        let mut found = BTreeSet::new();
        let mut paths = Vec::new();
        for prefix in self.layout.node_prefixes(&reach.at) {
            paths.extend(self.log.all_scopes(&prefix).await?);
        }
        for path in paths {
            let Some((namespace, kind)) = self.layout.parse(&path) else {
                continue;
            };
            if reach.admits(&namespace) && kinds.contains(&kind) {
                found.insert(KindScope::new(&self.layout, namespace, kind));
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
            Some(reach) => known(&self.layout, reach, kinds).into_iter().collect(),
            None => known(&self.layout, &Reach::exact(Namespace::ROOT), kinds)
                .into_iter()
                .collect(),
        };
        let root = Namespace::ROOT;
        let at = reach.map_or(&root, |reach| &reach.at);
        let mut paths = Vec::new();
        for prefix in self.layout.node_prefixes(at) {
            paths.extend(match listing {
                Listing::Lenient => self.log.scopes(&prefix).await?,
                Listing::Complete => self.log.all_scopes(&prefix).await?,
            });
        }
        for path in paths {
            let Some((namespace, kind)) = self.layout.parse(&path) else {
                continue;
            };
            if Reach::admitted_by(reach, &namespace) && kinds.contains(&kind) {
                found.insert(KindScope::new(&self.layout, namespace, kind));
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Every scope the engine holds, of every kind, service sandboxes
    /// included: where an id, which names one item wherever it lives, is
    /// looked up. The listing must be complete (`log::read::all_scopes`), so
    /// an id is never reported missing because its scope was cut off.
    ///
    /// # Errors
    ///
    /// [`crate::cortex::Error::Engine`] when the scope listing reaches its
    /// limit, and the backend failures of the listing itself.
    pub(super) async fn every_scope(&self) -> Result<Vec<KindScope>> {
        let mut found: BTreeSet<KindScope> =
            known(&self.layout, &Reach::exact(Namespace::ROOT), &ItemKind::ALL)
                .into_iter()
                .collect();
        for path in self.log.all_scopes(self.layout.root()).await? {
            if let Some((namespace, kind)) = self.layout.parse(&path) {
                found.insert(KindScope::new(&self.layout, namespace, kind));
            }
        }
        Ok(found.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "scopes_tests.rs"]
mod tests;
